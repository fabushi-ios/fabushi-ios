use crate::gateway_server::GatewayEventHub;
use super::mcp_service::{
    BoxServerStatus, CatalogField, CatalogPlugin, CatalogSkill, EffectivePlugin,
    McpManagerBackend, McpServerSummary,
};
use super::plugin_skills::{LoadedPlugins, PluginSkillsLoader};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, RecvTimeoutError, SyncSender},
};
use std::time::Duration;
use thiserror::Error;

pub const HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL: &str = "host-mcp-lifecycle-request";
pub const MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD: &str = "mcp.resolveLifecycleRequest";
pub const MCP_LIST_SERVERS_METHOD: &str = "listHostMcpServers";
pub const MCP_LIST_CATALOG_METHOD: &str = "listHostMcpCatalog";
pub const MCP_LIST_EFFECTIVE_PLUGINS_METHOD: &str = "listHostEffectiveMcpPlugins";
pub const MCP_LOAD_PLUGIN_SKILLS_METHOD: &str = "loadHostPluginSkills";
pub const MCP_INSTALL_PLUGIN_METHOD: &str = "installHostMcpPlugin";
pub const MCP_UNINSTALL_PLUGIN_METHOD: &str = "uninstallHostMcpPlugin";
pub const MCP_ADD_SERVER_METHOD: &str = "addHostMcpServer";
pub const MCP_REMOVE_SERVER_METHOD: &str = "removeHostMcpServer";
pub const MCP_RESTART_SERVERS_METHOD: &str = "restartHostMcpServers";
pub const MCP_SET_INSTRUCTIONS_METHOD: &str = "setHostMcpInstructions";
pub const MCP_AUTHENTICATE_SERVER_METHOD: &str = "authenticateHostMcpServer";
pub const MCP_LOGOUT_ACCOUNT_METHOD: &str = "logoutHostMcpAccount";
pub const MCP_REMOVE_ACCOUNT_METHOD: &str = "removeHostMcpAccount";
pub const MCP_RENAME_ACCOUNT_METHOD: &str = "renameHostMcpAccount";
pub const MCP_NOTE_AUTH_COMPLETED_METHOD: &str = "noteHostMcpAuthCompleted";

const DEFAULT_MCP_LIFECYCLE_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Error, PartialEq)]
pub enum CoordinatorMcpLifecycleRelayError {
    #[error("unsupported Host MCP lifecycle relay method: {0}")]
    UnsupportedMethod(String),
    #[error("Host MCP lifecycle relay request {0} timed out")]
    Timeout(String),
    #[error("Host MCP lifecycle relay request {request_id} failed: {message}")]
    Remote { request_id: String, message: String },
    #[error("Host MCP lifecycle relay is closed: {0}")]
    Closed(String),
    #[error("invalid Host MCP lifecycle relay response: {0}")]
    Protocol(String),
}

type PendingReply = SyncSender<Result<Value, CoordinatorMcpLifecycleRelayError>>;

pub fn is_host_mcp_lifecycle_method(method: &str) -> bool {
    matches!(
        method,
        MCP_LIST_SERVERS_METHOD
            | MCP_LIST_CATALOG_METHOD
            | MCP_LIST_EFFECTIVE_PLUGINS_METHOD
            | MCP_LOAD_PLUGIN_SKILLS_METHOD
            | MCP_INSTALL_PLUGIN_METHOD
            | MCP_UNINSTALL_PLUGIN_METHOD
            | MCP_ADD_SERVER_METHOD
            | MCP_REMOVE_SERVER_METHOD
            | MCP_RESTART_SERVERS_METHOD
            | MCP_SET_INSTRUCTIONS_METHOD
            | MCP_AUTHENTICATE_SERVER_METHOD
            | MCP_LOGOUT_ACCOUNT_METHOD
            | MCP_REMOVE_ACCOUNT_METHOD
            | MCP_RENAME_ACCOUNT_METHOD
            | MCP_NOTE_AUTH_COMPLETED_METHOD
    )
}

#[derive(Clone)]
pub struct CoordinatorPluginSkillsLoader {
    relay: Arc<CoordinatorMcpLifecycleRelay>,
}

