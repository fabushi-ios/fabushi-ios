use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::agent_v1_wire::{
    AgentToolCall, CommunicateUpdateArgs, CommunicateUpdateError,
    CommunicateUpdateResult, CommunicateUpdateSuccess, CommunicateUpdateToolCall,
    agent_tool_call, communicate_update_result,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use crate::runner::turn_observation::TurnObservationHandle;

pub const SAND_TOOL_MARKER: &str = "__sand_tool__";
pub const FROZEN_COMMUNICATE_TOOL_NAMES: &[&str] = &[
    "SendToAgent",
    "CreateAgent",
    "UpdateAgent",
    "CheckSubagent",
    "MessageSubagent",
    "StopSubagent",
    "CopyToBox",
    "CopyFromBox",
    "update_state",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "case", content = "value", rename_all = "camelCase")]
pub enum CommunicateResult {
    Success { current_step: String },
    Error { error: String },
}

pub fn encode_sand_step(payload: &serde_json::Map<String, Value>) -> String {
    let mut row = serde_json::Map::new();
    row.insert(SAND_TOOL_MARKER.to_string(), Value::Bool(true));
    for (key, value) in payload {
        row.insert(key.clone(), value.clone());
    }
    Value::Object(row).to_string()
}

pub fn tool_call_wrapper(payload: &serde_json::Map<String, Value>) -> Value {
    json!({
        "tool": {
            "case": "communicateUpdateToolCall",
            "value": {
                "args": {
                    "currentStep": encode_sand_step(payload)
                }
            }
        }
    })
}

pub fn empty_tool_call() -> Value {
    json!({
        "tool": {
            "case": "communicateUpdateToolCall",
            "value": {}
        }
    })
}

pub fn encode_error(message: &str) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert("error".into(), Value::String(message.to_string()));
    encode_sand_step(&payload)
}

pub fn encode_success(result: &str) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert("result".into(), Value::String(result.to_string()));
    encode_sand_step(&payload)
}

pub fn build_success_result(text: impl Into<String>) -> CommunicateResult {
    CommunicateResult::Success {
        current_step: text.into(),
    }
}

pub fn build_error_result(message: impl Into<String>) -> CommunicateResult {
    CommunicateResult::Error {
        error: message.into(),
    }
}

