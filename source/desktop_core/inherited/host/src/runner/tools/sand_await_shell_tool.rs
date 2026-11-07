use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use crate::runner::background_work::parse_shell_terminal_footer;
use crate::runner::box_tool_access::RunnerBoxResourcePort;
use crate::runner::routed_provider_runtime::{RoutedProviderCancellation, RoutedToolBridge};
use crate::runner::shell_terminal_watch::{
    ShellTerminalPollRead, read_shell_terminal_snapshot,
};
use crate::sand_activity::{SAND_BOX_AWAIT_SHELL_TOOL_NAME, SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME};

use super::sand_external_machine_tools::ExternalMachineExecutor;

const DEFAULT_BLOCK_UNTIL_MS: u64 = 30_000;
const SHELL_CHECK_SLICE_MS: u64 = 250;
const AWAIT_PROVIDER: &str = "mahayana-await";

#[derive(Clone)]
pub struct SandAwaitShellToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    box_resources: Option<Arc<dyn RunnerBoxResourcePort>>,
    external_machine: Option<Arc<dyn ExternalMachineExecutor>>,
    cancellation: RoutedProviderCancellation,
}

impl SandAwaitShellToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        box_resources: Option<Arc<dyn RunnerBoxResourcePort>>,
        external_machine: Option<Arc<dyn ExternalMachineExecutor>>,
        cancellation: RoutedProviderCancellation,
    ) -> Self {
        Self {
            delegate,
            box_resources,
            external_machine,
            cancellation,
        }
    }

    fn definitions(&self) -> Vec<RoutedToolDefinition> {
        let mut tools = Vec::new();
        if self.box_resources.is_some() {
            tools.push(definition(
                SAND_BOX_AWAIT_SHELL_TOOL_NAME,
                "Check or poll a background shell running in your own box.",
            ));
        }
        if self.external_machine.is_some() {
            tools.push(definition(
                SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME,
                "Check or poll a background shell running on the user's connected computer.",
            ));
        }
        tools
    }

    fn call_await(
        &self,
        external: bool,
        args: &Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let shell_id = normalize_shell_id(args)?;
        let block_until_ms = parse_block_until_ms(args)?;
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let matcher = pattern
            .map(|value| Regex::new(&format!("(?m){value}")))
            .transpose()
            .map_err(|error| {
                ProviderSessionError::Tool(format!("Invalid AwaitShell pattern: {error}"))
            })?;
        let started = Instant::now();

        if shell_id.is_empty() {
            if block_until_ms == 0 {
                return Err(ProviderSessionError::Tool(
                    "Must pass a shell id or wait for a nonzero duration.".into(),
                ));
            }
            sleep_cancellable(&self.cancellation, block_until_ms)?;
            return Ok(json!({
                "status": "complete",
                "taskId": "",
                "runtimeMs": started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                "outputFilePath": "",
                "outputLength": 0,
                "text": "Slept briefly."
            }));
        }
        if !shell_id.bytes().all(|value| value.is_ascii_digit()) {
            return Err(ProviderSessionError::Tool(
                "AwaitShell requires a numeric shell id; subagent awaiting is not supported on this shell-only surface.".into(),
            ));
        }

        let deadline = Instant::now() + Duration::from_millis(block_until_ms);
        loop {
            self.cancellation.check()?;
            let read = if external {
                self.external_machine
                    .as_ref()
                    .ok_or_else(|| {
                        ProviderSessionError::Tool("ExternalAwaitShell is unavailable".into())
                    })?
                    .poll_background_shell_terminal(&shell_id, tool_call_id)
            } else {
                self.box_resources
                    .as_ref()
                    .ok_or_else(|| ProviderSessionError::Tool("AwaitShell is unavailable".into()))?
                    .poll_background_shell_terminal(&shell_id)
            };
            let (output_path, snapshot) = match read {
                ShellTerminalPollRead::Snapshot {
                    output_path,
                    result,
                } => {
                    let snapshot = read_shell_terminal_snapshot(result)
                        .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
                    if !snapshot.exists {
                        return Err(ProviderSessionError::Tool(format!(
                            "No shell found for id {shell_id}"
                        )));
                    }
                    (output_path, snapshot)
                }
                ShellTerminalPollRead::PermissionDenied { output_path } => {
                    return Err(ProviderSessionError::Tool(match output_path {
                        Some(path) => format!("Permission denied reading {path}"),
                        None => format!("Permission denied reading shell {shell_id}"),
                    }));
                }
                ShellTerminalPollRead::TransientFailure(error) => {
                    return Err(ProviderSessionError::Tool(error));
                }
            };
            let footer = parse_shell_terminal_footer(&snapshot.content);
            let body = terminal_body(&snapshot.content);
            let regex_match = matcher.as_ref().and_then(|matcher| {
                matcher
                    .find(body)
                    .map(|value| value.as_str().chars().take(500).collect::<String>())
            });
            let output_length = snapshot.content.len();
            let runtime_ms = terminal_runtime_ms(&snapshot.content)
                .unwrap_or_else(|| started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            if footer.is_complete {
                let exit_label = footer
                    .exit_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".into());
                let text = format!(
                    "Task completed in {runtime_ms}ms with exit code: {exit_label}.\noutput_file_path: {output_path}\noutput_length: {output_length}"
                );
                return Ok(json!({
                    "status": "complete",
                    "taskId": shell_id,
                    "runtimeMs": runtime_ms,
                    "outputFilePath": output_path,
                    "outputLength": output_length,
                    "regexRequested": pattern.is_some(),
                    "regexMatch": regex_match,
                    "exitCode": footer.exit_code,
                    "text": text
                }));
            }
            if regex_match.is_some() || block_until_ms == 0 || Instant::now() >= deadline {
                let text = format!(
                    "Task still running after {runtime_ms}ms...\noutput_file_path: {output_path}\noutput_length: {output_length}"
                );
                return Ok(json!({
                    "status": "stillRunning",
                    "taskId": shell_id,
                    "runtimeMs": runtime_ms,
                    "outputFilePath": output_path,
                    "outputLength": output_length,
                    "regexRequested": pattern.is_some(),
                    "regexMatch": regex_match,
                    "text": text
                }));
            }
            sleep_cancellable(
                &self.cancellation,
                SHELL_CHECK_SLICE_MS.min(block_until_ms.max(1)),
            )?;
        }
    }
}

