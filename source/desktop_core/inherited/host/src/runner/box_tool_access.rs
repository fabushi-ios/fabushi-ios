use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};

use super::large_output_spill::{MCP_TEXT_FILE_THRESHOLD_BYTES, is_large_output_spill_enabled, maybe_spill_mcp_text_result};
use super::shell_terminal_watch::ShellTerminalPollRead;
use super::routed_provider_runtime::RoutedToolBridge;

pub const RUNNER_BOX_TOOL_PROVIDER: &str = "mahayana-box";
pub const RUNNER_BOX_SHELL_TOOL_NAME: &str = "Shell";
pub const RUNNER_BOX_READ_TOOL_NAME: &str = "Read";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerBoxShellRequest {
    pub command: String,
    pub working_directory: String,
    pub tool_call_id: String,
    pub is_background: bool,
    pub block_until_ms: Option<u64>,
}

impl RunnerBoxShellRequest {
    pub fn should_start_in_background(&self) -> bool {
        self.is_background || self.block_until_ms == Some(0)
    }
}

pub type BoxShellAutoReviewCallback = Arc<
    dyn Fn(&RunnerBoxShellRequest) -> Result<Option<String>, ProviderSessionError>
        + Send
        + Sync
        + 'static,
>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerBoxReadRequest {
    pub path: String,
    pub tool_call_id: String,
    pub offset: Option<i32>,
    pub limit: Option<u32>,
    pub encoding_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerBoxWriteRequest {
    pub path: String,
    pub data: Vec<u8>,
    pub tool_call_id: String,
}

/// Host-owned Box resource port consumed by the independent Runner.
///
/// Runner owns the model-facing tool names and schemas. Host supplies only
/// concrete resource execution; this prevents ForeverBox lifecycle concerns
/// from moving into the Runner process boundary.
pub trait RunnerBoxResourcePort: Send + Sync {
    fn execute_shell(
        &self,
        request: RunnerBoxShellRequest,
    ) -> Result<Value, ProviderSessionError>;

    fn execute_read(
        &self,
        request: RunnerBoxReadRequest,
    ) -> Result<Value, ProviderSessionError>;

    fn execute_write(
        &self,
        request: RunnerBoxWriteRequest,
    ) -> Result<(), ProviderSessionError>;

    fn poll_background_shell_terminal(&self, _shell_id: &str) -> ShellTerminalPollRead {
        ShellTerminalPollRead::TransientFailure(
            "Box background terminal polling is unavailable for this agent".into(),
        )
    }

    /// Execute the generated Grok ComputerUse protobuf through the Host-owned
    /// box transport. Runner owns the tool contract; Host owns the remote
    /// resource and lifecycle.
    fn execute_computer_use_protobuf(
        &self,
        _protobuf_args: Vec<u8>,
    ) -> Result<Vec<u8>, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "Box computer-use resource is not available for this agent".into(),
        ))
    }

    /// Execute ComputerUse while preserving the first generated action case for
    /// the remote-resource audit lifecycle. Existing ports may ignore the hint.
    fn execute_computer_use_protobuf_with_action_case(
        &self,
        protobuf_args: Vec<u8>,
        _action_case: Option<&str>,
    ) -> Result<Vec<u8>, ProviderSessionError> {
        self.execute_computer_use_protobuf(protobuf_args)
    }

    /// Resolve the Host-owned browser window assigned to this agent.
    ///
    /// The default fails closed so non-desktop box ports do not accidentally
    /// expose a synthetic display identity.
    fn browser_window_index(&self) -> Result<u32, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "Box browser window is not available for this agent".into(),
        ))
    }
}

pub fn runner_box_tool_definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: RUNNER_BOX_SHELL_TOOL_NAME.into(),
            provider_identifier: RUNNER_BOX_TOOL_PROVIDER.into(),
            tool_name: RUNNER_BOX_SHELL_TOOL_NAME.into(),
            description: Some(
                "Run a shell command on your own box. This is the default shell surface for files and commands inside the isolated box."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["command"],
                "properties": {
                    "command": {"type": "string"},
                    "workingDirectory": {
                        "type": "string",
                        "description": "Working directory inside the box; defaults to /workspace."
                    },
                    "is_background": {
                        "type": "boolean",
                        "description": "Start the command in the background immediately."
                    },
                    "block_until_ms": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Wait this many milliseconds before backgrounding; 0 starts in the background immediately."
                    }
                }
            }),
        },
        RoutedToolDefinition {
            name: RUNNER_BOX_READ_TOOL_NAME.into(),
            provider_identifier: RUNNER_BOX_TOOL_PROVIDER.into(),
            tool_name: RUNNER_BOX_READ_TOOL_NAME.into(),
            description: Some(
                "Reads a file on your own computer (the box), the same filesystem Shell acts on. Text reads support offset/limit paging."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["path"],
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer"},
                    "limit": {"type": "integer", "minimum": 0},
                    "encodingHint": {"type": "string"}
                }
            }),
        },
    ]
}

