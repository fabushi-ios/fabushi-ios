//! Provider-owned serialization for the strict transcript replay model.
//!
//! These functions remain pure provider-owner adapters. They make provider
//! capability and attachment failures explicit on the live typed replay path.

use serde_json::{Value, json};
use thiserror::Error;

use crate::extensions::session::typed_replay::{
    ReplayContentPart, ReplayMessage, ReplayRole, ReplayToolCall, ReplayToolResult,
    TypedReplayEntry,
};
use crate::extensions::attachments::replay_attachment_projection::provider_replay_attachment_url;
use super::cursor_inference_transport::{CursorCheckpointMessage, CursorCheckpointToolCall};

const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TypedReplaySerializationError {
    #[error("provider cannot serialize typed replay role {role}")]
    UnsupportedRole { role: &'static str },
    #[error("provider cannot serialize structured replay content at entry {index}")]
    UnsupportedStructuredContent { index: usize },
    #[error("provider cannot resolve replay attachment at entry {index}: {detail}")]
    UnsupportedAttachment { index: usize, detail: String },
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
    #[error("tool call identity {tool_call_id} is duplicated in the replay projection")]
    DuplicateToolCall { tool_call_id: String },
}

/// Serialize typed replay into OpenAI Responses input items. Attachments must
/// already contain a provider-usable URL; local paths are rejected rather than
/// silently converted to text.
pub fn serialize_responses_replay(
    entries: &[TypedReplayEntry],
) -> Result<Vec<Value>, TypedReplaySerializationError> {
    let mut output = Vec::new();
    let mut calls = ToolReplayState::default();
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            TypedReplayEntry::CompactionSummary(summary) => output.push(json!({
                "role": "user",
                "content": format!(
                    "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                    summary.summary
                )
            })),
            TypedReplayEntry::Message(message) => {
                serialize_responses_message(index, message, &mut output, &mut calls)?;
            }
            TypedReplayEntry::UserAttachment(attachment) => {
                // Responses input_image/input_file values are content parts;
                // keep the standalone transcript row inside a user message
                // just like attachments carried by a typed message.
                output.push(json!({
                    "role": "user",
                    "content": [responses_attachment(index, &attachment.value)?],
                }));
            }
        }
    }
    calls.finish()?;
    Ok(output)
}

/// Serialize typed replay into OpenAI-compatible Chat Completions messages.
/// Tool calls and results remain native provider messages and are never folded
/// into ordinary assistant text.
pub fn serialize_chat_replay(
    entries: &[TypedReplayEntry],
) -> Result<Vec<Value>, TypedReplaySerializationError> {
    let mut output = Vec::new();
    let mut calls = ToolReplayState::default();
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            TypedReplayEntry::CompactionSummary(summary) => output.push(json!({
                "role": "user",
                "content": format!(
                    "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                    summary.summary
                )
            })),
            TypedReplayEntry::Message(message) => {
                serialize_chat_message(index, message, &mut output, &mut calls)?;
            }
            TypedReplayEntry::UserAttachment(attachment) => {
                // Chat Completions accepts content parts only inside a user
                // message. A standalone attachment transcript row is still
                // one user turn, so preserve it as a one-part message rather
                // than emitting an invalid top-level content object.
                output.push(json!({
                    "role": "user",
                    "content": [chat_attachment(index, &attachment.value)?],
                }));
            }
        }
    }
    calls.finish()?;
    Ok(output)
}

