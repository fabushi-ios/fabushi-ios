//! Strict OpenClaw Gateway frame envelopes and generation-fenced correlation.
//!
//! Socket/TLS ownership remains outside this module. A transport owner may
//! decode one bounded text frame here and may only settle a request through
//! the current connection generation.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

pub const MAX_GATEWAY_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRAME_ID_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum NodeHostGatewayFrame {
    Request {
        id: String,
        method: String,
        params: Option<Value>,
        traceparent: Option<String>,
        expected_profile_id: Option<String>,
    },
    Response {
        id: String,
        ok: bool,
        payload: Option<Value>,
        error: Option<Value>,
    },
    Event {
        event: String,
        payload: Option<Value>,
        seq: Option<u64>,
        state_version: Option<Value>,
        recipient_profile_id: Option<String>,
    },
}
fn require_object(value: Value) -> Result<Map<String, Value>, String> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "Gateway frame must be an object".into())
}

fn require_non_empty_string(
    object: &Map<String, Value>,
    field: &str,
    max: usize,
) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= max)
        .ok_or_else(|| format!("Gateway frame {field} is invalid"))?;
    if value.chars().any(char::is_control) {
        return Err(format!("Gateway frame {field} contains control characters"));
    }
    Ok(value.to_string())
}

fn reject_unknown_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Gateway frame contains unknown field: {key}"));
    }
    Ok(())
}

