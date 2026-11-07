use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use crate::runner::shell_terminal_watch::ShellTerminalPollRead;
use crate::sand_activity::{SAND_EXTERNAL_READ_TOOL_NAME, SAND_EXTERNAL_SHELL_TOOL_NAME};

pub const EXTERNAL_MACHINE_TOOL_PROVIDER: &str = "mahayana-local-exec";
pub const EXTERNAL_SHELL_STATUS_TOOL_NAME: &str = "ExternalShellStatus";
pub const EXTERNAL_SHELL_HISTORY_TOOL_NAME: &str = "ExternalShellHistory";
pub const EXTERNAL_SHELL_CANCEL_TOOL_NAME: &str = "ExternalShellCancel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalMachineShellArgs {
    pub command: String,
    pub working_directory: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalMachineReadArgs {
    pub path: String,
    pub offset: Option<i32>,
    pub limit: Option<u32>,
    pub encoding_hint: Option<String>,
    pub tool_call_id: String,
}

pub trait ExternalMachineExecutor: Send + Sync {
    fn execute_shell(&self, args: &ExternalMachineShellArgs) -> Result<Value, ProviderSessionError>;
    fn execute_read(&self, args: &ExternalMachineReadArgs) -> Result<Value, ProviderSessionError>;

    fn operation_status(&self, _operation_id: &str) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "External shell operation status is unavailable".into(),
        ))
    }

    fn operation_history(&self) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "External shell operation history is unavailable".into(),
        ))
    }

    fn cancel_operation(&self, _operation_id: &str) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "External shell cancellation is unavailable".into(),
        ))
    }

    fn poll_background_shell_terminal(
        &self,
        _shell_id: &str,
        _tool_call_id: &str,
    ) -> ShellTerminalPollRead {
        ShellTerminalPollRead::TransientFailure(
            "External background terminal polling is unavailable".into(),
        )
    }
}

pub type ExternalShellAutoReviewCallback = Arc<
    dyn Fn(&ExternalMachineShellArgs) -> Result<Option<String>, ProviderSessionError>
        + Send + Sync + 'static,
>;

#[derive(Clone)]
pub struct ExternalMachineToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    executor: Arc<dyn ExternalMachineExecutor>,
    shell_review: Option<ExternalShellAutoReviewCallback>,
}

impl ExternalMachineToolBridge {
    pub fn new(delegate: Arc<dyn RoutedToolBridge>, executor: Arc<dyn ExternalMachineExecutor>) -> Self {
        Self { delegate, executor, shell_review: None }
    }

    pub fn with_shell_review(mut self, review: ExternalShellAutoReviewCallback) -> Self {
        self.shell_review = Some(review);
        self
    }
}

fn definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: SAND_EXTERNAL_SHELL_TOOL_NAME.into(),
            provider_identifier: EXTERNAL_MACHINE_TOOL_PROVIDER.into(),
            tool_name: SAND_EXTERNAL_SHELL_TOOL_NAME.into(),
            description: Some("Run a shell command on the user's connected computer. The operation has a stable operationId/requestId; if delivery or the result is lost and outcomeUnknown is true, it is never automatically retried after a lost result. Query ExternalShellStatus or ExternalShellHistory instead of blindly replaying the command.".into()),
            input_schema: json!({
                "type":"object","additionalProperties":false,"required":["command"],
                "properties":{"command":{"type":"string","minLength":1},"workingDirectory":{"type":"string"}}
            }),
        },
        RoutedToolDefinition {
            name: SAND_EXTERNAL_READ_TOOL_NAME.into(),
            provider_identifier: EXTERNAL_MACHINE_TOOL_PROVIDER.into(),
            tool_name: SAND_EXTERNAL_READ_TOOL_NAME.into(),
            description: Some("Read a file from the user's connected computer.".into()),
            input_schema: json!({
                "type":"object","additionalProperties":false,"required":["path"],
                "properties":{
                    "path":{"type":"string","minLength":1},"offset":{"type":"integer"},
                    "limit":{"type":"integer","minimum":0},"encodingHint":{"type":"string"}
                }
            }),
        },
        RoutedToolDefinition {
            name: EXTERNAL_SHELL_STATUS_TOOL_NAME.into(),
            provider_identifier: EXTERNAL_MACHINE_TOOL_PROVIDER.into(),
            tool_name: EXTERNAL_SHELL_STATUS_TOOL_NAME.into(),
            description: Some("Query one shell operation started by this conversation without re-running it.".into()),
            input_schema: json!({
                "type":"object","additionalProperties":false,"required":["operationId"],
                "properties":{"operationId":{"type":"string","minLength":1}}
            }),
        },
        RoutedToolDefinition {
            name: EXTERNAL_SHELL_HISTORY_TOOL_NAME.into(),
            provider_identifier: EXTERNAL_MACHINE_TOOL_PROVIDER.into(),
            tool_name: EXTERNAL_SHELL_HISTORY_TOOL_NAME.into(),
            description: Some("List the bounded shell operation history owned by this conversation.".into()),
            input_schema: json!({"type":"object","additionalProperties":false}),
        },
        RoutedToolDefinition {
            name: EXTERNAL_SHELL_CANCEL_TOOL_NAME.into(),
            provider_identifier: EXTERNAL_MACHINE_TOOL_PROVIDER.into(),
            tool_name: EXTERNAL_SHELL_CANCEL_TOOL_NAME.into(),
            description: Some("Request cancellation of one active shell operation owned by this conversation.".into()),
            input_schema: json!({
                "type":"object","additionalProperties":false,"required":["operationId"],
                "properties":{"operationId":{"type":"string","minLength":1}}
            }),
        },
    ]
}