/// Serialize typed replay into Cursor's existing checkpoint conversation.
/// Cursor's current protobuf has no attachment content part, so attachments
/// are rejected until that owner contract is extended and verified.
pub fn serialize_cursor_replay(
    entries: &[TypedReplayEntry],
) -> Result<Vec<CursorCheckpointMessage>, TypedReplaySerializationError> {
    let mut output = Vec::new();
    let mut calls = ToolReplayState::default();
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            TypedReplayEntry::CompactionSummary(summary) => {
                output.push(CursorCheckpointMessage::Text {
                    role: "user".into(),
                    text: format!(
                        "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                        summary.summary
                    ),
                });
            }
            TypedReplayEntry::Message(message) => {
                serialize_cursor_message(index, message, &mut output, &mut calls)?;
            }
            TypedReplayEntry::UserAttachment(attachment) => {
                return Err(TypedReplaySerializationError::UnsupportedAttachment {
                    index,
                    detail: format!(
                        "Cursor protobuf has no attachment content part ({})",
                        attachment.value
                    ),
                });
            }
        }
    }
    calls.finish()?;
    Ok(output)
}

pub fn serialize_claude_replay(
    entries: &[TypedReplayEntry],
) -> Result<String, TypedReplaySerializationError> {
    let mut sections = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            TypedReplayEntry::CompactionSummary(summary) => sections.push(format!(
                "USER: {COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                summary.summary
            )),
            TypedReplayEntry::Message(message) => {
                if !message.attachments.is_empty()
                    || !message.tool_calls.is_empty()
                    || message.tool_result.is_some()
                {
                    return Err(TypedReplaySerializationError::UnsupportedRole {
                        role: "typed-tool-or-attachment",
                    });
                }
                let text = message
                    .content
                    .iter()
                    .map(|part| match part {
                        ReplayContentPart::Text(text) => Ok(text.as_str()),
                        ReplayContentPart::Structured(_) => Err(
                            TypedReplaySerializationError::UnsupportedStructuredContent { index },
                        ),
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join("");
                let role = match message.role {
                    ReplayRole::User => "USER",
                    ReplayRole::Assistant => "ASSISTANT",
                    ReplayRole::Tool => {
                        return Err(TypedReplaySerializationError::UnsupportedRole {
                            role: "tool",
                        });
                    }
                };
                sections.push(format!("{role}: {text}"));
            }
            TypedReplayEntry::UserAttachment(_) => {
                return Err(TypedReplaySerializationError::UnsupportedAttachment {
                    index,
                    detail: "Claude Code CLI receives one text prompt and has no replay attachment contract".into(),
                });
            }
        }
    }
    Ok(sections.join("\n\n"))
}