impl RoutedToolBridge for SandAwaitShellToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        let own = self.definitions();
        tools.retain(|tool| !own.iter().any(|candidate| candidate.name == tool.name));
        tools.extend(own);
        Ok(tools)
    }

    fn observe_partial_tool_call(
        &self,
        partial: &crate::extensions::inference::provider_session::ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        match tool.tool_name.as_str() {
            SAND_BOX_AWAIT_SHELL_TOOL_NAME => self.call_await(false, &args, tool_call_id),
            SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME => self.call_await(true, &args, tool_call_id),
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}

fn definition(name: &str, description: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: AWAIT_PROVIDER.into(),
        tool_name: name.into(),
        description: Some(description.into()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "shell_id": {"type": ["string", "number"]},
                "task_id": {"type": ["string", "number"]},
                "block_until_ms": {"type": "number", "minimum": 0},
                "pattern": {"type": "string"}
            }
        }),
    }
}

fn normalize_shell_id(args: &Value) -> Result<String, ProviderSessionError> {
    let value = args.get("shell_id").or_else(|| args.get("task_id"));
    match value {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.eq_ignore_ascii_case("none") {
                Ok(String::new())
            } else {
                Ok(value.to_string())
            }
        }
        Some(Value::Number(value)) => Ok(value.to_string()),
        Some(_) => Err(ProviderSessionError::Tool(
            "AwaitShell shell_id must be a string or number".into(),
        )),
    }
}

fn parse_block_until_ms(args: &Value) -> Result<u64, ProviderSessionError> {
    let Some(value) = args.get("block_until_ms") else {
        return Ok(DEFAULT_BLOCK_UNTIL_MS);
    };
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value
        .as_str()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
    {
        return Ok(value);
    }
    Err(ProviderSessionError::Tool(
        "AwaitShell block_until_ms must be a non-negative integer".into(),
    ))
}

fn terminal_runtime_ms(content: &str) -> Option<u64> {
    let footer_start = content.rfind("\n---\n");
    if let Some(start) = footer_start {
        for line in content[start + 5..].lines() {
            if let Some(raw) = line.strip_prefix("elapsed_ms:") {
                if let Ok(value) = raw.trim().parse::<u64>() {
                    return Some(value);
                }
            }
        }
    }
    let header = content.strip_prefix("---\n")?;
    let end = header.find("\n---\n")?;
    for line in header[..end].lines() {
        if let Some(raw) = line.strip_prefix("running_for_ms:") {
            if let Ok(value) = raw.trim().parse::<u64>() {
                return Some(value);
            }
        }
    }
    None
}

fn terminal_body(content: &str) -> &str {
    let mut body = content;
    if let Some(rest) = body.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            body = &rest[end + 5..];
        }
    }
    if let Some(footer) = body.rfind("\n---\n") {
        body = &body[..footer];
    }
    body
}

fn sleep_cancellable(
    cancellation: &RoutedProviderCancellation,
    milliseconds: u64,
) -> Result<(), ProviderSessionError> {
    let deadline = Instant::now() + Duration::from_millis(milliseconds);
    while Instant::now() < deadline {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(Duration::from_millis(SHELL_CHECK_SLICE_MS)));
    }
    cancellation.check()
}
