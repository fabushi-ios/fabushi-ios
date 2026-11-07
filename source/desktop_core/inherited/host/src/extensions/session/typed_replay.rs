//! Typed replay projection for compacted transcript entries.
//!
//! This is deliberately a transcript-owner boundary. It preserves structured
//! rows until the inference owner supplies a provider-specific serialization;
//! it never degrades an unsupported row to text.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum ReplayContentPart {
    Text(String),
    Structured(Value),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayAttachment {
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: Value,
    pub is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayMessage {
    pub id: Option<String>,
    pub role: ReplayRole,
    pub content: Vec<ReplayContentPart>,
    pub attachments: Vec<ReplayAttachment>,
    pub tool_calls: Vec<ReplayToolCall>,
    pub tool_result: Option<ReplayToolResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayCompactionSummary {
    pub summary: String,
    pub tokens_before: u64,
    pub details: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedReplayEntry {
    CompactionSummary(ReplayCompactionSummary),
    Message(ReplayMessage),
    UserAttachment(ReplayAttachment),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TypedReplayError {
    #[error("transcript replay entry {index} is not an object")]
    NonObject { index: usize },
    #[error("transcript replay entry {index} has unsupported kind {kind:?}")]
    UnsupportedKind { index: usize, kind: Option<String> },
    #[error("transcript replay entry {index} is malformed: {detail}")]
    Malformed { index: usize, detail: String },
    #[error("transcript replay entry {index} has a tool-call row without a typed pairing identity")]
    UnpairedToolCall { index: usize },
    #[error("tool call identity {tool_call_id} is duplicated in the replay projection")]
    DuplicateToolCall { tool_call_id: String },
    #[error("tool result {tool_call_id} has no matching replayed tool call")]
    UnpairedToolResult { tool_call_id: String },
    #[error("tool result {tool_call_id} is duplicated in the replay projection")]
    DuplicateToolResult { tool_call_id: String },
    #[error("tool result {tool_call_id} names {actual_tool_name}, expected {expected_tool_name}")]
    ToolResultNameMismatch {
        tool_call_id: String,
        actual_tool_name: String,
        expected_tool_name: String,
    },
    #[error("tool call {tool_call_id} has no replayed result")]
    UnsettledToolCall { tool_call_id: String },
}

/// Convert the canonical compacted replay rows into a lossless, provider-neutral
/// representation. Unsupported rows fail closed so callers cannot accidentally
/// turn transcript control records or tool state into ordinary prompt text.
pub fn project_typed_replay_entries(
    entries: &[Value],
) -> Result<Vec<TypedReplayEntry>, TypedReplayError> {
    let projected = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| project_entry(index, entry))
        .collect::<Result<Vec<_>, _>>()?;
    validate_tool_pairing(&projected)?;
    Ok(projected)
}

fn validate_tool_pairing(entries: &[TypedReplayEntry]) -> Result<(), TypedReplayError> {
    let mut pending = BTreeMap::<String, String>::new();
    let mut consumed = BTreeSet::<String>::new();
    for entry in entries {
        let TypedReplayEntry::Message(message) = entry else {
            continue;
        };
        for call in &message.tool_calls {
            if pending.contains_key(&call.id) || consumed.contains(&call.id) {
                return Err(TypedReplayError::DuplicateToolCall {
                    tool_call_id: call.id.clone(),
                });
            }
            pending.insert(call.id.clone(), call.name.clone());
        }
        let Some(result) = message.tool_result.as_ref() else {
            continue;
        };
        if consumed.contains(&result.tool_call_id) {
            return Err(TypedReplayError::DuplicateToolResult {
                tool_call_id: result.tool_call_id.clone(),
            });
        }
        let Some(expected_tool_name) = pending.remove(&result.tool_call_id) else {
            return Err(TypedReplayError::UnpairedToolResult {
                tool_call_id: result.tool_call_id.clone(),
            });
        };
        if expected_tool_name != result.tool_name {
            pending.insert(result.tool_call_id.clone(), expected_tool_name.clone());
            return Err(TypedReplayError::ToolResultNameMismatch {
                tool_call_id: result.tool_call_id.clone(),
                actual_tool_name: result.tool_name.clone(),
                expected_tool_name,
            });
        }
        consumed.insert(result.tool_call_id.clone());
    }
    if let Some((tool_call_id, _)) = pending.into_iter().next() {
        return Err(TypedReplayError::UnsettledToolCall { tool_call_id });
    }
    Ok(())
}

fn project_entry(index: usize, entry: &Value) -> Result<TypedReplayEntry, TypedReplayError> {
    let object = entry
        .as_object()
        .ok_or(TypedReplayError::NonObject { index })?;
    let kind = object.get("kind").and_then(Value::as_str);
    match kind {
        Some("compaction") => project_compaction(index, object),
        Some("message") => project_message(index, object).map(TypedReplayEntry::Message),
        Some("send-message") => {
            project_send_message(index, object).map(TypedReplayEntry::Message)
        }
        Some("user-attachment") => {
            required_string(index, object, "file_path")?;
            Ok(TypedReplayEntry::UserAttachment(ReplayAttachment {
                value: entry.clone(),
            }))
        }
        Some("tool-call") => Err(TypedReplayError::UnpairedToolCall { index }),
        _ => Err(TypedReplayError::UnsupportedKind {
            index,
            kind: kind.map(ToOwned::to_owned),
        }),
    }
}

fn project_compaction(
    index: usize,
    object: &serde_json::Map<String, Value>,
) -> Result<TypedReplayEntry, TypedReplayError> {
    let summary = required_string(index, object, "summary")?;
    let tokens_before = object
        .get("tokensBefore")
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed(index, "tokensBefore must be a non-negative integer"))?;
    Ok(TypedReplayEntry::CompactionSummary(
        ReplayCompactionSummary {
            summary,
            tokens_before,
            details: object.get("details").cloned(),
        },
    ))
}

fn project_send_message(
    index: usize,
    object: &serde_json::Map<String, Value>,
) -> Result<ReplayMessage, TypedReplayError> {
    let message = object
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(|| malformed(index, "send-message message must be an object"))?;
    if message.get("type").and_then(Value::as_str) != Some("text") {
        return Err(malformed(
            index,
            "send-message replay requires message.type text",
        ));
    }
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(index, "send-message text content must be a string"))?;
    if message
        .get("images")
        .is_some_and(|images| !images.as_array().is_some_and(Vec::is_empty))
    {
        return Err(malformed(
            index,
            "send-message images are not supported in typed replay",
        ));
    }
    Ok(ReplayMessage {
        id: object.get("id").and_then(Value::as_str).map(ToOwned::to_owned),
        role: ReplayRole::Assistant,
        content: vec![ReplayContentPart::Text(content.to_string())],
        attachments: Vec::new(),
        tool_calls: Vec::new(),
        tool_result: None,
    })
}

fn project_message(
    index: usize,
    object: &serde_json::Map<String, Value>,
) -> Result<ReplayMessage, TypedReplayError> {
    let role = match required_string(index, object, "role")?.as_str() {
        "user" => ReplayRole::User,
        "assistant" => ReplayRole::Assistant,
        "tool" => ReplayRole::Tool,
        role => {
            return Err(malformed(
                index,
                format!("unsupported message role {role:?}"),
            ));
        }
    };
    if role != ReplayRole::Assistant && object.get("toolCalls").is_some() {
        return Err(malformed(
            index,
            "toolCalls are only valid on assistant replay messages",
        ));
    }
    if role == ReplayRole::Tool && object.get("attachments").is_some() {
        return Err(malformed(
            index,
            "attachments are not valid on tool replay messages",
        ));
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let content = if role == ReplayRole::Tool {
        Vec::new()
    } else {
        object
            .get("content")
            .map(|value| match value {
                Value::String(text) => Ok(vec![ReplayContentPart::Text(text.clone())]),
                other => Ok(vec![ReplayContentPart::Structured(other.clone())]),
            })
            .transpose()?
            .unwrap_or_default()
    };
    let attachments: Vec<ReplayAttachment> = object
        .get("attachments")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| malformed(index, "attachments must be an array"))
                .map(|items| {
                    items
                        .iter()
                        .cloned()
                        .map(|value| ReplayAttachment { value })
                        .collect()
                })
        })
        .transpose()?
        .unwrap_or_default();
    let tool_calls = project_tool_calls(index, object)?;
    let tool_result = project_tool_result(index, object, role)?;
    if content.is_empty()
        && attachments.is_empty()
        && tool_calls.is_empty()
        && tool_result.is_none()
    {
        return Err(malformed(index, "message has no replayable content"));
    }
    Ok(ReplayMessage {
        id,
        role,
        content,
        attachments,
        tool_calls,
        tool_result,
    })
}