fn serialize_cursor_message(
    index: usize,
    message: &ReplayMessage,
    output: &mut Vec<CursorCheckpointMessage>,
    calls: &mut ToolReplayState,
) -> Result<(), TypedReplaySerializationError> {
    if !message.attachments.is_empty() {
        return Err(TypedReplaySerializationError::UnsupportedAttachment {
            index,
            detail: "Cursor protobuf has no attachment content part".into(),
        });
    }
    let text = message
        .content
        .iter()
        .map(|part| match part {
            ReplayContentPart::Text(text) => Ok(text.as_str()),
            ReplayContentPart::Structured(_) => {
                Err(TypedReplaySerializationError::UnsupportedStructuredContent { index })
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("");
    if let Some(result) = message.tool_result.as_ref() {
        ensure_tool_result_pair(result, calls)?;
        output.push(CursorCheckpointMessage::ToolResult {
            tool_call_id: result.tool_call_id.clone(),
            tool_name: result.tool_name.clone(),
            result: result.result.clone(),
            is_error: result.is_error,
        });
        return Ok(());
    }
    match message.role {
        ReplayRole::User => output.push(CursorCheckpointMessage::Text {
            role: "user".into(),
            text,
        }),
        ReplayRole::Assistant if message.tool_calls.is_empty() => {
            output.push(CursorCheckpointMessage::Text {
                role: "assistant".into(),
                text,
            });
        }
        ReplayRole::Assistant => {
            for call in &message.tool_calls {
                register_tool_call(call, calls)?;
            }
            let tool_calls = message
                .tool_calls
                .iter()
                .map(|call| {
                    CursorCheckpointToolCall {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        args: call.arguments.to_string(),
                    }
                })
                .collect();
            output.push(CursorCheckpointMessage::AssistantTool { text, tool_calls });
        }
        ReplayRole::Tool => return Err(TypedReplaySerializationError::UnsupportedRole { role: "tool" }),
    }
    Ok(())
}

fn serialize_responses_message(
    index: usize,
    message: &ReplayMessage,
    output: &mut Vec<Value>,
    calls: &mut ToolReplayState,
) -> Result<(), TypedReplaySerializationError> {
    if let Some(result) = message.tool_result.as_ref() {
        ensure_tool_result_pair(result, calls)?;
        output.push(json!({
            "type": "function_call_output",
            "call_id": result.tool_call_id,
            "output": result.result.to_string(),
        }));
        return Ok(());
    }
    let role = responses_role(message.role)?;
    let content = responses_content(index, message)?;
    if !content.is_empty() || message.tool_calls.is_empty() {
        output.push(json!({"role": role, "content": content}));
    }
    for call in &message.tool_calls {
        register_tool_call(call, calls)?;
        output.push(responses_tool_call(call));
    }
    Ok(())
}

fn serialize_chat_message(
    index: usize,
    message: &ReplayMessage,
    output: &mut Vec<Value>,
    calls: &mut ToolReplayState,
) -> Result<(), TypedReplaySerializationError> {
    if let Some(result) = message.tool_result.as_ref() {
        ensure_tool_result_pair(result, calls)?;
        output.push(json!({
            "role": "tool",
            "tool_call_id": result.tool_call_id,
            "content": result.result.to_string(),
        }));
        return Ok(());
    }
    let role = chat_role(message.role)?;
    let content = chat_content(index, message)?;
    let mut item = json!({"role": role, "content": content});
    if !message.tool_calls.is_empty() {
        let tool_calls = message
            .tool_calls
            .iter()
            .map(|call| {
                register_tool_call(call, calls)?;
                Ok(json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    }
                }))
            })
            .collect::<Result<Vec<_>, TypedReplaySerializationError>>()?;
        item["tool_calls"] = Value::Array(tool_calls);
    }
    output.push(item);
    Ok(())
}

fn responses_role(role: ReplayRole) -> Result<&'static str, TypedReplaySerializationError> {
    match role {
        ReplayRole::User => Ok("user"),
        ReplayRole::Assistant => Ok("assistant"),
        ReplayRole::Tool => Err(TypedReplaySerializationError::UnsupportedRole { role: "tool" }),
    }
}

fn chat_role(role: ReplayRole) -> Result<&'static str, TypedReplaySerializationError> {
    match role {
        ReplayRole::User => Ok("user"),
        ReplayRole::Assistant => Ok("assistant"),
        ReplayRole::Tool => Err(TypedReplaySerializationError::UnsupportedRole { role: "tool" }),
    }
}

