use std::collections::HashSet;

use serde_json::{Value, json};

use crate::extensions::inference::cursor_inference_transport::CursorCheckpointMessage;
use crate::extensions::inference::provider_session::RoutedProviderCheckpoint;

use super::send_message_reminder_middleware::{
    CursorProviderOptions, MessageContent, MessageLike, MessagePart, ProviderOptions,
    SAND_SEND_MESSAGE_TOOL_NAME, is_injected_reminder_message,
};

pub const SAND_REACT_TO_MESSAGE_TOOL_NAME: &str = "ReactToMessage";

fn delivery_tool_names() -> [&'static str; 2] {
    [SAND_SEND_MESSAGE_TOOL_NAME, SAND_REACT_TO_MESSAGE_TOOL_NAME]
}

pub fn as_core_message(value: &Value) -> Option<MessageLike> {
    let object = value.as_object()?;
    let role = object.get("role")?.as_str()?.to_string();
    let content = match object.get("content")? {
        Value::String(text) => MessageContent::Text(text.clone()),
        Value::Array(parts) => MessageContent::Parts(
            parts
                .iter()
                .map(|part| {
                    let object = part.as_object();
                    MessagePart {
                        r#type: object
                            .and_then(|object| object.get("type"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        text: object
                            .and_then(|object| object.get("text"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        tool_name: object
                            .and_then(|object| object.get("toolName"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        tool_call_id: object
                            .and_then(|object| object.get("toolCallId"))
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        args: object.and_then(|object| object.get("args")).cloned(),
                    }
                })
                .collect(),
        ),
        _ => return None,
    };
    let cursor = object
        .get("providerOptions")
        .and_then(Value::as_object)
        .and_then(|provider| provider.get("cursor"))
        .and_then(Value::as_object)
        .map(|cursor| CursorProviderOptions {
            sand_send_message_reminder: cursor
                .get("sandSendMessageReminder")
                .and_then(Value::as_bool)
                == Some(true),
            sand_early_result_reminder: cursor
                .get("sandEarlyResultReminder")
                .and_then(Value::as_bool)
                == Some(true),
            sand_start_of_turn_ack_reminder: cursor
                .get("sandStartOfTurnAckReminder")
                .and_then(Value::as_bool)
                == Some(true),
            sand_disk_pressure_reminder: cursor
                .get("sandDiskPressureReminder")
                .and_then(Value::as_bool)
                == Some(true),
            sand_disk_pressure_reminder_episode_id: cursor
                .get("sandDiskPressureReminderEpisodeId")
                .and_then(Value::as_str)
                .map(str::to_string),
            high_level_tool_call_result: cursor.get("highLevelToolCallResult").cloned(),
        });
    Some(MessageLike {
        role,
        content,
        provider_options: cursor.map(|cursor| ProviderOptions {
            cursor: Some(cursor),
        }),
    })
}

pub fn tool_call_names(message: &MessageLike) -> Vec<String> {
    if message.role != "assistant" {
        return Vec::new();
    }
    let MessageContent::Parts(parts) = &message.content else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|part| part.r#type.as_deref() == Some("tool-call"))
        .filter_map(|part| part.tool_name.clone())
        .collect()
}

pub fn has_delivery_tool_call(names: &[String]) -> bool {
    names
        .iter()
        .any(|name| delivery_tool_names().contains(&name.as_str()))
}

fn delivery_tool_call_ids(message: &MessageLike) -> Vec<String> {
    if message.role != "assistant" {
        return Vec::new();
    }
    let MessageContent::Parts(parts) = &message.content else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|part| {
            part.r#type.as_deref() == Some("tool-call")
                && part
                    .tool_name
                    .as_deref()
                    .is_some_and(|name| delivery_tool_names().contains(&name))
        })
        .filter_map(|part| part.tool_call_id.clone())
        .collect()
}

fn errored_tool_result_ids(messages: &[Option<MessageLike>]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for message in messages.iter().flatten() {
        if message.role != "tool" {
            continue;
        }
        let is_error = message
            .provider_options
            .as_ref()
            .and_then(|options| options.cursor.as_ref())
            .and_then(|cursor| cursor.high_level_tool_call_result.as_ref())
            .and_then(Value::as_object)
            .and_then(|value| value.get("isError"))
            .and_then(Value::as_bool)
            == Some(true);
        if !is_error {
            continue;
        }
        let MessageContent::Parts(parts) = &message.content else {
            continue;
        };
        for part in parts {
            if part.r#type.as_deref() == Some("tool-result") {
                if let Some(id) = &part.tool_call_id {
                    ids.insert(id.clone());
                }
            }
        }
    }
    ids
}

pub fn is_blank_assistant_message(message: &MessageLike) -> bool {
    if message.role != "assistant" {
        return false;
    }
    match &message.content {
        MessageContent::Text(text) => text.trim().is_empty(),
        MessageContent::Parts(parts) => parts.iter().all(|part| {
            part.r#type.as_deref() == Some("text")
                && part.text.as_deref().unwrap_or("").trim().is_empty()
        }),
    }
}

pub fn turn_ended_on_silent_tool_calls(raw_messages: &[Value]) -> bool {
    let messages = raw_messages
        .iter()
        .map(as_core_message)
        .collect::<Vec<_>>();
    let mut tail_index = messages.len();
    while tail_index > 0 {
        let index = tail_index - 1;
        let skip = messages[index].as_ref().is_none_or(|message| {
            message.role == "tool"
                || is_blank_assistant_message(message)
                || is_injected_reminder_message(message)
        });
        if !skip {
            break;
        }
        tail_index -= 1;
    }
    if tail_index == 0 {
        return false;
    }
    let tail_index = tail_index - 1;
    let Some(tail) = messages[tail_index].as_ref() else {
        return false;
    };
    if tail.role != "assistant" {
        return false;
    }
    let tail_names = tool_call_names(tail);
    if tail_names.is_empty() || has_delivery_tool_call(&tail_names) {
        return false;
    }

    let mut boundary: isize = -1;
    for index in (0..tail_index).rev() {
        let Some(message) = messages[index].as_ref() else {
            continue;
        };
        if is_injected_reminder_message(message) {
            continue;
        }
        if matches!(message.role.as_str(), "user" | "system") {
            boundary = index as isize;
            break;
        }
    }

    let errored_ids = errored_tool_result_ids(&messages);
    let mut acked_first = false;
    for index in ((boundary + 1) as usize)..=tail_index {
        let Some(message) = messages[index].as_ref() else {
            continue;
        };
        if message.role != "assistant" {
            continue;
        }
        let names = tool_call_names(message);
        if names.is_empty() {
            continue;
        }
        if !acked_first {
            if !has_delivery_tool_call(&names) {
                return false;
            }
            acked_first = true;
        } else if delivery_tool_call_ids(message)
            .iter()
            .any(|id| !errored_ids.contains(id))
        {
            return false;
        }
    }
    acked_first
}


fn cursor_checkpoint_messages(checkpoint: &crate::extensions::inference::cursor_inference_transport::CursorCheckpoint) -> Vec<Value> {
    checkpoint
        .conversation
        .iter()
        .map(|message| match message {
            CursorCheckpointMessage::Text { role, text } => json!({
                "role": role,
                "content": text,
            }),
            CursorCheckpointMessage::AssistantTool { text, tool_calls } => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(json!({"type":"text","text":text}));
                }
                parts.extend(tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool-call",
                        "toolName": call.tool_name,
                        "toolCallId": call.tool_call_id,
                        "args": serde_json::from_str::<Value>(&call.args).unwrap_or(Value::Null),
                    })
                }));
                json!({"role":"assistant","content":parts})
            }
            CursorCheckpointMessage::ToolResult {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => json!({
                "role": "tool",
                "content": [{
                    "type": "tool-result",
                    "toolName": tool_name,
                    "toolCallId": tool_call_id,
                    "args": result,
                }],
                "providerOptions": {
                    "cursor": {
                        "highLevelToolCallResult": {"isError": is_error}
                    }
                }
            }),
        })
        .collect()
}