fn project_tool_calls(
    index: usize,
    object: &serde_json::Map<String, Value>,
) -> Result<Vec<ReplayToolCall>, TypedReplayError> {
    let Some(value) = object.get("toolCalls") else {
        return Ok(Vec::new());
    };
    let calls = value
        .as_array()
        .ok_or_else(|| malformed(index, "toolCalls must be an array"))?;
    calls
        .iter()
        .map(|call| {
            let call = call
                .as_object()
                .ok_or_else(|| malformed(index, "tool call must be an object"))?;
            let id = required_string(index, call, "id")?;
            let name = required_string(index, call, "name")?;
            let arguments = call
                .get("arguments")
                .or_else(|| call.get("args"))
                .cloned()
                .ok_or_else(|| malformed(index, "tool call is missing arguments"))?;
            Ok(ReplayToolCall {
                id,
                name,
                arguments,
            })
        })
        .collect()
}

fn project_tool_result(
    index: usize,
    object: &serde_json::Map<String, Value>,
    role: ReplayRole,
) -> Result<Option<ReplayToolResult>, TypedReplayError> {
    if role != ReplayRole::Tool {
        return Ok(None);
    }
    let tool_call_id = required_string(index, object, "toolCallId")?;
    let tool_name = required_string(index, object, "toolName")?;
    let result = object
        .get("result")
        .cloned()
        .or_else(|| object.get("content").cloned())
        .ok_or_else(|| malformed(index, "tool result is missing result/content"))?;
    Ok(Some(ReplayToolResult {
        tool_call_id,
        tool_name,
        result,
        is_error: object
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }))
}