pub fn completed_tool_call(result: &CommunicateResult) -> Value {
    let (args, encoded_result) = match result {
        CommunicateResult::Error { error } => (
            encode_error(if error.is_empty() { "Tool failed." } else { error }),
            json!({"case":"error","value":{"error":error}}),
        ),
        CommunicateResult::Success { current_step } => (
            encode_success(current_step),
            json!({"case":"success","value":{"currentStep":current_step}}),
        ),
    };
    json!({
        "tool": {
            "case": "communicateUpdateToolCall",
            "value": {
                "args": {"currentStep": args},
                "result": {"result": encoded_result}
            }
        }
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommunicateActivity {
    pub detail: Option<String>,
    pub target: Option<String>,
}

pub fn executing_tool_call(
    tool_name: &str,
    activity: Option<&CommunicateActivity>,
) -> Value {
    let mut payload = executing_payload(tool_name, activity);
    tool_call_wrapper(&payload)
}

fn executing_payload(
    tool_name: &str,
    activity: Option<&CommunicateActivity>,
) -> serde_json::Map<String, Value> {
    let mut payload = serde_json::Map::new();
    payload.insert("phase".into(), Value::String("executing".into()));
    payload.insert("tool".into(), Value::String(tool_name.to_string()));
    if let Some(detail) = activity
        .and_then(|activity| activity.detail.as_deref())
        .filter(|value| !value.is_empty())
    {
        payload.insert("detail".into(), Value::String(detail.to_string()));
    }
    if let Some(target) = activity
        .and_then(|activity| activity.target.as_deref())
        .filter(|value| !value.is_empty())
    {
        payload.insert("target".into(), Value::String(target.to_string()));
    }
    payload
}

fn wire_result(result: &CommunicateResult) -> CommunicateUpdateResult {
    let result = match result {
        CommunicateResult::Success { current_step } => {
            communicate_update_result::Result::Success(CommunicateUpdateSuccess {
                current_step: current_step.clone(),
                message_index: 0,
            })
        }
        CommunicateResult::Error { error } => {
            communicate_update_result::Result::Error(CommunicateUpdateError {
                error: error.clone(),
            })
        }
    };
    CommunicateUpdateResult {
        result: Some(result),
    }
}

pub fn executing_tool_call_wire(
    tool_name: &str,
    activity: Option<&CommunicateActivity>,
    tool_call_id: &str,
    started_at_ms: u64,
) -> AgentToolCall {
    let args = CommunicateUpdateArgs {
        current_step: Some(encode_sand_step(&executing_payload(tool_name, activity))),
        final_summary: None,
        completed_subtitle: None,
    };
    AgentToolCall {
        tool: Some(agent_tool_call::Tool::CommunicateUpdateToolCall(
            CommunicateUpdateToolCall {
                args: Some(args),
                result: None,
            },
        )),
        tool_call_id: Some(tool_call_id.to_string()),
        started_at_ms: Some(started_at_ms),
        completed_at_ms: None,
    }
}

pub fn completed_tool_call_wire(
    result: &CommunicateResult,
    tool_call_id: &str,
    started_at_ms: u64,
    completed_at_ms: u64,
) -> AgentToolCall {
    let args = match result {
        CommunicateResult::Error { error } => {
            encode_error(if error.is_empty() { "Tool failed." } else { error })
        }
        CommunicateResult::Success { current_step } => encode_success(current_step),
    };
    AgentToolCall {
        tool: Some(agent_tool_call::Tool::CommunicateUpdateToolCall(
            CommunicateUpdateToolCall {
                args: Some(CommunicateUpdateArgs {
                    current_step: Some(args),
                    final_summary: None,
                    completed_subtitle: None,
                }),
                result: Some(wire_result(result)),
            },
        )),
        tool_call_id: Some(tool_call_id.to_string()),
        started_at_ms: Some(started_at_ms),
        completed_at_ms: Some(completed_at_ms),
    }
}

pub fn render_result(result: &CommunicateResult) -> String {
    match result {
        CommunicateResult::Error { error } => format!("Error: {error}"),
        CommunicateResult::Success { current_step } if current_step.is_empty() => {
            "Tool completed.".to_string()
        }
        CommunicateResult::Success { current_step } => current_step.clone(),
    }
}

pub fn run_communicate_execution<F, E>(execute: F) -> CommunicateResult
where
    F: FnOnce() -> Result<String, E>,
    E: std::fmt::Display,
{
    match execute() {
        Ok(text) => build_success_result(text),
        Err(error) => build_error_result(error.to_string()),
    }
}

pub fn serialize_error(error: impl std::fmt::Display) -> Value {
    let message = error.to_string();
    completed_tool_call(&build_error_result(message))
}

pub trait CommunicateInteractionSink: Send + Sync {
    fn on_tool_call(
        &self,
        phase: &'static str,
        tool_name: &str,
        tool_call: &AgentToolCall,
    );
}

pub struct TurnObservationCommunicateSink {
    observation: TurnObservationHandle,
}

impl TurnObservationCommunicateSink {
    pub fn new(observation: TurnObservationHandle) -> Self {
        Self { observation }
    }
}

impl CommunicateInteractionSink for TurnObservationCommunicateSink {
    fn on_tool_call(
        &self,
        phase: &'static str,
        tool_name: &str,
        tool_call: &AgentToolCall,
    ) {
        if let Ok(observation) = self.observation.lock() {
            observation.observe_communicate_tool_call(
                phase,
                tool_name,
                tool_call.tool_call_id.as_deref().unwrap_or_default(),
                &tool_call.encode_to_vec(),
            );
        }
    }
}

pub struct CommunicateRoutedToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    sink: Option<Arc<dyn CommunicateInteractionSink>>,
}

impl CommunicateRoutedToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        sink: Option<Arc<dyn CommunicateInteractionSink>>,
    ) -> Self {
        Self { delegate, sink }
    }
}

fn routed_tool_name(tool: &RoutedToolDefinition) -> &str {
    if tool.tool_name.trim().is_empty() {
        tool.name.as_str()
    } else {
        tool.tool_name.as_str()
    }
}

fn is_frozen_communicate_tool(name: &str) -> bool {
    FROZEN_COMMUNICATE_TOOL_NAMES.contains(&name)
}

fn value_result_text(value: &Value) -> String {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

impl RoutedToolBridge for CommunicateRoutedToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.delegate.list_tools()
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let name = routed_tool_name(tool);
        if !is_frozen_communicate_tool(name) {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }

        let started_at_ms = now_ms();
        let activity = (name == "update_state")
            .then(|| crate::runner::tools::sand_state_tool::describe_state_update(&args))
            .flatten()
            .map(|detail| CommunicateActivity {
                detail: Some(detail),
                target: None,
            });
        let initial = executing_tool_call_wire(name, activity.as_ref(), tool_call_id, started_at_ms);
        if let Some(sink) = self.sink.as_ref() {
            sink.on_tool_call("initial", name, &initial);
        }

        let result = self.delegate.call_tool(tool, args, tool_call_id);
        let communicate_result = match &result {
            Ok(value) => build_success_result(value_result_text(value)),
            Err(error) => build_error_result(error.to_string()),
        };
        let completed = completed_tool_call_wire(
            &communicate_result,
            tool_call_id,
            started_at_ms,
            now_ms(),
        );
        if let Some(sink) = self.sink.as_ref() {
            sink.on_tool_call("completed", name, &completed);
        }
        result
    }
}
