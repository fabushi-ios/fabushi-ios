//! Pure heartbeat policy owned by the Host runner.
//!
//! This module deliberately owns no timer, session, transcript, scheduler, or
//! notification transport. Those responsibilities stay with their existing
//! owners; these functions only make heartbeat prompt/response decisions
//! deterministic and testable.

use serde_json::Value;

pub const HEARTBEAT_TOKEN: &str = "HEARTBEAT_OK";
pub const SILENT_REPLY_TOKEN: &str = "NO_REPLY";
pub const DEFAULT_HEARTBEAT_PROMPT: &str =
    "Follow the heartbeat monitor scratch context when provided. Recurring tasks are automations; create or change their schedules with the automations tool, not heartbeat scratch. Do not infer or repeat old tasks from prior chats. If nothing needs attention, reply NO_REPLY.";
pub const HEARTBEAT_RESPONSE_TOOL_INSTRUCTIONS: &str =
    "Use heartbeat_respond to report the wake outcome. Set notify=false when nothing needs the user's attention. Set notify=true with notificationText only when the user should be interrupted.";
pub const HEARTBEAT_RESPONSE_TOOL_FALLBACK_INSTRUCTIONS: &str =
    "If the heartbeat_respond tool is not available in this run, reply NO_REPLY when nothing needs the user's attention; when the user should be interrupted, reply with only the alert text instead of a prose report.";
