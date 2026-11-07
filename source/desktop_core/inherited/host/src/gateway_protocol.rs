use serde_json::{Value, json};

pub const GATEWAY_PREPARE_UPGRADE_PATH: &str = "/prepare-upgrade";

pub use crate::host_gateway_api::FROZEN_HOST_GATEWAY_METHODS as GROK_GATEWAY_COMMANDS;
pub use crate::host_gateway_api::{OPENCLAW_GATEWAY_METHODS, openclaw_gateway_owner};
use crate::host_gateway_api::host_gateway_owner;

/// Fabushi-owned gateway command that intentionally extends the frozen Grok 0.18
/// public Host surface. Keep this outside `FROZEN_HOST_GATEWAY_METHODS` so
/// parity checks cannot silently widen the upstream contract.
pub const FABUSHI_GATEWAY_COMPAT_COMMANDS: &[&str] = &["resumeAfterRecreate"];

pub fn is_openclaw_gateway_command(method: &str) -> bool {
    OPENCLAW_GATEWAY_METHODS.contains(&method)
}

pub fn is_grok_gateway_command(method: &str) -> bool {
    host_gateway_owner(method).is_some()
        || FABUSHI_GATEWAY_COMPAT_COMMANDS.contains(&method)
        || is_openclaw_gateway_command(method)
}

pub fn parse_command_args(body: &[u8]) -> Result<Value, serde_json::Error> {
    if body.is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_slice(body)
    }
}

pub fn slim_command_result(method: &str, mut value: Value) -> Value {
    fn strip_summary(summary: &mut Value) {
        if let Some(object) = summary.as_object_mut() {
            if object.contains_key("avatarDataUrl") {
                object.insert("avatarDataUrl".into(), Value::Null);
            }
        }
    }

    match method {
        "listAgents" => {
            if let Some(rows) = value.as_array_mut() {
                for row in rows {
                    strip_summary(row);
                }
            }
        }
        "updateAgent" | "setGroupMembers" | "setAgentAvatarBytes" => strip_summary(&mut value),
        "createAgent" | "createGroup" | "duplicateAgent" => {
            if let Some(agent) = value.get_mut("agent") {
                strip_summary(agent);
            }
        }
        _ => {}
    }
    value
}

pub fn slim_event(mut event: Value) -> Value {
    match event.get("channel").and_then(Value::as_str) {
        Some("agents") => {
            if let Some(rows) = event
                .get_mut("payload")
                .and_then(|payload| payload.get_mut("agents"))
                .and_then(Value::as_array_mut)
            {
                for row in rows {
                    if let Some(object) = row.as_object_mut() {
                        if object.contains_key("avatarDataUrl") {
                            object.insert("avatarDataUrl".into(), Value::Null);
                        }
                    }
                }
            }
        }
        Some("agent-upserted") => {
            if let Some(object) = event
                .get_mut("payload")
                .and_then(|payload| payload.get_mut("agent"))
                .and_then(Value::as_object_mut)
            {
                if object.contains_key("avatarDataUrl") {
                    object.insert("avatarDataUrl".into(), Value::Null);
                }
            }
        }
        _ => {}
    }
    event
}

