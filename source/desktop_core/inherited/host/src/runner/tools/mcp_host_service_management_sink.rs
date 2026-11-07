use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::extensions::mcp::mcp_service::McpHostService;
use crate::runner::tools::sand_mcp_management_tools::{
    ConnectorCard, McpAuthenticationResult, McpInstalledServer, McpManagementSink,
    McpPluginDetail, McpPluginField, McpPluginSkill, McpPluginSummary,
    McpRemoveServerResult, McpUninstallPluginResult,
};

pub type AwaitingUserSelectionProbe = Arc<dyn Fn() -> bool + Send + Sync>;
pub type ConnectorCardEmitter =
    Arc<dyn Fn(ConnectorCard) -> Result<(), ProviderSessionError> + Send + Sync>;

pub struct McpHostServiceManagementSink {
    service: Arc<McpHostService>,
    requesting_agent_id: Option<String>,
    multi_account_enabled: bool,
    awaiting_user_selection: Option<AwaitingUserSelectionProbe>,
    connector_card_emitter: Option<ConnectorCardEmitter>,
}

impl McpHostServiceManagementSink {
    pub fn new(
        service: Arc<McpHostService>,
        requesting_agent_id: Option<String>,
        multi_account_enabled: bool,
    ) -> Self {
        Self {
            service,
            requesting_agent_id,
            multi_account_enabled,
            awaiting_user_selection: None,
            connector_card_emitter: None,
        }
    }

    pub fn with_interaction_callbacks(
        mut self,
        awaiting_user_selection: AwaitingUserSelectionProbe,
        connector_card_emitter: ConnectorCardEmitter,
    ) -> Self {
        self.awaiting_user_selection = Some(awaiting_user_selection);
        self.connector_card_emitter = Some(connector_card_emitter);
        self
    }
}

fn tool_error(error: impl ToString) -> ProviderSessionError {
    ProviderSessionError::Tool(error.to_string())
}

fn string_field(value: &Value, key: &str) -> Result<String, ProviderSessionError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| tool_error(format!("MCP service response requires string {key}")))
}

fn optional_string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn installed_server(value: Value) -> Result<McpInstalledServer, ProviderSessionError> {
    Ok(McpInstalledServer {
        id: string_field(&value, "id")?,
        server_identifier: string_field(&value, "serverIdentifier")?,
        name: string_field(&value, "name")?,
        status: string_field(&value, "status")?,
        account_key: string_field(&value, "accountKey")?,
        transport: string_field(&value, "transport")?,
        tool_count: value.get("toolCount").and_then(Value::as_u64).unwrap_or_default() as usize,
        disabled_tool_count: value
            .get("disabledToolCount")
            .and_then(Value::as_u64)
            .map(|count| count as usize),
        plugin_id: optional_string_field(&value, "pluginId"),
        status_detail: optional_string_field(&value, "statusDetail"),
        custom_instructions: value
            .get("customInstructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_team_server: value.get("isTeamServer").and_then(Value::as_bool),
    })
}

fn installed_servers(values: Vec<Value>) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
    values.into_iter().map(installed_server).collect()
}