pub const DEFAULT_HEARTBEAT_ACK_MAX_CHARS: usize = 300;
pub const HEARTBEAT_SCRATCH_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatResponseOutcome {
    NoChange,
    Progress,
    Done,
    Blocked,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatResponsePriority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatResponse {
    pub outcome: HeartbeatResponseOutcome,
    pub notify: bool,
    pub summary: String,
    pub notification_text: Option<String>,
    pub reason: Option<String>,
    pub priority: Option<HeartbeatResponsePriority>,
    pub next_check: Option<String>,
    pub scratch: Option<String>,
}

/// Normalizes the fixed-source `heartbeat_respond` result without owning any
/// scheduler, session, transcript, notification, or durable outcome state.
pub fn normalize_heartbeat_response(value: &Value) -> Option<HeartbeatResponse> {
    let object = value.as_object()?;
    let outcome = match trimmed_string(object.get("outcome"))?.as_str() {
        "no_change" => HeartbeatResponseOutcome::NoChange,
        "progress" => HeartbeatResponseOutcome::Progress,
        "done" => HeartbeatResponseOutcome::Done,
        "blocked" => HeartbeatResponseOutcome::Blocked,
        "needs_attention" => HeartbeatResponseOutcome::NeedsAttention,
        _ => return None,
    };
    let notify = object.get("notify")?.as_bool()?;
    let summary = trimmed_string(object.get("summary"))?;
    let notification_text = first_trimmed_string(object, &["notificationText", "notification_text"]);
    let reason = trimmed_string(object.get("reason"));
    let priority = match trimmed_string(object.get("priority")).as_deref() {
        Some("low") => Some(HeartbeatResponsePriority::Low),
        Some("normal") => Some(HeartbeatResponsePriority::Normal),
        Some("high") => Some(HeartbeatResponsePriority::High),
        _ => None,
    };
    let next_check = first_trimmed_string(object, &["nextCheck", "next_check"]);
    let scratch = object.get("scratch").and_then(Value::as_str).map(ToOwned::to_owned);
    if scratch
        .as_deref()
        .is_some_and(|content| content.len() > HEARTBEAT_SCRATCH_MAX_BYTES)
    {
        return None;
    }
    Some(HeartbeatResponse {
        outcome,
        notify,
        summary,
        notification_text,
        reason,
        priority,
        next_check,
        scratch,
    })
}

fn trimmed_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn first_trimmed_string(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| trimmed_string(object.get(*key)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatMode {
    Heartbeat,
    Message,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatPresentation {
    pub should_skip: bool,
    pub text: String,
    pub did_strip: bool,
}

/// Returns false for missing scratch, matching OpenClaw's conservative rule.
pub fn is_content_effectively_empty(content: Option<&str>) -> bool {
    let Some(content) = content else { return false };
    let mut in_html_comment = false;
    for raw_line in content.lines() {
        let mut line = raw_line;
        loop {
            let trimmed_start = line.trim_start();
            if !in_html_comment && !trimmed_start.starts_with("<!--") {
                break;
            }
            let search = if in_html_comment { line } else { trimmed_start };
            let Some(end) = search.find("-->") else {
                in_html_comment = true;
                line = "";
                break;
            };
            in_html_comment = false;
            line = &search[end + 3..];
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || is_empty_list_stub(trimmed)
            || is_fence_marker(trimmed)
        {
            continue;
        }
        return false;
    }
    true
}

fn is_empty_list_stub(value: &str) -> bool {
    let Some(rest) = value.strip_prefix(['-', '*', '+']) else {
        return false;
    };
    let rest = rest.trim();
    rest.is_empty() || rest == "[]" || rest == "[ ]" || rest == "[x]" || rest == "[X]"
}

fn is_fence_marker(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("```") else {
        return false;
    };
    rest.is_empty()
        || rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Resolves the configured heartbeat prompt and ensures response-tool guidance.
pub fn resolve_prompt_for_response_tool(raw: Option<&str>) -> String {
    let prompt = raw.map(str::trim).filter(|value| !value.is_empty());
    let mut resolved = prompt.unwrap_or(DEFAULT_HEARTBEAT_PROMPT).to_string();
    for instruction in [
        HEARTBEAT_RESPONSE_TOOL_INSTRUCTIONS,
        HEARTBEAT_RESPONSE_TOOL_FALLBACK_INSTRUCTIONS,
    ] {
        if !resolved.contains(instruction) {
            resolved.push_str("\n\n");
            resolved.push_str(instruction);
        }
    }
    resolved
}

/// Strips an acknowledgement token only at reply edges and decides visibility.
pub fn strip_heartbeat_token(
    raw: Option<&str>,
    mode: HeartbeatMode,
    max_ack_chars: usize,
    allow_truncated: bool,
) -> HeartbeatPresentation {
    let trimmed = raw.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return HeartbeatPresentation {
            should_skip: true,
            text: String::new(),
            did_strip: false,
        };
    }
    let upper = trimmed.to_ascii_uppercase();
    if mode == HeartbeatMode::Heartbeat
        && allow_truncated
        && upper.len() >= 4
        && upper.len() < HEARTBEAT_TOKEN.len()
        && HEARTBEAT_TOKEN.starts_with(&upper)
    {
        return HeartbeatPresentation {
            should_skip: true,
            text: String::new(),
            did_strip: true,
        };
    }

    let mut text = trimmed.to_string();
    let mut did_strip = false;
    if text.to_ascii_uppercase().starts_with(HEARTBEAT_TOKEN) {
        text = text[HEARTBEAT_TOKEN.len()..].trim_start().to_string();
        did_strip = true;
    } else if let Some(index) = upper.rfind(HEARTBEAT_TOKEN) {
        let suffix = &trimmed[index + HEARTBEAT_TOKEN.len()..];
        if suffix.chars().count() <= 4 && suffix.chars().all(|c| !c.is_alphanumeric()) {
            text = format!("{}{}", &trimmed[..index], suffix)
                .trim()
                .to_string();
            did_strip = true;
        }
    }
    let should_skip = did_strip
        && (text.is_empty()
            || (mode == HeartbeatMode::Heartbeat && text.chars().count() <= max_ack_chars));
    HeartbeatPresentation {
        should_skip,
        text: if should_skip { String::new() } else { text },
        did_strip,
    }
}

pub fn is_silent_reply(raw: Option<&str>) -> bool {
    raw.map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case(SILENT_REPLY_TOKEN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scratch_is_conservative_and_handles_markdown_noise() {
        assert!(!is_content_effectively_empty(None));
        assert!(is_content_effectively_empty(Some(
            "# Tasks\n- [ ]\n```markdown\n<!-- note -->"
        )));
        assert!(!is_content_effectively_empty(Some(
            "# Tasks\n- investigate outage"
        )));
    }

    #[test]
    fn prompt_injects_both_response_paths_once() {
        let prompt = resolve_prompt_for_response_tool(Some("check status"));
        assert!(prompt.starts_with("check status"));
        assert_eq!(
            prompt.matches(HEARTBEAT_RESPONSE_TOOL_INSTRUCTIONS).count(),
            1
        );
        assert_eq!(
            prompt
                .matches(HEARTBEAT_RESPONSE_TOOL_FALLBACK_INSTRUCTIONS)
                .count(),
            1
        );
    }

    #[test]
    fn heartbeat_ack_is_silent_but_message_text_is_preserved() {
        let heartbeat = strip_heartbeat_token(
            Some("HEARTBEAT_OK"),
            HeartbeatMode::Heartbeat,
            DEFAULT_HEARTBEAT_ACK_MAX_CHARS,
            true,
        );
        assert!(heartbeat.should_skip && heartbeat.did_strip);
        let message = strip_heartbeat_token(
            Some("HEARTBEAT_OK outage detected"),
            HeartbeatMode::Message,
            DEFAULT_HEARTBEAT_ACK_MAX_CHARS,
            false,
        );
        assert_eq!(message.text, "outage detected");
        assert!(!message.should_skip);
    }

    #[test]
    fn silent_reply_token_is_exact_and_case_insensitive() {
        assert!(is_silent_reply(Some(" no_reply ")));
        assert!(!is_silent_reply(Some("NO_REPLY: details")));
    }

    #[test]
    fn heartbeat_response_normalizes_visible_and_private_fields() {
        let response = normalize_heartbeat_response(&serde_json::json!({
            "outcome": "progress",
            "notify": true,
            "summary": "  work advanced  ",
            "notification_text": "  checkpoint reached ",
            "reason": "  dependency completed ",
            "priority": "high",
            "next_check": "10m",
            "scratch": "private state"
        }))
        .expect("valid heartbeat response");
        assert_eq!(response.outcome, HeartbeatResponseOutcome::Progress);
        assert!(response.notify);
        assert_eq!(response.summary, "work advanced");
        assert_eq!(response.notification_text.as_deref(), Some("checkpoint reached"));
        assert_eq!(response.reason.as_deref(), Some("dependency completed"));
        assert_eq!(response.priority, Some(HeartbeatResponsePriority::High));
        assert_eq!(response.next_check.as_deref(), Some("10m"));
        assert_eq!(response.scratch.as_deref(), Some("private state"));
    }

    #[test]
    fn heartbeat_response_rejects_invalid_required_fields_and_scratch() {
        assert!(normalize_heartbeat_response(&serde_json::json!({
            "outcome": "progress",
            "notify": true,
            "summary": ""
        }))
        .is_none());
        assert!(normalize_heartbeat_response(&serde_json::json!({
            "outcome": "unknown",
            "notify": false,
            "summary": "ok"
        }))
        .is_none());
        assert!(normalize_heartbeat_response(&serde_json::json!({
            "outcome": "done",
            "notify": false,
            "summary": "ok",
            "scratch": "x".repeat(HEARTBEAT_SCRATCH_MAX_BYTES + 1)
        }))
        .is_none());
    }

    #[test]
    fn heartbeat_response_omits_unknown_priority_without_inventing_identity() {
        let response = normalize_heartbeat_response(&serde_json::json!({
            "outcome": "needs_attention",
            "notify": true,
            "summary": "review needed",
            "priority": "urgent",
            "threadId": "native-thread-must-not-be-copied"
        }))
        .expect("valid response with unknown optional priority");
        assert_eq!(response.priority, None);
        assert!(response.scratch.is_none());
    }

    #[test]
    fn heartbeat_response_accepts_silent_no_change() {
        let response = normalize_heartbeat_response(&serde_json::json!({
            "outcome": "no_change",
            "notify": false,
            "summary": "nothing changed"
        }))
        .expect("valid silent heartbeat response");
        assert_eq!(response.outcome, HeartbeatResponseOutcome::NoChange);
        assert!(!response.notify);
        assert_eq!(response.notification_text, None);
    }
}
