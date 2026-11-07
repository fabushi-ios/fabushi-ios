use serde_json::Value;

use crate::ports::mcp_state_executor::{
    McpStateExecResult, McpStateServer, McpStateSuccess, SandMcpToolProvider, execute_mcp_state,
};

#[derive(Debug, Clone, PartialEq)]
pub struct BoxMcpTool {
    pub name: String,
    pub provider_identifier: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub client_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxMcpServer {
    pub server_identifier: String,
    pub status: String,
    pub status_detail: Option<String>,
    pub tool_count: usize,
    pub tools: Vec<BoxMcpTool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxMcpToolCall {
    pub tool_call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BoxMcpToolResult {
    Success(Value),
    Error { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SandBoxMcpExecError(pub String);

pub trait BoxMcpExecBackend: Send + Sync {
    fn load_servers(&self, config_json: &str) -> Result<(), String>;
    fn state_provider(&self) -> Result<&dyn SandMcpToolProvider, String>;
    fn execute_tool(&self, args: &BoxMcpToolCall) -> Result<Value, String>;
}

pub struct SandBoxMcpExec<B: BoxMcpExecBackend> {
    backend: B,
}

impl<B: BoxMcpExecBackend> SandBoxMcpExec<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn load_servers(&self, config_json: &str) -> Result<(), SandBoxMcpExecError> {
        self.backend
            .load_servers(config_json)
            .map_err(SandBoxMcpExecError)
    }

    pub fn list_tools(
        &self,
        server_identifiers: &[String],
        _kick_only: bool,
    ) -> Result<Vec<BoxMcpServer>, SandBoxMcpExecError> {
        let provider = self.backend.state_provider().map_err(SandBoxMcpExecError)?;
        let state = execute_mcp_state(provider).map_err(|error| {
            SandBoxMcpExecError(format!("Box MCP tool discovery failed: {error}"))
        })?;
        let McpStateExecResult::Success(McpStateSuccess { servers }) = state;
        let requested = server_identifiers
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        Ok(servers
            .into_iter()
            .filter(|server| requested.is_empty() || requested.contains(&server.server_identifier))
            .map(map_server)
            .collect())
    }

    pub fn execute_tool(&self, args: &BoxMcpToolCall) -> BoxMcpToolResult {
        match self.backend.execute_tool(args) {
            Ok(value) => BoxMcpToolResult::Success(value),
            Err(error) => BoxMcpToolResult::Error {
                error: format!(
                    "Box MCP execution failed for {:?}: {}",
                    args.name,
                    error_label(&error)
                ),
            },
        }
    }
}

fn map_server(server: McpStateServer) -> BoxMcpServer {
    let server_identifier = server.server_identifier.clone();
    let tools = server
        .tools
        .into_iter()
        .map(|tool| BoxMcpTool {
            name: tool.name,
            provider_identifier: tool.provider_identifier,
            tool_name: tool.tool_name,
            description: tool.description,
            input_schema: tool.input_schema,
            client_key: server_identifier.clone(),
        })
        .collect::<Vec<_>>();
    BoxMcpServer {
        server_identifier,
        status: if server.status.is_empty() {
            "connected".into()
        } else {
            server.status
        },
        status_detail: server
            .error_message
            .filter(|message| !message.trim().is_empty()),
        tool_count: tools.len(),
        tools,
    }
}

pub fn error_label(error: &str) -> String {
    if error.is_empty() {
        "unknown error".into()
    } else {
        error.to_string()
    }
}