impl CoordinatorPluginSkillsLoader {
    pub fn new(relay: Arc<CoordinatorMcpLifecycleRelay>) -> Self {
        Self { relay }
    }
}

impl PluginSkillsLoader for CoordinatorPluginSkillsLoader {
    fn load(&self) -> Result<LoadedPlugins, String> {
        let payload = self
            .relay
            .request(MCP_LOAD_PLUGIN_SKILLS_METHOD, json!({}))
            .map_err(|error| error.to_string())?;
        serde_json::from_value(payload)
            .map_err(|error| format!("invalid Plugin Skills loader response: {error}"))
    }
}

pub struct CoordinatorMcpLifecycleRelay {
    events: GatewayEventHub,
    pending: Mutex<HashMap<String, PendingReply>>,
    next_request_id: AtomicU64,
    timeout: Duration,
    closed: AtomicBool,
}

impl CoordinatorMcpLifecycleRelay {
    pub fn new(events: GatewayEventHub) -> Self {
        Self::with_timeout(events, DEFAULT_MCP_LIFECYCLE_REQUEST_TIMEOUT)
    }

    pub fn with_timeout(events: GatewayEventHub, timeout: Duration) -> Self {
        Self {
            events,
            pending: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(0),
            timeout,
            closed: AtomicBool::new(false),
        }
    }

    pub fn request(&self, method: &str, args: Value) -> Result<Value, CoordinatorMcpLifecycleRelayError> {
        if !is_host_mcp_lifecycle_method(method) {
            return Err(CoordinatorMcpLifecycleRelayError::UnsupportedMethod(method.to_string()));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(CoordinatorMcpLifecycleRelayError::Closed("Mahayana Host is shutting down".into()));
        }

        let sequence = self.next_request_id.fetch_add(1, Ordering::AcqRel) + 1;
        let request_id = format!("host-mcp-{sequence}");
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| CoordinatorMcpLifecycleRelayError::Protocol("pending relay lock poisoned".into()))?
            .insert(request_id.clone(), reply_tx);

        self.events.publish(json!({
            "channel": HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL,
            "payload": {
                "requestId": request_id,
                "method": method,
                "args": args,
            }
        }));

        match reply_rx.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&request_id);
                }
                Err(CoordinatorMcpLifecycleRelayError::Timeout(request_id))
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&request_id);
                }
                Err(CoordinatorMcpLifecycleRelayError::Closed(format!("reply channel disconnected for {request_id}")))
            }
        }
    }

    pub fn resolve(&self, args: &Value) -> Result<Value, CoordinatorMcpLifecycleRelayError> {
        let request_id = args
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoordinatorMcpLifecycleRelayError::Protocol("mcp.resolveLifecycleRequest requires requestId".into()))?
            .to_string();
        let ok = args
            .get("ok")
            .and_then(Value::as_bool)
            .ok_or_else(|| CoordinatorMcpLifecycleRelayError::Protocol("mcp.resolveLifecycleRequest requires boolean ok".into()))?;
        let sender = self.pending
            .lock()
            .map_err(|_| CoordinatorMcpLifecycleRelayError::Protocol("pending relay lock poisoned".into()))?
            .remove(&request_id)
            .ok_or_else(|| CoordinatorMcpLifecycleRelayError::Protocol(format!("unknown or already-settled Host MCP lifecycle request {request_id}")))?;

        let outcome = if ok {
            Ok(args.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(CoordinatorMcpLifecycleRelayError::Remote {
                request_id: request_id.clone(),
                message: args
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Host MCP lifecycle request failed")
                    .to_string(),
            })
        };
        sender
            .send(outcome)
            .map_err(|_| CoordinatorMcpLifecycleRelayError::Closed(format!("request waiter disappeared for {request_id}")))?;
        Ok(json!({"resolved": true, "requestId": request_id}))
    }

    pub fn cancel_all(&self, reason: impl Into<String>) {
        self.closed.store(true, Ordering::Release);
        let reason = reason.into();
        let pending = self.pending
            .lock()
            .map(|mut pending| pending.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        for (_request_id, sender) in pending {
            let _ = sender.send(Err(CoordinatorMcpLifecycleRelayError::Closed(reason.clone())));
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().map(|pending| pending.len()).unwrap_or_default()
    }
}


pub type BoxServerStatusLoader =
    Arc<dyn Fn(&[String], bool) -> Result<Vec<BoxServerStatus>, String> + Send + Sync>;

pub struct CoordinatorMcpManagerBackend {
    relay: Arc<CoordinatorMcpLifecycleRelay>,
    list_box_servers: BoxServerStatusLoader,
}

impl CoordinatorMcpManagerBackend {
    pub fn new(
        relay: Arc<CoordinatorMcpLifecycleRelay>,
        list_box_servers: BoxServerStatusLoader,
    ) -> Self {
        Self { relay, list_box_servers }
    }
}

fn required_string(row: &Value, key: &str, context: &str) -> Result<String, String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{context} requires string {key}"))
}

