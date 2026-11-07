//! Strict node.invoke request/input/cancel frame admission.
//!
//! The transport owns framing and correlation; this module owns only the
//! schema boundary. Direct params is rejected so a caller cannot silently
//! switch dialects, while the original paramsJSON text and byte count remain
//! available to the side-effect owner.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_INPUT_FRAME_BYTES: usize = 16 * 1024;
pub const MAX_PARAMS_JSON_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeInvokeError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeInvokeResult {
    pub id: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(rename = "payloadJSON", skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<NodeInvokeError>,
}

impl NodeInvokeResult {
    pub fn success_json(
        id: impl Into<String>,
        node_id: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            node_id: node_id.into(),
            ok: true,
            payload: None,
            payload_json: Some(payload_json.into()),
            error: None,
        }
    }

    pub fn success_payload(
        id: impl Into<String>,
        node_id: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            id: id.into(),
            node_id: node_id.into(),
            ok: true,
            payload: Some(payload),
            payload_json: None,
            error: None,
        }
    }

    pub fn failure(
        id: impl Into<String>,
        node_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            id: id.into(),
            node_id: node_id.into(),
            ok: false,
            payload: None,
            payload_json: None,
            error: Some(NodeInvokeError {
                code: Some(code.into()),
                message: Some(message.into()),
                details,
            }),
        }
    }

    /// Apply the Gateway capability fence used by the live node-host client.
    pub fn without_permission_details(mut self, allow_permission_details: bool) -> Self {
        if !allow_permission_details {
            if let Some(error) = self.error.as_mut() {
                error.details = None;
            }
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeInvokeRequest {
    pub id: String,
    pub node_id: String,
    pub command: String,
    pub params: Value,
    pub received_params_bytes: usize,
    pub received_params_json: Option<String>,
    pub timeout_ms: Option<u64>,
    pub idempotency_key: Option<String>,
    pub session_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInvokeInput {
    pub invoke_id: String,
    pub node_id: String,
    pub seq: u64,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInvokeCancel {
    pub invoke_id: String,
    pub node_id: String,
}

fn non_empty(label: &str, value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    Ok(value.to_string())
}

pub fn parse_node_invoke_request(payload: &Value) -> Result<NodeInvokeRequest, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "invalid node.invoke.request: expected object".to_string())?;
    let id = non_empty(
        "invocation id",
        object.get("id").and_then(Value::as_str).unwrap_or_default(),
    )?;
    let node_id = non_empty(
        "invocation node id",
        object
            .get("nodeId")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let command = non_empty(
        "invocation command",
        object
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    if object.contains_key("params") {
        return Err("invalid node.invoke.request: direct params are unsupported; use paramsJSON".into());
    }
    let received_params_json = match object.get("paramsJSON") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err("invalid node.invoke.request: paramsJSON must be a string or null".into()),
    };
    let (params, received_params_bytes) = match received_params_json.as_deref() {
        Some(raw) => {
            if raw.len() > MAX_PARAMS_JSON_BYTES {
                return Err("invalid invocation paramsJSON: exceeds 256 KiB".into());
            }
            (
                serde_json::from_str(raw)
                    .map_err(|error| format!("invalid invocation paramsJSON: {error}"))?,
                raw.len(),
            )
        }
        None => (Value::Null, 0),
    };
    let timeout_ms = match object.get("timeoutMs") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| "invalid node.invoke.request: timeoutMs must be a non-negative integer".to_string())?,
        ),
    };
    let idempotency_key = match object.get("idempotencyKey") {
        None | Some(Value::Null) => None,
        Some(value) => Some(non_empty(
            "invocation idempotency key",
            value
                .as_str()
                .ok_or_else(|| "invalid node.invoke.request: idempotencyKey must be a string".to_string())?,
        )?),
    };
    let session_key = match object.get("sessionKey") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| "invalid node.invoke.request: sessionKey must be a string".to_string())?
                .to_string(),
        ),
    };
    Ok(NodeInvokeRequest {
        id,
        node_id,
        command,
        params,
        received_params_bytes,
        received_params_json,
        timeout_ms,
        idempotency_key,
        session_key,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct InputFrame {
    id: String,
    node_id: String,
    seq: u64,
    #[serde(rename = "payloadJSON")]
    payload_json: String,
}

pub fn parse_node_invoke_input(payload: &Value) -> Result<NodeInvokeInput, String> {
    let frame: InputFrame = serde_json::from_value(payload.clone())
        .map_err(|error| format!("invalid node.invoke.input: {error}"))?;
    if frame.payload_json.len() > MAX_INPUT_FRAME_BYTES {
        return Err("node.invoke.input payloadJSON exceeds 16 KiB".into());
    }
    Ok(NodeInvokeInput {
        invoke_id: non_empty("input invocation id", &frame.id)?,
        node_id: non_empty("input invocation node id", &frame.node_id)?,
        seq: frame.seq,
        payload_json: frame.payload_json,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelFrame {
    invoke_id: String,
    node_id: String,
}

pub fn parse_node_invoke_cancel(payload: &Value) -> Result<NodeInvokeCancel, String> {
    let frame: CancelFrame = serde_json::from_value(payload.clone())
        .map_err(|error| format!("invalid node.invoke.cancel: {error}"))?;
    Ok(NodeInvokeCancel {
        invoke_id: non_empty("cancelled invocation id", &frame.invoke_id)?,
        node_id: non_empty("cancelled invocation node id", &frame.node_id)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        parse_node_invoke_cancel, parse_node_invoke_input, parse_node_invoke_request,
        NodeInvokeResult, MAX_INPUT_FRAME_BYTES, MAX_PARAMS_JSON_BYTES,
    };
    use serde_json::json;

    #[test]
    fn request_preserves_canonical_params_json_and_metadata() {
        let request = parse_node_invoke_request(&json!({
            "id": "invoke-1",
            "nodeId": "node-1",
            "command": "example.duplex",
            "paramsJSON": "{\"value\":1}",
            "timeoutMs": 1000,
            "idempotencyKey": "idem-1",
            "sessionKey": "agent:main"
        }))
        .expect("canonical request should parse");
        assert_eq!(request.params["value"], 1);
        assert_eq!(request.received_params_bytes, 11);
        assert_eq!(request.received_params_json.as_deref(), Some("{\"value\":1}"));
        assert_eq!(request.timeout_ms, Some(1000));
    }

    #[test]
    fn direct_params_and_malformed_params_json_fail_closed() {
        assert!(parse_node_invoke_request(&json!({
            "id": "i", "nodeId": "n", "command": "c", "params": {}
        }))
        .unwrap_err()
        .contains("direct params"));
        assert!(parse_node_invoke_request(&json!({
            "id": "i", "nodeId": "n", "command": "c", "paramsJSON": "{"
        }))
        .unwrap_err()
        .contains("invalid invocation paramsJSON"));
        let oversized = "x".repeat(MAX_PARAMS_JSON_BYTES + 1);
        assert!(parse_node_invoke_request(&json!({
            "id": "i", "nodeId": "n", "command": "c", "paramsJSON": oversized
        }))
        .unwrap_err()
        .contains("256 KiB"));
    }

    #[test]
    fn input_bounds_and_cancel_identity_are_checked() {
        let input = parse_node_invoke_input(&json!({
            "id": "i", "nodeId": "n", "seq": 2, "payloadJSON": "payload"
        }))
        .expect("input should parse");
        assert_eq!(input.seq, 2);
        let oversized = "x".repeat(MAX_INPUT_FRAME_BYTES + 1);
        assert!(parse_node_invoke_input(&json!({
            "id": "i", "nodeId": "n", "seq": 0, "payloadJSON": oversized
        }))
        .unwrap_err()
        .contains("16 KiB"));
        let cancel = parse_node_invoke_cancel(&json!({
            "invokeId": "i", "nodeId": "n"
        }))
        .expect("cancel should parse");
        assert_eq!(cancel.invoke_id, "i");
    }

    #[test]
    fn result_preserves_ids_and_capability_fences_error_details() {
        let success = NodeInvokeResult::success_json("invoke-1", "node-1", "{\"ok\":true}");
        let success_json = serde_json::to_value(&success).expect("serialize result");
        assert_eq!(success_json["id"], "invoke-1");
        assert_eq!(success_json["nodeId"], "node-1");
        assert_eq!(success_json["ok"], true);
        assert_eq!(success_json["payloadJSON"], "{\"ok\":true}");
        assert!(success_json.get("payload").is_none());

        let failure = NodeInvokeResult::failure(
            "invoke-2",
            "node-1",
            "PERMISSION_DENIED",
            "permission denied",
            Some(json!({"scope": "computer"})),
        );
        let redacted = serde_json::to_value(failure.clone().without_permission_details(false))
            .expect("serialize redacted result");
        assert_eq!(redacted["error"]["code"], "PERMISSION_DENIED");
        assert!(redacted["error"].get("details").is_none());
        let permitted = serde_json::to_value(failure.without_permission_details(true))
            .expect("serialize detailed result");
        assert_eq!(permitted["error"]["details"]["scope"], "computer");
    }
}