fn openrouter_checkpoint_messages(checkpoint: &crate::extensions::inference::provider_session::OpenRouterCheckpoint) -> Vec<Value> {
    checkpoint
        .conversation
        .iter()
        .filter_map(|message| {
            let role = message.get("role")?.as_str()?;
            if role == "assistant" {
                let mut parts = Vec::new();
                if let Some(text) = message.get("content").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        parts.push(json!({"type":"text","text":text}));
                    }
                }
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    parts.extend(calls.iter().filter_map(|call| {
                        let function = call.get("function")?;
                        Some(json!({
                            "type": "tool-call",
                            "toolName": function.get("name")?.as_str()?,
                            "toolCallId": call.get("id")?.as_str()?,
                            "args": function
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                                .unwrap_or(Value::Null),
                        }))
                    }));
                }
                return Some(json!({"role":"assistant","content":parts}));
            }
            if role == "tool" {
                let tool_call_id = message.get("tool_call_id").and_then(Value::as_str).unwrap_or_default();
                let result = message
                    .get("content")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .unwrap_or(Value::Null);
                let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
                return Some(json!({
                    "role":"tool",
                    "content":[{"type":"tool-result","toolCallId":tool_call_id}],
                    "providerOptions":{"cursor":{"highLevelToolCallResult":{"isError":is_error}}}
                }));
            }
            Some(json!({
                "role": role,
                "content": message.get("content").and_then(Value::as_str).unwrap_or_default(),
            }))
        })
        .collect()
}

