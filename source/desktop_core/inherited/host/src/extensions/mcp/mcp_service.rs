use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSummary {
    pub id: String,
    pub name: String,
    pub server_identifier: String,
    pub account_key: String,
    pub plugin_id: Option<String>,
    pub is_team_server: bool,
    pub status: String,
    pub status_detail: Option<String>,
    pub transport: String,
    pub tool_count: usize,
    pub disabled_tool_count: Option<usize>,
    pub custom_instructions: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogField {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub is_required: bool,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSkill {
    pub name: String,
    pub description: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPlugin {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub fields: Vec<CatalogField>,
    pub connector_count: usize,
    pub skills: Vec<CatalogSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePlugin {
    pub plugin_id: String,
    pub install_mode: Option<String>,
    pub is_enabled: bool,
    pub has_team_configured_variables: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxServerStatus {
    pub server_identifier: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub tool_count: usize,
}

pub fn to_installed_server(summary: &McpServerSummary) -> Value {
    let mut value = json!({
        "id": summary.id,
        "name": summary.name,
        "serverIdentifier": summary.server_identifier,
        "accountKey": summary.account_key,
        "isTeamServer": summary.is_team_server,
        "status": summary.status,
        "transport": summary.transport,
        "toolCount": summary.tool_count,
        "customInstructions": summary.custom_instructions,
    });
    let object = value.as_object_mut().expect("object");
    if let Some(plugin_id) = &summary.plugin_id {
        object.insert("pluginId".into(), json!(plugin_id));
    }
    if let Some(detail) = &summary.status_detail {
        object.insert("statusDetail".into(), json!(detail));
    }
    if let Some(count) = summary.disabled_tool_count {
        object.insert("disabledToolCount".into(), json!(count));
    }
    value
}

pub fn to_installed_servers(servers: &[McpServerSummary]) -> Vec<Value> {
    servers.iter().map(to_installed_server).collect()
}

pub fn to_auth_result(
    status: &str,
    server_name: &str,
    authorization_url: Option<&str>,
    message: Option<&str>,
) -> Value {
    match status {
        "started" => json!({
            "kind": "started",
            "authorizationUrl": authorization_url,
            "serverName": server_name,
        }),
        "already-authenticated" | "not-configured" => json!({
            "kind": status,
            "serverName": server_name,
        }),
        other => json!({
            "kind": other,
            "message": message,
            "serverName": server_name,
        }),
    }
}

pub fn is_effective_plugin_installed(plugin: &EffectivePlugin) -> bool {
    plugin.is_enabled
}

pub fn to_plugin_summary(
    view: &CatalogPlugin,
    effective_plugins: Option<&[EffectivePlugin]>,
    servers: &[McpServerSummary],
) -> Value {
    let record = effective_plugins
        .and_then(|plugins| plugins.iter().find(|plugin| plugin.plugin_id == view.id));
    let effective = record.filter(|plugin| is_effective_plugin_installed(plugin));
    let attributed = servers
        .iter()
        .find(|server| server.plugin_id.as_deref() == Some(&view.id));
    let installed = effective.is_some() || (record.is_none() && attributed.is_some());
    json!({
        "pluginId": view.id,
        "name": view.name,
        "displayName": view.display_name,
        "description": view.description,
        "category": view.category,
        "isInstalled": installed,
        "installMode": if installed {
            effective.and_then(|plugin| plugin.install_mode.clone()).unwrap_or_else(|| "unknown".into())
        } else {
            String::new()
        },
        "connectorCount": view.connector_count,
        "skills": view.skills.iter().map(|skill| json!({
            "name": skill.name,
            "description": skill.description,
        })).collect::<Vec<_>>(),
    })
}

pub trait PluginSkillsPort: Send + Sync {
    fn sync(&self, trigger: &str) -> Result<Vec<Value>, String>;
    fn status(&self) -> Value;
    fn remove_live_references(&self, source_urls: &[String]);
}

pub trait McpManagerBackend: Send + Sync {
    fn list_servers(&self) -> Result<Vec<McpServerSummary>, String>;
    fn list_catalog(&self, force_refresh: bool) -> Result<Vec<CatalogPlugin>, String>;
    fn list_effective_plugins(&self) -> Result<Vec<EffectivePlugin>, String>;
    fn uninstall_plugin(&self, plugin_id: &str) -> Result<(bool, Option<String>), String>;
    fn install_plugin(
        &self,
        plugin_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<Vec<McpServerSummary>, String>;
    fn add_server(&self, name: &str, config_json: &str) -> Result<Vec<McpServerSummary>, String>;
    fn remove_server(&self, server_id: &str) -> Result<(bool, Option<String>, Vec<McpServerSummary>), String>;
    fn restart_servers(&self) -> Result<Vec<McpServerSummary>, String>;
    fn set_server_custom_instructions(&self, server_id: &str, instructions: &str) -> Result<Vec<McpServerSummary>, String>;
    fn authenticate_server(
        &self,
        server_id: &str,
        account_key: &str,
        requesting_agent_id: Option<&str>,
        force_reauth: bool,
    ) -> Result<Value, String>;
    fn logout_account(&self, server_id: &str, account_key: &str) -> Result<Vec<McpServerSummary>, String>;
    fn remove_account(&self, server_id: &str, account_key: &str) -> Result<Vec<McpServerSummary>, String>;
    fn rename_account(&self, server_id: &str, account_key: &str, new_account_key: &str) -> Result<Vec<McpServerSummary>, String>;
    fn list_box_servers(
        &self,
        ids: &[String],
        kick_only: bool,
    ) -> Result<Vec<BoxServerStatus>, String>;
    fn note_auth_completed_elsewhere(&self, server_id: &str, account_key: &str);
    fn dispose(&self) -> Result<(), String>;
}

type AuthListener = Arc<dyn Fn(Value) + Send + Sync>;
type ServersListener = Arc<dyn Fn(Vec<BoxServerStatus>) + Send + Sync>;

#[derive(Default)]
struct ServiceState {
    disposed: bool,
    next_listener_id: u64,
    auth_listeners: HashMap<u64, AuthListener>,
    server_listeners: HashMap<u64, ServersListener>,
    status_followups: HashSet<String>,
}

pub struct McpHostService {
    backend: Arc<dyn McpManagerBackend>,
    plugin_skills: Option<Arc<dyn PluginSkillsPort>>,
    state: Arc<Mutex<ServiceState>>,
}

impl McpHostService {
    pub fn new(
        backend: Arc<dyn McpManagerBackend>,
        plugin_skills: Option<Arc<dyn PluginSkillsPort>>,
    ) -> Self {
        Self {
            backend,
            plugin_skills,
            state: Arc::new(Mutex::new(ServiceState::default())),
        }
    }

    pub fn list_installed(&self) -> Result<Vec<Value>, String> {
        self.backend
            .list_servers()
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn transport_for_server_identifier(
        &self,
        server_identifier: &str,
    ) -> Result<String, String> {
        let transport = self
            .backend
            .list_servers()?
            .into_iter()
            .find(|server| server.server_identifier == server_identifier)
            .map(|server| server.transport)
            .filter(|transport| !transport.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        Ok(transport)
    }

    pub fn list_plugins(&self) -> Result<Vec<Value>, String> {
        let views = self.backend.list_catalog(false)?;
        let servers = self.backend.list_servers()?;
        let effective = self.backend.list_effective_plugins().ok();
        Ok(views
            .iter()
            .map(|view| to_plugin_summary(view, effective.as_deref(), &servers))
            .collect())
    }

    pub fn get_plugin(&self, plugin_id: &str) -> Result<Option<Value>, String> {
        let mut views = self.backend.list_catalog(false)?;
        let mut view = views.iter().find(|entry| entry.id == plugin_id).cloned();
        if view.is_none() {
            views = self.backend.list_catalog(true)?;
            view = views.iter().find(|entry| entry.id == plugin_id).cloned();
        }
        let Some(view) = view else {
            return Ok(None);
        };
        let servers = self.backend.list_servers()?;
        let effective = self.backend.list_effective_plugins().ok();
        let mut summary = to_plugin_summary(&view, effective.as_deref(), &servers);
        let object = summary.as_object_mut().expect("plugin summary object");
        object.insert(
            "fields".into(),
            json!(
                view.fields
                    .iter()
                    .map(|field| json!({
                        "key": field.key,
                        "label": field.label,
                        "hint": field.hint,
                        "isRequired": field.is_required,
                        "isSecret": field.is_secret,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        object.insert(
            "servers".into(),
            json!(
                servers
                    .iter()
                    .filter(|server| server.plugin_id.as_deref() == Some(plugin_id))
                    .map(to_installed_server)
                    .collect::<Vec<_>>()
            ),
        );
        Ok(Some(summary))
    }

    pub fn uninstall_plugin(&self, plugin_id: &str) -> Result<Value, String> {
        let source_urls = self
            .backend
            .list_catalog(false)
            .ok()
            .and_then(|catalog| catalog.into_iter().find(|plugin| plugin.id == plugin_id))
            .map(|plugin| {
                plugin
                    .skills
                    .into_iter()
                    .filter_map(|skill| skill.source_url)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let (removed, reason) = self.backend.uninstall_plugin(plugin_id)?;
        if removed {
            if let Some(skills) = &self.plugin_skills {
                skills.remove_live_references(&source_urls);
                let _ = skills.sync("uninstall");
            }
        }
        Ok(json!({
            "removed": removed,
            "reason": reason,
        }))
    }

    pub fn install_plugin(
        &self,
        plugin_id: &str,
        values: &HashMap<String, String>,
    ) -> Result<Vec<Value>, String> {
        let servers = self.backend.install_plugin(plugin_id, values)?;
        if let Some(skills) = &self.plugin_skills {
            if let Ok(catalog) = self.backend.list_catalog(false) {
                let urls = catalog
                    .into_iter()
                    .find(|plugin| plugin.id == plugin_id)
                    .map(|plugin| {
                        plugin
                            .skills
                            .into_iter()
                            .filter_map(|skill| skill.source_url)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                skills.remove_live_references(&urls);
            }
            let _ = skills.sync("install");
        }
        Ok(to_installed_servers(&servers))
    }

    pub fn add_server(&self, name: &str, config_json: &str) -> Result<Vec<Value>, String> {
        self.backend
            .add_server(name, config_json)
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn remove_server(&self, server_id: &str) -> Result<Value, String> {
        let (removed, reason, servers) = self.backend.remove_server(server_id)?;
        Ok(json!({
            "removed": removed,
            "reason": reason,
            "servers": to_installed_servers(&servers),
        }))
    }

    pub fn restart_servers(&self) -> Result<Vec<Value>, String> {
        self.backend
            .restart_servers()
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn set_server_custom_instructions(
        &self,
        server_id: &str,
        instructions: &str,
    ) -> Result<Vec<Value>, String> {
        self.backend
            .set_server_custom_instructions(server_id, instructions)
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn authenticate_server(
        &self,
        server_id: &str,
        account_key: &str,
        requesting_agent_id: Option<&str>,
        force_reauth: bool,
    ) -> Result<Value, String> {
        let result = self.backend.authenticate_server(
            server_id,
            account_key,
            requesting_agent_id,
            force_reauth,
        )?;
        let status = result
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| "MCP authentication result requires status".to_string())?;
        let server_name = result
            .get("serverName")
            .and_then(Value::as_str)
            .ok_or_else(|| "MCP authentication result requires serverName".to_string())?;
        Ok(to_auth_result(
            status,
            server_name,
            result.get("authorizationUrl").and_then(Value::as_str),
            result.get("message").and_then(Value::as_str),
        ))
    }

    pub fn logout_account(&self, server_id: &str, account_key: &str) -> Result<Vec<Value>, String> {
        self.backend
            .logout_account(server_id, account_key)
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn remove_account(&self, server_id: &str, account_key: &str) -> Result<Vec<Value>, String> {
        self.backend
            .remove_account(server_id, account_key)
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn rename_account(
        &self,
        server_id: &str,
        account_key: &str,
        new_account_key: &str,
    ) -> Result<Vec<Value>, String> {
        self.backend
            .rename_account(server_id, account_key, new_account_key)
            .map(|servers| to_installed_servers(&servers))
    }

    pub fn sync_plugin_skills(&self) -> Result<Vec<Value>, String> {
        self.plugin_skills
            .as_ref()
            .map(|skills| skills.sync("desktop"))
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    pub fn plugin_sync_status(&self) -> Value {
        self.plugin_skills
            .as_ref()
            .map(|skills| skills.status())
            .unwrap_or_else(|| json!({"authBlocked":[]}))
    }

    pub fn list_box_servers(&self, ids: &[String]) -> Result<Vec<BoxServerStatus>, String> {
        let servers = self.backend.list_box_servers(ids, true)?;
        self.schedule_status_followups(&servers);
        Ok(servers)
    }

    pub fn run_status_followup(&self, server_id: &str) -> Result<(), String> {
        let should_run = {
            let state = self.state.lock().expect("mcp service state poisoned");
            !state.disposed && state.status_followups.contains(server_id)
        };
        if !should_run {
            return Ok(());
        }
        let result = self
            .backend
            .list_box_servers(&[server_id.to_string()], false);
        let listeners = {
            let mut state = self.state.lock().expect("mcp service state poisoned");
            state.status_followups.remove(server_id);
            if state.disposed {
                Vec::new()
            } else {
                state.server_listeners.values().cloned().collect::<Vec<_>>()
            }
        };
        let servers = result?;
        if let Some(updated) = servers.first().cloned() {
            for listener in listeners {
                listener(vec![updated.clone()]);
            }
        }
        Ok(())
    }

    fn schedule_status_followups(&self, servers: &[BoxServerStatus]) {
        let mut state = self.state.lock().expect("mcp service state poisoned");
        if state.disposed {
            return;
        }
        for server in servers {
            if matches!(server.status.as_str(), "loading" | "error") {
                state
                    .status_followups
                    .insert(server.server_identifier.clone());
            }
        }
    }

    pub fn subscribe_to_auth_completion(&self, listener: AuthListener) -> Box<dyn FnOnce() + Send> {
        let id = {
            let mut state = self.state.lock().expect("mcp service state poisoned");
            let id = state.next_listener_id;
            state.next_listener_id += 1;
            state.auth_listeners.insert(id, listener);
            id
        };
        let listener_id = Arc::new(Mutex::new(Some(id)));
        let service_state = Arc::clone(&self.state);
        Box::new(move || {
            if let Some(id) = listener_id
                .lock()
                .expect("unsubscribe state poisoned")
                .take()
            {
                service_state
                    .lock()
                    .expect("mcp service state poisoned")
                    .auth_listeners
                    .remove(&id);
            }
        })
    }

    pub fn emit_auth_completion(&self, event: Value) {
        let listeners = {
            let state = self.state.lock().expect("mcp service state poisoned");
            if state.disposed {
                return;
            }
            state.auth_listeners.values().cloned().collect::<Vec<_>>()
        };
        for listener in listeners {
            listener(event.clone());
        }
    }

    pub fn subscribe_to_servers_updated(&self, listener: ServersListener) -> u64 {
        let mut state = self.state.lock().expect("mcp service state poisoned");
        let id = state.next_listener_id;
        state.next_listener_id += 1;
        state.server_listeners.insert(id, listener);
        id
    }

    pub fn unsubscribe_servers_updated(&self, id: u64) {
        self.state
            .lock()
            .expect("mcp service state poisoned")
            .server_listeners
            .remove(&id);
    }

    pub fn note_auth_completed_elsewhere(&self, server_id: &str, account_key: &str) {
        self.backend
            .note_auth_completed_elsewhere(server_id, account_key);
    }

    pub fn dispose(&self) -> Result<(), String> {
        {
            let mut state = self.state.lock().expect("mcp service state poisoned");
            if state.disposed {
                return Ok(());
            }
            state.disposed = true;
            state.auth_listeners.clear();
            state.server_listeners.clear();
            state.status_followups.clear();
        }
        self.backend.dispose()
    }
}