fn plugin_summary(value: &Value) -> Result<McpPluginSummary, ProviderSessionError> {
    let skills = value
        .get("skills")
        .and_then(Value::as_array)
        .map(|skills| {
            skills
                .iter()
                .map(|skill| {
                    Ok(McpPluginSkill {
                        name: string_field(skill, "name")?,
                        description: skill
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect::<Result<Vec<_>, ProviderSessionError>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(McpPluginSummary {
        plugin_id: string_field(value, "pluginId")?,
        name: string_field(value, "name")?,
        display_name: value
            .get("displayName")
            .and_then(Value::as_str)
            .unwrap_or_else(|| value.get("name").and_then(Value::as_str).unwrap_or_default())
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        category: value
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        is_installed: value.get("isInstalled").and_then(Value::as_bool).unwrap_or(false),
        install_mode: optional_string_field(value, "installMode").filter(|value| !value.is_empty()),
        connector_count: value
            .get("connectorCount")
            .and_then(Value::as_u64)
            .unwrap_or_default() as usize,
        skills,
    })
}

fn plugin_detail(value: Value) -> Result<McpPluginDetail, ProviderSessionError> {
    let summary = plugin_summary(&value)?;
    let fields = value
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    Ok(McpPluginField {
                        key: string_field(field, "key")?,
                        label: field
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        is_required: field
                            .get("isRequired")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        is_secret: field
                            .get("isSecret")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>, ProviderSessionError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let servers = value
        .get("servers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(McpPluginDetail {
        summary,
        fields,
        servers: installed_servers(servers)?,
    })
}

fn authentication_result(value: Value) -> Result<McpAuthenticationResult, ProviderSessionError> {
    let kind = string_field(&value, "kind")?;
    let server_name = string_field(&value, "serverName")?;
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match kind.as_str() {
        "started" => Ok(McpAuthenticationResult::Started { server_name }),
        "already-authenticated" => Ok(McpAuthenticationResult::AlreadyAuthenticated { server_name }),
        "not-configured" => Ok(McpAuthenticationResult::NotConfigured { server_name }),
        "not-supported" => Ok(McpAuthenticationResult::NotSupported { server_name, message }),
        "unreachable" => Ok(McpAuthenticationResult::Unreachable { server_name, message }),
        other => Err(tool_error(format!("unsupported MCP authentication result kind: {other}"))),
    }
}

impl McpManagementSink for McpHostServiceManagementSink {
    fn list_plugins(&self) -> Result<Vec<McpPluginSummary>, ProviderSessionError> {
        self.service
            .list_plugins()
            .map_err(tool_error)?
            .iter()
            .map(plugin_summary)
            .collect()
    }

    fn get_plugin(&self, plugin_id: &str) -> Result<Option<McpPluginDetail>, ProviderSessionError> {
        self.service
            .get_plugin(plugin_id)
            .map_err(tool_error)?
            .map(plugin_detail)
            .transpose()
    }

    fn install_plugin(
        &self,
        plugin_id: &str,
        values: Option<&BTreeMap<String, String>>,
    ) -> Result<(), ProviderSessionError> {
        let values = values
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        self.service
            .install_plugin(plugin_id, &values)
            .map(|_| ())
            .map_err(tool_error)
    }

    fn add_server(
        &self,
        name: &str,
        config_json: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(self.service.add_server(name, config_json).map_err(tool_error)?)
    }

    fn list_installed(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(self.service.list_installed().map_err(tool_error)?)
    }

    fn remove_server(&self, server_id: &str) -> Result<McpRemoveServerResult, ProviderSessionError> {
        let value = self.service.remove_server(server_id).map_err(tool_error)?;
        let servers = value
            .get("servers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(McpRemoveServerResult {
            removed: value.get("removed").and_then(Value::as_bool).unwrap_or(false),
            reason: optional_string_field(&value, "reason"),
            servers: installed_servers(servers)?,
        })
    }

    fn uninstall_plugin(&self, plugin_id: &str) -> Result<McpUninstallPluginResult, ProviderSessionError> {
        let value = self.service.uninstall_plugin(plugin_id).map_err(tool_error)?;
        Ok(McpUninstallPluginResult {
            removed: value.get("removed").and_then(Value::as_bool).unwrap_or(false),
            reason: optional_string_field(&value, "reason"),
        })
    }

    fn set_instructions(
        &self,
        server_id: &str,
        instructions: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(
            self.service
                .set_server_custom_instructions(server_id, instructions)
                .map_err(tool_error)?,
        )
    }

    fn restart(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(self.service.restart_servers().map_err(tool_error)?)
    }

    fn authenticate(
        &self,
        server_id: &str,
        account_key: &str,
        requesting_agent_id: Option<&str>,
        force_reauth: bool,
    ) -> Result<McpAuthenticationResult, ProviderSessionError> {
        authentication_result(
            self.service
                .authenticate_server(
                    server_id,
                    account_key,
                    requesting_agent_id.or(self.requesting_agent_id.as_deref()),
                    force_reauth,
                )
                .map_err(tool_error)?,
        )
    }

    fn remove_account(
        &self,
        server_id: &str,
        account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(self.service.remove_account(server_id, account_key).map_err(tool_error)?)
    }

    fn rename_account(
        &self,
        server_id: &str,
        account_key: &str,
        new_account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        installed_servers(
            self.service
                .rename_account(server_id, account_key, new_account_key)
                .map_err(tool_error)?,
        )
    }

    fn requesting_agent_id(&self) -> Option<String> {
        self.requesting_agent_id.clone()
    }

    fn is_awaiting_user_selection(&self) -> bool {
        self.awaiting_user_selection
            .as_ref()
            .is_some_and(|probe| probe())
    }

    fn is_multi_account_enabled(&self) -> bool {
        self.multi_account_enabled
    }

    fn emit_connector_card(&self, card: ConnectorCard) -> Result<(), ProviderSessionError> {
        match self.connector_card_emitter.as_ref() {
            Some(emitter) => emitter(card),
            None => Ok(()),
        }
    }
}