fn codex_message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn codex_checkpoint_messages(checkpoint: &crate::extensions::inference::codex_direct_responses::CodexDirectCheckpoint) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut pending_calls = Vec::new();
    let flush_calls = |messages: &mut Vec<Value>, pending_calls: &mut Vec<Value>| {
        if !pending_calls.is_empty() {
            messages.push(json!({"role":"assistant","content":std::mem::take(pending_calls)}));
        }
    };
    for item in &checkpoint.input {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                pending_calls.push(json!({
                    "type":"tool-call",
                    "toolName": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "toolCallId": item.get("call_id").and_then(Value::as_str).unwrap_or_default(),
                    "args": item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .unwrap_or(Value::Null),
                }));
            }
            Some("function_call_output") => {
                flush_calls(&mut messages, &mut pending_calls);
                let result = item
                    .get("output")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .unwrap_or(Value::Null);
                let is_error = result.get("isError").and_then(Value::as_bool) == Some(true);
                messages.push(json!({
                    "role":"tool",
                    "content":[{
                        "type":"tool-result",
                        "toolCallId":item.get("call_id").and_then(Value::as_str).unwrap_or_default()
                    }],
                    "providerOptions":{"cursor":{"highLevelToolCallResult":{"isError":is_error}}}
                }));
            }
            Some("message") | None if item.get("role").and_then(Value::as_str).is_some() => {
                flush_calls(&mut messages, &mut pending_calls);
                messages.push(json!({
                    "role": item.get("role").and_then(Value::as_str).unwrap_or_default(),
                    "content": codex_message_text(item.get("content").unwrap_or(&Value::Null)),
                }));
            }
            _ => {}
        }
    }
    flush_calls(&mut messages, &mut pending_calls);
    messages
}

pub fn checkpoint_ended_on_silent_tool_calls(
    checkpoint: &RoutedProviderCheckpoint,
    final_text: &str,
) -> bool {
    if final_text.len() > checkpoint.emitted_text_bytes() {
        return false;
    }
    let messages = match checkpoint {
        RoutedProviderCheckpoint::Fabushi(checkpoint) => codex_checkpoint_messages(checkpoint),
        RoutedProviderCheckpoint::Cursor(checkpoint) => cursor_checkpoint_messages(checkpoint),
        RoutedProviderCheckpoint::Codex(checkpoint) => codex_checkpoint_messages(checkpoint),
        RoutedProviderCheckpoint::OpenRouter(checkpoint) => openrouter_checkpoint_messages(checkpoint),
    };
    turn_ended_on_silent_tool_calls(&messages)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::inference::codex_direct_responses::{
        CodexDirectCheckpoint, CodexDirectUsage,
    };

    fn responses_checkpoint() -> CodexDirectCheckpoint {
        CodexDirectCheckpoint {
            input: vec![
                serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": "finish the task",
                }),
                serde_json::json!({
                    "type": "function_call",
                    "name": SAND_SEND_MESSAGE_TOOL_NAME,
                    "call_id": "send-1",
                    "arguments": "{}",
                }),
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": "send-1",
                    "output": "{\"ok\":true}",
                }),
                serde_json::json!({
                    "type": "function_call",
                    "name": "ReadFile",
                    "call_id": "read-1",
                    "arguments": "{}",
                }),
                serde_json::json!({
                    "type": "function_call_output",
                    "call_id": "read-1",
                    "output": "{\"ok\":true}",
                }),
            ],
            text: String::new(),
            response_id: "resp-1".into(),
            usage: CodexDirectUsage::default(),
            completed_steps: 2,
            tool_calls_completed: 2,
        }
    }

    #[test]
    fn fabushi_checkpoint_preserves_responses_turn_shape_semantics() {
        let checkpoint = responses_checkpoint();
        let fabushi = checkpoint_ended_on_silent_tool_calls(
            &RoutedProviderCheckpoint::Fabushi(checkpoint.clone()),
            "",
        );
        let codex = checkpoint_ended_on_silent_tool_calls(
            &RoutedProviderCheckpoint::Codex(checkpoint),
            "",
        );

        assert!(fabushi);
        assert_eq!(fabushi, codex);
    }
}
