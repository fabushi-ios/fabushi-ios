use serde_json::Value;

use super::send_message_shaping::{
    describe_replied_message_quote, strip_reply_to, with_reply_to,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyContext {
    pub target_id: String,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReplyThreading {
    pub reply_to_id: Option<String>,
    pub reply_context: Option<ReplyContext>,
    pub is_fork: bool,
}

pub fn resolve_send_reply_threading(
    entries: &[Value],
    reply_to_id_option: Option<&str>,
    is_fork_option: bool,
) -> SendReplyThreading {
    let reply_to_id = reply_to_id_option
        .filter(|candidate| entries.iter().any(|entry| entry.get("id").and_then(Value::as_str) == Some(*candidate)))
        .map(ToOwned::to_owned);
    let reply_context = reply_to_id.as_deref().and_then(|target_id| {
        entries
            .iter()
            .find(|entry| entry.get("id").and_then(Value::as_str) == Some(target_id))
            .map(|target| ReplyContext {
                target_id: target_id.to_string(),
                quote: describe_replied_message_quote(target),
            })
    });
    SendReplyThreading {
        is_fork: is_fork_option && reply_to_id.is_some(),
        reply_to_id,
        reply_context,
    }
}

pub fn validate_ai_reply_target(message: &Value, in_flight_id: Option<&str>, entries: &[Value]) -> Value {
    let Some(target) = message.get("reply_to").and_then(Value::as_str).filter(|value| !value.is_empty()) else {
        return message.clone();
    };
    let target_is_live = entries.iter().any(|entry| entry.get("id").and_then(Value::as_str) == Some(target));
    if in_flight_id == Some(target) || !target_is_live { strip_reply_to(message) } else { message.clone() }
}

pub fn apply_auto_reply_thread(message: &Value, reply_thread_target: Option<&str>, entries: &[Value]) -> Value {
    if message.get("reply_to").and_then(Value::as_str).is_some_and(|value| !value.is_empty()) {
        return message.clone();
    }
    let Some(target) = reply_thread_target else { return message.clone(); };
    if entries.iter().any(|entry| entry.get("id").and_then(Value::as_str) == Some(target)) {
        with_reply_to(message, target)
    } else {
        message.clone()
    }
}
