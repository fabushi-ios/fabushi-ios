use crate::protocol::Failure;
use serde_json::{Value, json};

pub const RUNNER_TOOL_REQUEST_EVENT_CHANNEL: &str = "runner-tool-request";
pub const RUNNER_RESOLVE_ROUTED_TOOL_GATEWAY_METHOD: &str =
    "runner.resolveRoutedToolRequest";
pub const ROUTED_TOOL_LIST_METHOD: &str = "listRoutedMcpTools";
pub const ROUTED_TOOL_EXECUTE_METHOD: &str = "executeRoutedMcpTool";

#[derive(Debug, Clone, PartialEq)]
pub struct RunnerToolRequest {
    pub request_id: String,
    pub method: String,
    pub args: Value,
}

pub fn parse_runner_tool_request(payload: &Value) -> Result<RunnerToolRequest, Failure> {
    let request_id = payload
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Failure::new(
                "RUNNER_TOOL_RELAY_PROTOCOL_ERROR",
                "runner tool request requires requestId",
            )
        })?;
    if request_id.len() > 256 {
        return Err(Failure::new(
            "RUNNER_TOOL_RELAY_PROTOCOL_ERROR",
            "runner tool requestId exceeds 256 bytes",
        ));
    }

    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            Failure::new(
                "RUNNER_TOOL_RELAY_PROTOCOL_ERROR",
                "runner tool request requires method",
            )
        })?;
    if !matches!(method, ROUTED_TOOL_LIST_METHOD | ROUTED_TOOL_EXECUTE_METHOD) {
        return Err(Failure::new(
            "RUNNER_TOOL_RELAY_METHOD_DENIED",
            format!("runner tool relay may not invoke {method}"),
        ));
    }

    let args = payload.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err(Failure::new(
            "RUNNER_TOOL_RELAY_PROTOCOL_ERROR",
            "runner tool request args must be an object",
        ));
    }

    Ok(RunnerToolRequest {
        request_id: request_id.to_string(),
        method: method.to_string(),
        args,
    })
}

pub fn runner_tool_resolution_success(request_id: &str, result: Value) -> Value {
    json!({
        "requestId": request_id,
        "ok": true,
        "result": result,
    })
}

pub fn runner_tool_resolution_failure(
    request_id: &str,
    message: impl Into<String>,
) -> Value {
    json!({
        "requestId": request_id,
        "ok": false,
        "error": message.into(),
    })
}

pub fn execute_runner_tool_request<Execute>(
    payload: &Value,
    execute: Execute,
) -> Result<Value, Failure>
where
    Execute: FnOnce(&str, Value) -> Result<Value, Failure>,
{
    let request = parse_runner_tool_request(payload)?;
    let request_id = request.request_id.clone();
    Ok(match execute(&request.method, request.args) {
        Ok(result) => runner_tool_resolution_success(&request_id, result),
        Err(failure) => runner_tool_resolution_failure(
            &request_id,
            format!("{}: {}", failure.code, failure.message),
        ),
    })
}
