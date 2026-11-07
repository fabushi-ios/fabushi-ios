use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use crate::runner::subagent_runtime::{
    ControlResult, RunningSubagentInfo, SubagentRuntime,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerReview {
    pub allowed: bool,
    pub reason: String,
}

pub fn elapsed_label(elapsed_ms: u64) -> String {
    let total_seconds = elapsed_ms.saturating_add(500) / 1_000;
    if total_seconds < 90 {
        return format!("{total_seconds}s");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if seconds == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

pub fn describe_running_subagent(info: &RunningSubagentInfo, detailed: bool) -> String {
    let header = format!(
        "- {} [{}] \"{}\" — running for {}, {} tool call(s)",
        info.subagent_id,
        info.subagent_type,
        info.title,
        elapsed_label(info.elapsed_ms),
        info.tool_call_count,
    );
    if !detailed {
        return header;
    }
    let mut lines = vec![header];
    if info.recent_activity.is_empty() {
        lines.push("  No tool activity recorded yet.".to_string());
    } else {
        lines.push("  Recent activity (oldest → newest):".to_string());
        lines.extend(
            info.recent_activity
                .iter()
                .map(|entry| format!("    {entry}")),
        );
    }
    if let Some(path) = info.transcript_path.as_deref() {
        lines.push(format!(
            "  Full transcript (read it for the complete play-by-play): {path}"
        ));
    }
    lines.join("\n")
}

pub fn not_running_message(
    subagent_id: &str,
    running: &[RunningSubagentInfo],
) -> String {
    let base = format!(
        "No subagent \"{subagent_id}\" is currently running. It may have already finished (you're revived automatically with a finished subagent's result), or the id is wrong."
    );
    if running.is_empty() {
        format!("{base} No subagents are running right now.")
    } else {
        format!(
            "{base} Currently running: {}.",
            running
                .iter()
                .map(|info| info.subagent_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

pub fn check_subagent(
    runtime: &SubagentRuntime,
    subagent_id: Option<&str>,
    now_ms: u64,
) -> String {
    let id = subagent_id.map(str::trim).filter(|id| !id.is_empty());
    let running = runtime.list_running_subagents(now_ms);
    let Some(id) = id else {
        if running.is_empty() {
            return "No background subagents are running right now.".to_string();
        }
        let mut lines = vec![format!("{} subagent(s) running:", running.len())];
        lines.extend(
            running
                .iter()
                .map(|info| describe_running_subagent(info, false)),
        );
        lines.push(
            "Pass a subagent_id to see its recent activity and transcript path.".to_string(),
        );
        return lines.join("\n");
    };
    runtime
        .get_running_subagent(id, now_ms)
        .map(|info| describe_running_subagent(&info, true))
        .unwrap_or_else(|| not_running_message(id, &running))
}

pub fn message_subagent(
    runtime: &mut SubagentRuntime,
    subagent_id: &str,
    message: &str,
    review: Option<&SteerReview>,
    now_ms: u64,
) -> String {
    if let Some(review) = review.filter(|review| !review.allowed) {
        return review.reason.clone();
    }
    if matches!(
        runtime.steer_subagent(subagent_id, message),
        ControlResult::NotRunning
    ) {
        return not_running_message(
            subagent_id,
            &runtime.list_running_subagents(now_ms),
        );
    }
    format!(
        "Message delivered to subagent {subagent_id}. It will interrupt what it's doing, take your message into account, and keep working. You'll be revived with its result when it finishes — don't wait on it."
    )
}

pub fn stop_subagent(
    runtime: &mut SubagentRuntime,
    subagent_id: &str,
    now_ms: u64,
) -> String {
    if matches!(
        runtime.abort_subagent(subagent_id),
        ControlResult::NotRunning
    ) {
        return not_running_message(
            subagent_id,
            &runtime.list_running_subagents(now_ms),
        );
    }
    format!(
        "Stopping subagent {subagent_id}. It will be torn down and won't report back."
    )
}


pub const CHECK_SUBAGENT_TOOL_NAME: &str = "CheckSubagent";
pub const MESSAGE_SUBAGENT_TOOL_NAME: &str = "MessageSubagent";
pub const STOP_SUBAGENT_TOOL_NAME: &str = "StopSubagent";

pub type SubagentSteerReviewCallback = Arc<
    dyn Fn(&str, &str, &str) -> Result<SteerReview, ProviderSessionError> + Send + Sync + 'static,
>;

pub struct SubagentManagementToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    runtime: Arc<Mutex<SubagentRuntime>>,
    review_steer: Option<SubagentSteerReviewCallback>,
}

impl SubagentManagementToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        runtime: Arc<Mutex<SubagentRuntime>>,
        review_steer: Option<SubagentSteerReviewCallback>,
    ) -> Self {
        Self {
            delegate,
            runtime,
            review_steer,
        }
    }
}

fn management_tool_definitions() -> [RoutedToolDefinition; 3] {
    [
        RoutedToolDefinition {
            name: CHECK_SUBAGENT_TOOL_NAME.into(),
            provider_identifier: "fabushi-runner".into(),
            tool_name: CHECK_SUBAGENT_TOOL_NAME.into(),
            description: Some("Check how a background subagent dispatched via Task is doing without waiting for it to finish. Omit subagent_id to list every running subagent; pass it to inspect status, recent activity and transcript path.".into()),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "subagent_id": {"type":"string"}
                }
            }),
        },
        RoutedToolDefinition {
            name: MESSAGE_SUBAGENT_TOOL_NAME.into(),
            provider_identifier: "fabushi-runner".into(),
            tool_name: MESSAGE_SUBAGENT_TOOL_NAME.into(),
            description: Some("Force a message into a running background subagent to course-correct it without aborting it. The subagent keeps its context and continues after the steering interruption.".into()),
            input_schema: json!({
                "type": "object",
                "required": ["subagent_id", "message"],
                "additionalProperties": false,
                "properties": {
                    "subagent_id": {"type":"string","minLength":1},
                    "message": {"type":"string","minLength":1}
                }
            }),
        },
        RoutedToolDefinition {
            name: STOP_SUBAGENT_TOOL_NAME.into(),
            provider_identifier: "fabushi-runner".into(),
            tool_name: STOP_SUBAGENT_TOOL_NAME.into(),
            description: Some("Abort a running background subagent dispatched via Task. Use MessageSubagent instead when the subagent should change course and keep working.".into()),
            input_schema: json!({
                "type": "object",
                "required": ["subagent_id"],
                "additionalProperties": false,
                "properties": {
                    "subagent_id": {"type":"string","minLength":1}
                }
            }),
        },
    ]
}

fn is_management_tool(name: &str) -> bool {
    [
        CHECK_SUBAGENT_TOOL_NAME,
        MESSAGE_SUBAGENT_TOOL_NAME,
        STOP_SUBAGENT_TOOL_NAME,
    ]
    .iter()
    .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

fn routed_name(tool: &RoutedToolDefinition) -> &str {
    if tool.tool_name.trim().is_empty() {
        tool.name.as_str()
    } else {
        tool.tool_name.as_str()
    }
}

fn required_trimmed_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, ProviderSessionError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderSessionError::Tool(format!("{key} is required")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

impl RoutedToolBridge for SubagentManagementToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| !is_management_tool(routed_name(tool)) && !is_management_tool(&tool.name));
        for definition in management_tool_definitions().into_iter().rev() {
            tools.insert(0, definition);
        }
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let name = routed_name(tool);
        if !is_management_tool(name) {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }
        let current_ms = now_ms();
        match name {
            CHECK_SUBAGENT_TOOL_NAME => {
                let subagent_id = args
                    .get("subagent_id")
                    .or_else(|| args.get("subagentId"))
                    .and_then(Value::as_str);
                let runtime = self.runtime.lock().map_err(|_| {
                    ProviderSessionError::Tool("subagent runtime mutex poisoned".into())
                })?;
                Ok(Value::String(check_subagent(&runtime, subagent_id, current_ms)))
            }
            MESSAGE_SUBAGENT_TOOL_NAME => {
                let subagent_id = required_trimmed_string(&args, "subagent_id")?;
                let message = required_trimmed_string(&args, "message")?;
                let review = match self.review_steer.as_ref() {
                    Some(review) => Some(review(subagent_id, message, tool_call_id)?),
                    None => None,
                };
                let mut runtime = self.runtime.lock().map_err(|_| {
                    ProviderSessionError::Tool("subagent runtime mutex poisoned".into())
                })?;
                Ok(Value::String(message_subagent(
                    &mut runtime,
                    subagent_id,
                    message,
                    review.as_ref(),
                    current_ms,
                )))
            }
            STOP_SUBAGENT_TOOL_NAME => {
                let subagent_id = required_trimmed_string(&args, "subagent_id")?;
                let mut runtime = self.runtime.lock().map_err(|_| {
                    ProviderSessionError::Tool("subagent runtime mutex poisoned".into())
                })?;
                Ok(Value::String(stop_subagent(
                    &mut runtime,
                    subagent_id,
                    current_ms,
                )))
            }
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}
