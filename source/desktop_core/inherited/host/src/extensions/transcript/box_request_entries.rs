use serde_json::Value;

use crate::extensions::session::production::ProductionSessionWorkers;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveBoxRequest {
    pub agent_id: String,
    pub entry_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxRequestTrackDecision {
    pub next: Option<ActiveBoxRequest>,
    pub superseded_request_id: Option<String>,
}

pub fn pending_box_request_from_entry(
    agent_id: &str,
    entry: &Value,
) -> Option<ActiveBoxRequest> {
    if entry.get("kind").and_then(Value::as_str) != Some("send-message")
        || entry.get("boxResolution").is_some_and(|value| !value.is_null())
    {
        return None;
    }
    let entry_id = entry.get("id").and_then(Value::as_str)?.trim();
    let request_id = entry.get("boxRequestId").and_then(Value::as_str)?.trim();
    if entry_id.is_empty() || request_id.is_empty() {
        return None;
    }
    Some(ActiveBoxRequest {
        agent_id: agent_id.to_string(),
        entry_id: entry_id.to_string(),
        request_id: request_id.to_string(),
    })
}

pub fn track_box_request_entry(
    prior: Option<&ActiveBoxRequest>,
    agent_id: &str,
    entry: &Value,
) -> BoxRequestTrackDecision {
    let next = pending_box_request_from_entry(agent_id, entry);
    let superseded_request_id = match (prior, next.as_ref()) {
        (Some(prior), Some(next))
            if prior.agent_id == next.agent_id
                && prior.request_id != next.request_id =>
        {
            Some(prior.request_id.clone())
        }
        _ => None,
    };
    BoxRequestTrackDecision {
        next,
        superseded_request_id,
    }
}

pub fn resolve_box_request_entry(
    sessions: &ProductionSessionWorkers,
    agent_id: &str,
    request_id: &str,
    resolution: &str,
) -> Result<Option<Value>, String> {
    let entries = sessions.read_agent_transcript_entries(agent_id)?;
    let Some(target) = entries.into_iter().find(|entry| {
        entry.get("kind").and_then(Value::as_str) == Some("send-message")
            && entry.get("boxRequestId").and_then(Value::as_str) == Some(request_id)
            && entry.get("boxResolution").is_none_or(Value::is_null)
    }) else {
        return Ok(None);
    };
    let Some(entry_id) = target
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };
    let Some(mut object) = target.as_object().cloned() else {
        return Ok(None);
    };
    object.insert(
        "boxResolution".into(),
        Value::String(resolution.to_string()),
    );
    let next = Value::Object(object);
    if let Some(session_key) = target
        .get("sessionKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sessions.update_agent_transcript_entry_if_session_admitted(
            agent_id,
            session_key,
            &entry_id,
            &next,
        )
    } else {
        sessions.update_agent_transcript_entry(agent_id, &entry_id, &next)
    }
}