#[derive(Clone)]
pub struct RunnerBoxToolBridge {
    upstream: Arc<dyn RoutedToolBridge>,
    box_resources: Arc<dyn RunnerBoxResourcePort>,
    shell_review: Option<BoxShellAutoReviewCallback>,
}

impl RunnerBoxToolBridge {
    pub fn new(
        upstream: Arc<dyn RoutedToolBridge>,
        box_resources: Arc<dyn RunnerBoxResourcePort>,
    ) -> Self {
        Self {
            upstream,
            box_resources,
            shell_review: None,
        }
    }

    pub fn with_shell_review(mut self, review: BoxShellAutoReviewCallback) -> Self {
        self.shell_review = Some(review);
        self
    }

    fn is_box_tool(tool: &RoutedToolDefinition) -> bool {
        tool.provider_identifier == RUNNER_BOX_TOOL_PROVIDER
            && matches!(
                tool.name.as_str(),
                RUNNER_BOX_SHELL_TOOL_NAME | RUNNER_BOX_READ_TOOL_NAME
            )
    }

    fn parse_shell_request(
        args: &Value,
        tool_call_id: &str,
    ) -> Result<RunnerBoxShellRequest, ProviderSessionError> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Tool(
                    "Shell requires a non-empty command".into(),
                )
            })?;
        let working_directory = args
            .get("workingDirectory")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("/workspace");
        let is_background = args
            .get("is_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let block_until_ms = match args.get("block_until_ms") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_u64().ok_or_else(|| {
                ProviderSessionError::Tool(
                    "Shell block_until_ms must be a non-negative integer".into(),
                )
            })?),
        };
        Ok(RunnerBoxShellRequest {
            command: command.into(),
            working_directory: working_directory.into(),
            tool_call_id: tool_call_id.into(),
            is_background,
            block_until_ms,
        })
    }

    fn parse_read_request(
        args: &Value,
        tool_call_id: &str,
    ) -> Result<RunnerBoxReadRequest, ProviderSessionError> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ProviderSessionError::Tool("Read requires a path".into()))?;
        let offset = match args.get("offset") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let raw = value.as_i64().ok_or_else(|| {
                    ProviderSessionError::Tool("Read offset must be an integer".into())
                })?;
                Some(i32::try_from(raw).map_err(|_| {
                    ProviderSessionError::Tool("Read offset is out of range".into())
                })?)
            }
        };
        let limit = match args.get("limit") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let raw = value.as_u64().ok_or_else(|| {
                    ProviderSessionError::Tool(
                        "Read limit must be a non-negative integer".into(),
                    )
                })?;
                Some(u32::try_from(raw).map_err(|_| {
                    ProviderSessionError::Tool("Read limit is out of range".into())
                })?)
            }
        };
        let encoding_hint = args
            .get("encodingHint")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        Ok(RunnerBoxReadRequest {
            path: path.into(),
            tool_call_id: tool_call_id.into(),
            offset,
            limit,
            encoding_hint,
        })
    }
}

impl RoutedToolBridge for RunnerBoxToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.upstream.list_tools()?;
        let mut names = tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        for tool in runner_box_tool_definitions() {
            if !names.insert(tool.name.clone()) {
                return Err(ProviderSessionError::Protocol(format!(
                    "Host tool name collides with Runner box tool {}",
                    tool.name
                )));
            }
            tools.push(tool);
        }
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if !Self::is_box_tool(tool) {
            let result = self.upstream.call_tool(tool, args, tool_call_id)?;
            return Ok(if is_large_output_spill_enabled() {
                maybe_spill_mcp_text_result(
                    self.box_resources.as_ref(),
                    result,
                    tool_call_id,
                    MCP_TEXT_FILE_THRESHOLD_BYTES,
                )
            } else {
                result
            });
        }
        match tool.name.as_str() {
            RUNNER_BOX_SHELL_TOOL_NAME => {
                let request = Self::parse_shell_request(&args, tool_call_id)?;
                if let Some(review) = self.shell_review.as_ref() {
                    if let Some(reason) = review(&request)? {
                        return Ok(Value::String(reason));
                    }
                }
                self.box_resources.execute_shell(request)
            }
            RUNNER_BOX_READ_TOOL_NAME => self
                .box_resources
                .execute_read(Self::parse_read_request(&args, tool_call_id)?),
            _ => Err(ProviderSessionError::Tool(format!(
                "Unsupported Runner box tool {}",
                tool.name
            ))),
        }
    }
}