fn optional_string(row: &Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn decode_mcp_server_state(value: Value) -> Result<Vec<McpServerSummary>, String> {
    let rows = value
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| "desktop MCP owner listServers response requires servers".to_string())?;
    rows.iter()
        .map(|row| {
            Ok(McpServerSummary {
                id: required_string(row, "id", "MCP server")?,
                name: required_string(row, "name", "MCP server")?,
                server_identifier: required_string(row, "serverIdentifier", "MCP server")?,
                account_key: required_string(row, "accountKey", "MCP server")?,
                plugin_id: optional_string(row, "pluginId"),
                is_team_server: row.get("isTeamServer").and_then(Value::as_bool).unwrap_or(false),
                status: required_string(row, "status", "MCP server")?,
                status_detail: optional_string(row, "statusDetail"),
                transport: required_string(row, "transport", "MCP server")?,
                tool_count: row.get("toolCount").and_then(Value::as_u64).unwrap_or_default() as usize,
                disabled_tool_count: row.get("disabledToolCount").and_then(Value::as_u64).map(|value| value as usize),
                custom_instructions: row
                    .get("customInstructions")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

pub fn decode_mcp_catalog(value: Value) -> Result<Vec<CatalogPlugin>, String> {
    let rows = value
        .as_array()
        .ok_or_else(|| "desktop MCP owner catalog response must be an array".to_string())?;
    rows.iter()
        .map(|row| {
            let fields = row
                .get("fields")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields.iter()
                        .map(|field| {
                            Ok(CatalogField {
                                key: required_string(field, "key", "MCP catalog field")?,
                                label: field.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
                                hint: field.get("hint").and_then(Value::as_str).unwrap_or_default().to_string(),
                                is_required: field.get("isRequired").and_then(Value::as_bool).unwrap_or(false),
                                is_secret: field.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?
                .unwrap_or_default();
            let skills = row
                .get("skills")
                .and_then(Value::as_array)
                .map(|skills| {
                    skills.iter()
                        .map(|skill| {
                            Ok(CatalogSkill {
                                name: required_string(skill, "name", "MCP catalog skill")?,
                                description: optional_string(skill, "description"),
                                source_url: optional_string(skill, "sourceUrl"),
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()
                })
                .transpose()?
                .unwrap_or_default();
            Ok(CatalogPlugin {
                id: required_string(row, "id", "MCP catalog plugin")?,
                name: required_string(row, "name", "MCP catalog plugin")?,
                display_name: optional_string(row, "displayName"),
                description: optional_string(row, "description"),
                category: optional_string(row, "category"),
                fields,
                connector_count: row.get("connectors").and_then(Value::as_array).map(Vec::len).unwrap_or_default(),
                skills,
            })
        })
        .collect()
}

pub fn decode_effective_plugins(value: Value) -> Result<Vec<EffectivePlugin>, String> {
    let rows = value
        .as_array()
        .ok_or_else(|| "desktop MCP owner effective plugins response must be an array".to_string())?;
    rows.iter()
        .map(|row| {
            Ok(EffectivePlugin {
                plugin_id: required_string(row, "pluginId", "effective MCP plugin")?,
                install_mode: optional_string(row, "installMode"),
                is_enabled: row.get("isEnabled").and_then(Value::as_bool).unwrap_or(false),
                has_team_configured_variables: row
                    .get("hasTeamConfiguredVariables")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

impl McpManagerBackend for CoordinatorMcpManagerBackend {
    fn list_servers(&self) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_LIST_SERVERS_METHOD, json!({}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn list_catalog(&self, force_refresh: bool) -> Result<Vec<CatalogPlugin>, String> {
        self.relay
            .request(MCP_LIST_CATALOG_METHOD, json!({"forceRefresh": force_refresh}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_catalog)
    }

    fn list_effective_plugins(&self) -> Result<Vec<EffectivePlugin>, String> {
        self.relay
            .request(MCP_LIST_EFFECTIVE_PLUGINS_METHOD, json!({}))
            .map_err(|error| error.to_string())
            .and_then(decode_effective_plugins)
    }

    fn uninstall_plugin(&self, plugin_id: &str) -> Result<(bool, Option<String>), String> {
        let result = self.relay
            .request(MCP_UNINSTALL_PLUGIN_METHOD, json!({"pluginId": plugin_id}))
            .map_err(|error| error.to_string())?;
        let removed = result.get("removed").and_then(Value::as_bool).unwrap_or(false);
        Ok((removed, optional_string(&result, "reason")))
    }

    fn install_plugin(
        &self,
        plugin_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_INSTALL_PLUGIN_METHOD, json!({"entryId": plugin_id, "values": values}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn add_server(&self, name: &str, config_json: &str) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_ADD_SERVER_METHOD, json!({"name": name, "configJson": config_json}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn remove_server(&self, server_id: &str) -> Result<(bool, Option<String>, Vec<McpServerSummary>), String> {
        let result = self.relay
            .request(MCP_REMOVE_SERVER_METHOD, json!({"serverId": server_id}))
            .map_err(|error| error.to_string())?;
        let removed = result.get("removed").and_then(Value::as_bool).unwrap_or(false);
        let reason = optional_string(&result, "reason");
        let servers = result
            .get("state")
            .cloned()
            .or_else(|| result.get("servers").map(|servers| json!({"servers": servers})))
            .ok_or_else(|| "desktop MCP owner removeServer response requires state".to_string())
            .and_then(decode_mcp_server_state)?;
        Ok((removed, reason, servers))
    }

    fn restart_servers(&self) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_RESTART_SERVERS_METHOD, json!({}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn set_server_custom_instructions(&self, server_id: &str, instructions: &str) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_SET_INSTRUCTIONS_METHOD, json!({"serverId": server_id, "instructions": instructions}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn authenticate_server(
        &self,
        server_id: &str,
        account_key: &str,
        requesting_agent_id: Option<&str>,
        force_reauth: bool,
    ) -> Result<Value, String> {
        self.relay
            .request(MCP_AUTHENTICATE_SERVER_METHOD, json!({
                "serverId": server_id,
                "accountKey": account_key,
                "requestingAgentId": requesting_agent_id,
                "forceReauth": force_reauth,
            }))
            .map_err(|error| error.to_string())
    }

    fn logout_account(&self, server_id: &str, account_key: &str) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_LOGOUT_ACCOUNT_METHOD, json!({"serverId": server_id, "accountKey": account_key}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn remove_account(&self, server_id: &str, account_key: &str) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_REMOVE_ACCOUNT_METHOD, json!({"serverId": server_id, "accountKey": account_key}))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn rename_account(&self, server_id: &str, account_key: &str, new_account_key: &str) -> Result<Vec<McpServerSummary>, String> {
        self.relay
            .request(MCP_RENAME_ACCOUNT_METHOD, json!({
                "serverId": server_id,
                "accountKey": account_key,
                "newAccountKey": new_account_key,
            }))
            .map_err(|error| error.to_string())
            .and_then(decode_mcp_server_state)
    }

    fn list_box_servers(
        &self,
        ids: &[String],
        kick_only: bool,
    ) -> Result<Vec<BoxServerStatus>, String> {
        (self.list_box_servers)(ids, kick_only)
    }

    fn note_auth_completed_elsewhere(&self, server_id: &str, account_key: &str) {
        if let Err(error) = self.relay.request(
            MCP_NOTE_AUTH_COMPLETED_METHOD,
            json!({"serverId": server_id, "accountKey": account_key}),
        ) {
            eprintln!("Host MCP external-auth completion relay failed: {error}");
        }
    }

    fn dispose(&self) -> Result<(), String> {
        Ok(())
    }
}