pub fn parse_gateway_frame(bytes: &[u8]) -> Result<NodeHostGatewayFrame, String> {
    if bytes.is_empty() || bytes.len() > MAX_GATEWAY_FRAME_BYTES {
        return Err("Gateway frame size is invalid".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Gateway frame JSON is invalid: {error}"))?;
    let object = require_object(value)?;
    let frame_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "Gateway frame type is missing".to_string())?;
    match frame_type {
        "req" => {
            reject_unknown_keys(
                &object,
                &["type", "id", "method", "params", "traceparent", "expectedProfileId"],
            )?;
            let id = require_non_empty_string(&object, "id", MAX_FRAME_ID_BYTES)?;
            let method = require_non_empty_string(&object, "method", MAX_FRAME_ID_BYTES)?;
            Ok(NodeHostGatewayFrame::Request {
                id,
                method,
                params: object.get("params").cloned(),
                traceparent: object
                    .get("traceparent")
                    .map(|_| require_non_empty_string(&object, "traceparent", 512))
                    .transpose()?,
                expected_profile_id: object
                    .get("expectedProfileId")
                    .map(|_| require_non_empty_string(&object, "expectedProfileId", 256))
                    .transpose()?,
            })
        }
        "res" => {
            reject_unknown_keys(&object, &["type", "id", "ok", "payload", "error"])?;
            let id = require_non_empty_string(&object, "id", MAX_FRAME_ID_BYTES)?;
            let ok = object
                .get("ok")
                .and_then(Value::as_bool)
                .ok_or_else(|| "Gateway response ok is invalid".to_string())?;
            if ok && object.contains_key("error") {
                return Err("successful Gateway response cannot contain error".into());
            }
            if !ok && !object.contains_key("error") {
                return Err("failed Gateway response must contain error".into());
            }
            Ok(NodeHostGatewayFrame::Response {
                id,
                ok,
                payload: object.get("payload").cloned(),
                error: object.get("error").cloned(),
            })
        }
        "event" => {
            reject_unknown_keys(
                &object,
                &[
                    "type",
                    "event",
                    "payload",
                    "seq",
                    "stateVersion",
                    "recipientProfileId",
                ],
            )?;
            let event = require_non_empty_string(&object, "event", MAX_FRAME_ID_BYTES)?;
            let seq = match object.get("seq") {
                None => None,
                Some(value) => Some(
                    value
                        .as_u64()
                        .ok_or_else(|| "Gateway event seq is invalid".to_string())?,
                ),
            };
            Ok(NodeHostGatewayFrame::Event {
                event,
                payload: object.get("payload").cloned(),
                seq,
                state_version: object.get("stateVersion").cloned(),
                recipient_profile_id: object
                    .get("recipientProfileId")
                    .map(|_| require_non_empty_string(&object, "recipientProfileId", 256))
                    .transpose()?,
            })
        }
        other => Err(format!("unsupported Gateway frame type: {other}")),
    }
}
pub fn encode_gateway_request(
    id: &str,
    method: &str,
    params: Option<Value>,
) -> Result<Vec<u8>, String> {
    if id.trim().is_empty()
        || id.len() > MAX_FRAME_ID_BYTES
        || id.chars().any(char::is_control)
        || method.trim().is_empty()
        || method.len() > MAX_FRAME_ID_BYTES
        || method.chars().any(char::is_control)
    {
        return Err("Gateway request identity or method is invalid".into());
    }
    let mut frame = json!({ "type": "req", "id": id.trim(), "method": method.trim() });
    if let Some(params) = params {
        frame["params"] = params;
    }
    let bytes = serde_json::to_vec(&frame)
        .map_err(|error| format!("encode Gateway request: {error}"))?;
    if bytes.len() > MAX_GATEWAY_FRAME_BYTES {
        return Err("encoded Gateway request exceeds frame limit".into());
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseSettlement {
    Current,
    StaleGeneration,
    UnknownRequest,
}

#[derive(Debug, Default)]
pub struct NodeHostGatewayRequestRegistry {
    generation: u64,
    next_id: u64,
    pending: BTreeMap<String, u64>,
}

impl NodeHostGatewayRequestRegistry {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            next_id: 0,
            pending: BTreeMap::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn replace_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.pending.clear();
    }

    pub fn admit(&mut self, prefix: &str) -> Result<(String, u64), String> {
        let prefix = prefix.trim();
        if prefix.is_empty() || prefix.len() > 32 || prefix.chars().any(char::is_control) {
            return Err("Gateway request id prefix is invalid".into());
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| "Gateway request id exhausted".to_string())?;
        let id = format!("{prefix}-{}-{}", self.generation, self.next_id);
        if id.len() > MAX_FRAME_ID_BYTES {
            return Err("Gateway request id is too long".into());
        }
        self.pending.insert(id.clone(), self.generation);
        Ok((id, self.generation))
    }

    pub fn settle(&mut self, id: &str, generation: u64) -> ResponseSettlement {
        let Some(admitted_generation) = self.pending.remove(id) else {
            return ResponseSettlement::UnknownRequest;
        };
        if admitted_generation != self.generation || generation != self.generation {
            return ResponseSettlement::StaleGeneration;
        }
        ResponseSettlement::Current
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        NodeHostGatewayFrame, NodeHostGatewayRequestRegistry, ResponseSettlement,
        encode_gateway_request, parse_gateway_frame,
    };

    #[test]
    fn frames_are_strict_and_requests_are_generation_fenced() {
        let encoded = encode_gateway_request(
            "req-1",
            "node.invoke.result",
            Some(json!({ "ok": true })),
        )
        .expect("encode");
        assert!(matches!(
            parse_gateway_frame(&encoded).expect("parse"),
            NodeHostGatewayFrame::Request { .. }
        ));
        assert!(parse_gateway_frame(
            br#"{"type":"res","id":"x","ok":true,"error":null}"#
        )
        .is_err());
        assert!(parse_gateway_frame(
            br#"{"type":"event","event":"tick","unknown":1}"#
        )
        .is_err());

        let mut registry = NodeHostGatewayRequestRegistry::new(7);
        let (id, generation) = registry.admit("connect").expect("admit");
        assert_eq!(registry.pending_count(), 1);
        assert_eq!(
            registry.settle(&id, generation + 1),
            ResponseSettlement::StaleGeneration
        );
        assert_eq!(
            registry.settle(&id, generation),
            ResponseSettlement::UnknownRequest
        );
        let (next_id, next_generation) = registry.admit("next").expect("admit next");
        registry.replace_generation(8);
        assert_eq!(
            registry.settle(&next_id, next_generation),
            ResponseSettlement::UnknownRequest
        );
    }
}