fn responses_content(
    index: usize,
    message: &ReplayMessage,
) -> Result<Vec<Value>, TypedReplaySerializationError> {
    let mut content = message
        .content
        .iter()
        .map(|part| match part {
            ReplayContentPart::Text(text) => Ok(json!({"type": "input_text", "text": text})),
            ReplayContentPart::Structured(_) => {
                Err(TypedReplaySerializationError::UnsupportedStructuredContent { index })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    for attachment in &message.attachments {
        content.push(responses_attachment(index, &attachment.value)?);
    }
    Ok(content)
}

fn chat_content(
    index: usize,
    message: &ReplayMessage,
) -> Result<Value, TypedReplaySerializationError> {
    let mut content = message
        .content
        .iter()
        .map(|part| match part {
            ReplayContentPart::Text(text) => Ok(json!({"type": "text", "text": text})),
            ReplayContentPart::Structured(_) => {
                Err(TypedReplaySerializationError::UnsupportedStructuredContent { index })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    for attachment in &message.attachments {
        content.push(chat_attachment(index, &attachment.value)?);
    }
    Ok(Value::Array(content))
}

fn responses_attachment(index: usize, value: &Value) -> Result<Value, TypedReplaySerializationError> {
    let url = attachment_url(index, value)?;
    let is_image = value
        .get("mimeType")
        .or_else(|| value.get("mime_type"))
        .and_then(Value::as_str)
        .is_some_and(|mime| mime.starts_with("image/"));
    Ok(if is_image {
        json!({"type": "input_image", "image_url": url})
    } else {
        json!({"type": "input_file", "file_url": url})
    })
}

fn chat_attachment(index: usize, value: &Value) -> Result<Value, TypedReplaySerializationError> {
    let url = attachment_url(index, value)?;
    let is_image = value
        .get("mimeType")
        .or_else(|| value.get("mime_type"))
        .and_then(Value::as_str)
        .is_some_and(|mime| mime.starts_with("image/"));
    if !is_image {
        return Err(TypedReplaySerializationError::UnsupportedAttachment {
            index,
            detail: "Chat Completions replay currently supports image attachments only".into(),
        });
    }
    Ok(json!({"type": "image_url", "image_url": {"url": url}}))
}

fn attachment_url(index: usize, value: &Value) -> Result<String, TypedReplaySerializationError> {
    provider_replay_attachment_url(value).map_err(|error| {
        TypedReplaySerializationError::UnsupportedAttachment {
            index,
            detail: error.to_string(),
        }
    })
}

fn responses_tool_call(call: &ReplayToolCall) -> Value {
    json!({
        "type": "function_call",
        "call_id": call.id,
        "name": call.name,
        "arguments": call.arguments.to_string(),
    })
}

#[derive(Debug, Default)]
struct ToolReplayState {
    pending: std::collections::BTreeMap<String, String>,
    consumed: std::collections::BTreeSet<String>,
}

impl ToolReplayState {
    fn register_call(
        &mut self,
        call: &ReplayToolCall,
    ) -> Result<(), TypedReplaySerializationError> {
        if self.pending.contains_key(&call.id) || self.consumed.contains(&call.id) {
            return Err(TypedReplaySerializationError::DuplicateToolCall {
                tool_call_id: call.id.clone(),
            });
        }
        self.pending.insert(call.id.clone(), call.name.clone());
        Ok(())
    }

    fn consume_result(
        &mut self,
        result: &ReplayToolResult,
    ) -> Result<(), TypedReplaySerializationError> {
        if self.consumed.contains(&result.tool_call_id) {
            return Err(TypedReplaySerializationError::DuplicateToolResult {
                tool_call_id: result.tool_call_id.clone(),
            });
        }
        let Some(expected_tool_name) = self.pending.remove(&result.tool_call_id) else {
            return Err(TypedReplaySerializationError::UnpairedToolResult {
                tool_call_id: result.tool_call_id.clone(),
            });
        };
        if expected_tool_name != result.tool_name {
            self.pending
                .insert(result.tool_call_id.clone(), expected_tool_name.clone());
            return Err(TypedReplaySerializationError::ToolResultNameMismatch {
                tool_call_id: result.tool_call_id.clone(),
                actual_tool_name: result.tool_name.clone(),
                expected_tool_name,
            });
        }
        self.consumed.insert(result.tool_call_id.clone());
        Ok(())
    }

    fn finish(self) -> Result<(), TypedReplaySerializationError> {
        self.pending
            .into_iter()
            .next()
            .map(|(tool_call_id, _)| {
                TypedReplaySerializationError::UnsettledToolCall { tool_call_id }
            })
            .map_or(Ok(()), Err)
    }
}

fn ensure_tool_result_pair(
    result: &ReplayToolResult,
    calls: &mut ToolReplayState,
) -> Result<(), TypedReplaySerializationError> {
    calls.consume_result(result)
}

fn register_tool_call(
    call: &ReplayToolCall,
    calls: &mut ToolReplayState,
) -> Result<(), TypedReplaySerializationError> {
    calls.register_call(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::session::typed_replay::ReplayCompactionSummary;

    #[test]
    fn responses_preserves_summary_and_balanced_tool_items() {
        let entries = vec![
            TypedReplayEntry::CompactionSummary(ReplayCompactionSummary {
                summary: "facts".into(),
                tokens_before: 10,
                details: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "call-1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                }],
                tool_result: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Tool,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![],
                tool_result: Some(ReplayToolResult {
                    tool_call_id: "call-1".into(),
                    tool_name: "read".into(),
                    result: json!({"ok": true}),
                    is_error: false,
                }),
            }),
        ];
        let serialized = serialize_responses_replay(&entries).expect("responses replay");
        assert!(serialized[0]["content"].as_str().unwrap().contains("<summary>"));
        assert_eq!(serialized[1]["type"], "function_call");
        assert_eq!(serialized[2]["type"], "function_call_output");
    }

    #[test]
    fn local_attachment_is_rejected_until_attachment_owner_resolves_it() {
        let entry = TypedReplayEntry::UserAttachment(crate::extensions::session::typed_replay::ReplayAttachment {
            value: json!({"file_path": "/tmp/a.png", "mimeType": "image/png"}),
        });
        let error = serialize_responses_replay(&[entry]).expect_err("local path must fail closed");
        assert!(matches!(error, TypedReplaySerializationError::UnsupportedAttachment { .. }));
    }

    #[test]
    fn cursor_preserves_native_tool_checkpoint_pairing() {
        let entries = vec![
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "cursor-call".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                }],
                tool_result: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Tool,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![],
                tool_result: Some(ReplayToolResult {
                    tool_call_id: "cursor-call".into(),
                    tool_name: "read".into(),
                    result: json!({"ok": true}),
                    is_error: false,
                }),
            }),
        ];
        let serialized = serialize_cursor_replay(&entries).expect("cursor replay");
        assert!(matches!(serialized[0], CursorCheckpointMessage::AssistantTool { .. }));
        assert!(matches!(serialized[1], CursorCheckpointMessage::ToolResult { .. }));
    }

    #[test]
    fn claude_rejects_typed_tool_rows_instead_of_flattening_them() {
        let entry = TypedReplayEntry::Message(ReplayMessage {
            id: None,
            role: ReplayRole::Assistant,
            content: vec![],
            attachments: vec![],
            tool_calls: vec![ReplayToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: json!({"path": "a"}),
            }],
            tool_result: None,
        });
        let error = serialize_claude_replay(&[entry]).expect_err("Claude must reject tool rows");
        assert_eq!(
            error,
            TypedReplaySerializationError::UnsupportedRole {
                role: "typed-tool-or-attachment"
            }
        );
    }

    fn duplicate_call_entries() -> Vec<TypedReplayEntry> {
        vec![
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "duplicate-call".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                }],
                tool_result: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "duplicate-call".into(),
                    name: "read".into(),
                    arguments: json!({"path": "b"}),
                }],
                tool_result: None,
            }),
        ]
    }

    fn duplicate_call_entries_in_one_message() -> Vec<TypedReplayEntry> {
        vec![TypedReplayEntry::Message(ReplayMessage {
            id: None,
            role: ReplayRole::Assistant,
            content: vec![],
            attachments: vec![],
            tool_calls: vec![
                ReplayToolCall {
                    id: "duplicate-call".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                },
                ReplayToolCall {
                    id: "duplicate-call".into(),
                    name: "write".into(),
                    arguments: json!({"path": "b"}),
                },
            ],
            tool_result: None,
        })]
    }

    fn assert_duplicate_call_rejected(entries: &[TypedReplayEntry]) {
        let responses = serialize_responses_replay(entries)
            .expect_err("Responses must reject duplicate call identity");
        let chat = serialize_chat_replay(entries)
            .expect_err("Chat Completions must reject duplicate call identity");
        let cursor = serialize_cursor_replay(entries)
            .expect_err("Cursor must reject duplicate call identity");
        for error in [responses, chat, cursor] {
            assert_eq!(
                error,
                TypedReplaySerializationError::DuplicateToolCall {
                    tool_call_id: "duplicate-call".into(),
                }
            );
        }
    }

    #[test]
    fn duplicate_tool_call_identity_is_rejected_by_all_typed_serializers() {
        assert_duplicate_call_rejected(&duplicate_call_entries());
        assert_duplicate_call_rejected(&duplicate_call_entries_in_one_message());
    }

    #[test]
    fn duplicate_tool_result_identity_is_rejected_by_all_typed_serializers() {
        let entries = vec![
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "call-once".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                }],
                tool_result: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Tool,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![],
                tool_result: Some(ReplayToolResult {
                    tool_call_id: "call-once".into(),
                    tool_name: "read".into(),
                    result: json!({"ok": true}),
                    is_error: false,
                }),
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Tool,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![],
                tool_result: Some(ReplayToolResult {
                    tool_call_id: "call-once".into(),
                    tool_name: "read".into(),
                    result: json!({"ok": "duplicate"}),
                    is_error: false,
                }),
            }),
        ];
        for error in [
            serialize_responses_replay(&entries).expect_err("Responses must reject duplicate result"),
            serialize_chat_replay(&entries).expect_err("Chat must reject duplicate result"),
            serialize_cursor_replay(&entries).expect_err("Cursor must reject duplicate result"),
        ] {
            assert_eq!(
                error,
                TypedReplaySerializationError::DuplicateToolResult {
                    tool_call_id: "call-once".into(),
                }
            );
        }
    }

    #[test]
    fn unsettled_tool_call_is_rejected_instead_of_replayed_without_result() {
        let entries = vec![TypedReplayEntry::Message(ReplayMessage {
            id: None,
            role: ReplayRole::Assistant,
            content: vec![],
            attachments: vec![],
            tool_calls: vec![ReplayToolCall {
                id: "missing-result".into(),
                name: "read".into(),
                arguments: json!({"path": "a"}),
            }],
            tool_result: None,
        })];
        for error in [
            serialize_responses_replay(&entries).expect_err("Responses must reject unsettled call"),
            serialize_chat_replay(&entries).expect_err("Chat must reject unsettled call"),
            serialize_cursor_replay(&entries).expect_err("Cursor must reject unsettled call"),
        ] {
            assert_eq!(
                error,
                TypedReplaySerializationError::UnsettledToolCall {
                    tool_call_id: "missing-result".into(),
                }
            );
        }
    }

    #[test]
    fn tool_result_name_mismatch_is_rejected_by_all_typed_serializers() {
        let entries = vec![
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Assistant,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![ReplayToolCall {
                    id: "named-call".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a"}),
                }],
                tool_result: None,
            }),
            TypedReplayEntry::Message(ReplayMessage {
                id: None,
                role: ReplayRole::Tool,
                content: vec![],
                attachments: vec![],
                tool_calls: vec![],
                tool_result: Some(ReplayToolResult {
                    tool_call_id: "named-call".into(),
                    tool_name: "write".into(),
                    result: json!({"ok": true}),
                    is_error: false,
                }),
            }),
        ];
        for error in [
            serialize_responses_replay(&entries).expect_err("Responses must reject name mismatch"),
            serialize_chat_replay(&entries).expect_err("Chat must reject name mismatch"),
            serialize_cursor_replay(&entries).expect_err("Cursor must reject name mismatch"),
        ] {
            assert_eq!(
                error,
                TypedReplaySerializationError::ToolResultNameMismatch {
                    tool_call_id: "named-call".into(),
                    actual_tool_name: "write".into(),
                    expected_tool_name: "read".into(),
                }
            );
        }
    }
}