impl RoutedToolBridge for ExternalMachineToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        let mut names = tools.iter().map(|tool| tool.name.clone()).collect::<BTreeSet<_>>();
        for tool in definitions() {
            if !names.insert(tool.name.clone()) {
                return Err(ProviderSessionError::Tool(format!(
                    "Host tool name collides with external-machine tool {}", tool.name
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
        match tool.name.as_str() {
            SAND_EXTERNAL_SHELL_TOOL_NAME => {
                let request = ExternalMachineShellArgs {
                    command: required_string(&args, "command", SAND_EXTERNAL_SHELL_TOOL_NAME)?,
                    working_directory: optional_string(&args, "workingDirectory").unwrap_or_default(),
                    tool_call_id: tool_call_id.to_string(),
                };
                if let Some(review) = self.shell_review.as_ref() {
                    if let Some(reason) = review(&request)? {
                        return Ok(Value::String(reason));
                    }
                }
                self.executor.execute_shell(&request)
            }
            SAND_EXTERNAL_READ_TOOL_NAME => self.executor.execute_read(&ExternalMachineReadArgs {
                path: required_string(&args, "path", SAND_EXTERNAL_READ_TOOL_NAME)?,
                offset: optional_i32(&args, "offset")?,
                limit: optional_u32(&args, "limit")?,
                encoding_hint: optional_string(&args, "encodingHint"),
                tool_call_id: tool_call_id.to_string(),
            }),
            EXTERNAL_SHELL_STATUS_TOOL_NAME => self.executor.operation_status(&required_string(
                &args, "operationId", EXTERNAL_SHELL_STATUS_TOOL_NAME,
            )?),
            EXTERNAL_SHELL_HISTORY_TOOL_NAME => self.executor.operation_history(),
            EXTERNAL_SHELL_CANCEL_TOOL_NAME => self.executor.cancel_operation(&required_string(
                &args, "operationId", EXTERNAL_SHELL_CANCEL_TOOL_NAME,
            )?),
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}

fn required_string(args: &Value, key: &str, tool: &str) -> Result<String, ProviderSessionError> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderSessionError::Tool(format!("{tool} requires {key}")))
}

fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn optional_i32(args: &Value, key: &str) -> Result<Option<i32>, ProviderSessionError> {
    let Some(value) = args.get(key) else { return Ok(None); };
    let raw = value.as_i64().ok_or_else(|| ProviderSessionError::Tool(format!("{key} must be an integer")))?;
    i32::try_from(raw).map(Some).map_err(|_| ProviderSessionError::Tool(format!("{key} is out of range")))
}

fn optional_u32(args: &Value, key: &str) -> Result<Option<u32>, ProviderSessionError> {
    let Some(value) = args.get(key) else { return Ok(None); };
    let raw = value.as_u64().ok_or_else(|| ProviderSessionError::Tool(format!("{key} must be a non-negative integer")))?;
    u32::try_from(raw).map(Some).map_err(|_| ProviderSessionError::Tool(format!("{key} is out of range")))
}
