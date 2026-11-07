use serde_json::Value;

use super::agent_db::{AgentDbProjectionError, SandAgentDb};

pub fn expire_pending_auto_review_approval_entries(
    owner: &SandAgentDb,
    only_request_id: Option<&str>,
) -> Result<Vec<String>, AgentDbProjectionError> {
    let entries = owner.get_transcript_entries()?;
    let mut expired = Vec::new();
    for entry in entries {
        let Some(request_id) = pending_request_id(
            &entry,
            "auto-review-approval",
            "approval",
        ) else {
            continue;
        };
        if only_request_id.is_some_and(|only| only != request_id) {
            continue;
        }
        let request_id = request_id.to_string();
        let Some(settled) = settle_pending_entry(
            &entry,
            "auto-review-approval",
            "approval",
            only_request_id.unwrap_or(&request_id),
        ) else {
            continue;
        };
        let updated = update_swept_entry(owner, &entry, &settled)?;
        if updated.is_some() {
            expired.push(request_id);
        }
    }
    Ok(expired)
}

pub fn expire_pending_local_tool_permission_ask_entries(
    owner: &SandAgentDb,
    only_request_id: Option<&str>,
    if_pending_before_ms: Option<f64>,
) -> Result<Vec<String>, AgentDbProjectionError> {
    let entries = owner.get_transcript_entries()?;
    let mut expired = Vec::new();
    for entry in entries {
        let Some(request_id) = pending_request_id(
            &entry,
            "local-tool-permission",
            "ask",
        ) else {
            continue;
        };
        if if_pending_before_ms.is_some_and(|cutoff| {
            entry
                .get("timestampMs")
                .and_then(Value::as_f64)
                .is_some_and(|timestamp| timestamp >= cutoff)
        }) {
            continue;
        }
        if only_request_id.is_some_and(|only| only != request_id) {
            continue;
        }
        let request_id = request_id.to_string();
        let Some(settled) = settle_pending_entry(
            &entry,
            "local-tool-permission",
            "ask",
            only_request_id.unwrap_or(&request_id),
        ) else {
            continue;
        };
        let updated = update_swept_entry(owner, &entry, &settled)?;
        if updated.is_some() {
            expired.push(request_id);
        }
    }
    Ok(expired)
}

fn pending_request_id<'a>(
    entry: &'a Value,
    message_type: &str,
    nested_key: &str,
) -> Option<&'a str> {
    if entry.get("kind").and_then(Value::as_str) != Some("send-message") {
        return None;
    }
    let message = entry.get("message")?.as_object()?;
    if message.get("type").and_then(Value::as_str) != Some(message_type) {
        return None;
    }
    let nested = message.get(nested_key)?.as_object()?;
    if nested.get("status").and_then(Value::as_str) != Some("pending") {
        return None;
    }
    nested
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|request_id| !request_id.is_empty())
}

fn settle_pending_entry(
    entry: &Value,
    message_type: &str,
    nested_key: &str,
    request_id: &str,
) -> Option<Value> {
    if pending_request_id(entry, message_type, nested_key)? != request_id {
        return None;
    }
    let mut settled = entry.clone();
    settled
        .get_mut("message")?
        .as_object_mut()?
        .get_mut(nested_key)?
        .as_object_mut()?
        .insert("status".into(), Value::String("expired".into()));
    Some(settled)
}

fn entry_id(entry: &Value) -> Option<&str> {
    entry.get("id").and_then(Value::as_str)
}

fn update_swept_entry(
    owner: &SandAgentDb,
    current: &Value,
    next: &Value,
) -> Result<Option<Value>, AgentDbProjectionError> {
    let entry_id = entry_id(current).unwrap_or_default();
    if let Some(session_key) = current
        .get("sessionKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        owner.update_transcript_entry_if_session_admitted(session_key, entry_id, next)
    } else {
        owner.update_transcript_entry(entry_id, next)
    }
}