fn required_string(
    index: usize,
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, TypedReplayError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| malformed(index, format!("{field} must be a non-empty string")))
}

fn malformed(index: usize, detail: impl Into<String>) -> TypedReplayError {
    TypedReplayError::Malformed {
        index,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_summary_roles_tool_pair_and_attachments() {
        let entries = vec![
            serde_json::json!({
                "kind": "compaction",
                "summary": "prior context",
                "tokensBefore": 42,
                "details": {"readFiles": ["src/lib.rs"]}
            }),
            serde_json::json!({
                "kind": "message",
                "id": "assistant-1",
                "role": "assistant",
                "content": "I will inspect it.",
                "toolCalls": [{
                    "id": "call-1",
                    "name": "read",
                    "arguments": {"path": "src/lib.rs"}
                }]
            }),
            serde_json::json!({
                "kind": "message",
                "id": "tool-1",
                "role": "tool",
                "toolCallId": "call-1",
                "toolName": "read",
                "result": {"text": "ok"}
            }),
            serde_json::json!({
                "kind": "message",
                "id": "user-1",
                "role": "user",
                "content": "continue",
                "attachments": [{"path": "/tmp/a.png", "mimeType": "image/png"}]
            }),
        ];

        let projected = project_typed_replay_entries(&entries).expect("typed replay");
        assert!(matches!(
            &projected[0],
            TypedReplayEntry::CompactionSummary(ReplayCompactionSummary {
                summary,
                tokens_before: 42,
                details: Some(_),
            }) if summary == "prior context"
        ));
        let TypedReplayEntry::Message(assistant) = &projected[1] else {
            panic!("assistant message was not preserved");
        };
        assert_eq!(assistant.role, ReplayRole::Assistant);
        assert_eq!(assistant.tool_calls[0].id, "call-1");
        let TypedReplayEntry::Message(tool) = &projected[2] else {
            panic!("tool result was not preserved");
        };
        assert_eq!(tool.tool_result.as_ref().unwrap().tool_call_id, "call-1");
        let TypedReplayEntry::Message(user) = &projected[3] else {
            panic!("user message was not preserved");
        };
        assert_eq!(user.attachments.len(), 1);
    }

    #[test]
    fn rejects_legacy_unpaired_tool_call_instead_of_stringifying_it() {
        let error = project_typed_replay_entries(&[serde_json::json!({
            "kind": "tool-call",
            "name": "read",
            "summary": "legacy row"
        })])
        .expect_err("legacy tool-call must fail closed");
        assert_eq!(error, TypedReplayError::UnpairedToolCall { index: 0 });
    }

    #[test]
    fn rejects_tool_calls_on_non_assistant_replay_messages() {
        let error = project_typed_replay_entries(&[serde_json::json!({
            "kind": "message",
            "role": "user",
            "content": "not a tool call owner",
            "toolCalls": [{
                "id": "call-1",
                "name": "read",
                "arguments": {"path": "a"}
            }]
        })])
        .expect_err("non-assistant tool calls must fail closed");
        assert_eq!(
            error,
            TypedReplayError::Malformed {
                index: 0,
                detail: "toolCalls are only valid on assistant replay messages".into(),
            }
        );
    }

    #[test]
    fn rejects_attachments_on_tool_result_messages() {
        let error = project_typed_replay_entries(&[serde_json::json!({
            "kind": "message",
            "role": "tool",
            "toolCallId": "call-1",
            "toolName": "read",
            "result": {"ok": true},
            "attachments": [{"file_path": "/tmp/result.txt"}]
        })])
        .expect_err("tool result attachments must fail closed");
        assert_eq!(
            error,
            TypedReplayError::Malformed {
                index: 0,
                detail: "attachments are not valid on tool replay messages".into(),
            }
        );
    }

    #[test]
    fn tool_result_content_fallback_is_not_replayed_as_an_extra_content_part() {
        let projected = project_typed_replay_entries(&[
            serde_json::json!({
                "kind": "message",
                "role": "assistant",
                "content": "Calling read",
                "toolCalls": [{
                    "id": "call-1",
                    "name": "read",
                    "arguments": {"path": "src/lib.rs"}
                }]
            }),
            serde_json::json!({
                "kind": "message",
                "role": "tool",
                "toolCallId": "call-1",
                "toolName": "read",
                "content": {"ok": true}
            }),
        ])
        .expect("tool result content fallback");
        let TypedReplayEntry::Message(message) = &projected[1] else {
            panic!("tool result was not projected as a message");
        };
        assert!(message.content.is_empty());
        assert_eq!(
            message.tool_result.as_ref().map(|result| &result.result),
            Some(&serde_json::json!({"ok": true}))
        );
    }
}
