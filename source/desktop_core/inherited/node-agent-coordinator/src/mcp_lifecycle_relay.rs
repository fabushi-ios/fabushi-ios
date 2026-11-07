use crate::protocol::Failure;
use serde_json::{Value, json};

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

#[derive(Debug, Clone, PartialEq)]
pub struct HostMcpLifecycleRequest {
    pub request_id: String,
    pub method: String,
    pub args: Value,
}

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

pub fn parse_host_mcp_lifecycle_request(payload: &Value) -> Result<HostMcpLifecycleRequest, Failure> {
    let request_id = payload
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Failure::new("MCP_LIFECYCLE_RELAY_PROTOCOL_ERROR", "Host MCP lifecycle request requires requestId"))?;
    if request_id.len() > 256 {
        return Err(Failure::new("MCP_LIFECYCLE_RELAY_PROTOCOL_ERROR", "Host MCP lifecycle requestId exceeds 256 bytes"));
    }
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Failure::new("MCP_LIFECYCLE_RELAY_PROTOCOL_ERROR", "Host MCP lifecycle request requires method"))?;
    if !is_host_mcp_lifecycle_method(method) {
        return Err(Failure::new("MCP_LIFECYCLE_RELAY_METHOD_DENIED", format!("Host MCP lifecycle relay may not invoke {method}")));
    }
    let args = payload.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err(Failure::new("MCP_LIFECYCLE_RELAY_PROTOCOL_ERROR", "Host MCP lifecycle request args must be an object"));
    }
    Ok(HostMcpLifecycleRequest { request_id: request_id.to_string(), method: method.to_string(), args })
}

pub fn host_mcp_lifecycle_resolution_success(request_id: &str, result: Value) -> Value {
    json!({"requestId": request_id, "ok": true, "result": result})
}

pub fn host_mcp_lifecycle_resolution_failure(request_id: &str, message: impl Into<String>) -> Value {
    json!({"requestId": request_id, "ok": false, "error": message.into()})
}

pub fn execute_host_mcp_lifecycle_request<Execute>(
    payload: &Value,
    execute: Execute,
) -> Result<Value, Failure>
where
    Execute: FnOnce(&str, Value) -> Result<Value, Failure>,
{
    let request = parse_host_mcp_lifecycle_request(payload)?;
    let request_id = request.request_id.clone();
    Ok(match execute(&request.method, request.args) {
        Ok(result) => host_mcp_lifecycle_resolution_success(&request_id, result),
        Err(failure) => host_mcp_lifecycle_resolution_failure(
            &request_id,
            format!("{}: {}", failure.code, failure.message),
        ),
    })
}
