use serde_json::{Value, json};

use crate::runner::agent_v1_wire::{
    AgentToolCall, SendMessageArgs, SendMessageAttachment, SendMessageText,
    SendMessageToolCall, agent_tool_call, send_message_args,
};

use super::sand_permission_request::summarize_permission_request;
use super::sand_secret_request::summarize_secret_request;

fn wire_text(content: String) -> SendMessageArgs {
    SendMessageArgs {
        message: Some(send_message_args::Message::Text(SendMessageText { content })),
    }
}

pub fn encode_markdown_image_destination(url: &str) -> String {
    let mut output = String::new();
    for ch in url.chars() {
        match ch {
            '<' => output.push_str("%3C"),
            '>' => output.push_str("%3E"),
            '\r' => output.push_str("%0D"),
            '\n' => output.push_str("%0A"),
            _ => output.push(ch),
        }
    }
    format!("<{output}>")
}

pub fn encode_text_content(content: &str, images: Option<&[Value]>) -> String {
    let Some(images) = images.filter(|images| !images.is_empty()) else {
        return content.to_string();
    };
    let markdown = images
        .iter()
        .map(|image| {
            let alt = image
                .get("alt")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .map(|ch| if matches!(ch, '[' | ']' | '\n') { ' ' } else { ch })
                .collect::<String>()
                .trim()
                .to_string();
            let url = image
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("![{alt}]({})", encode_markdown_image_destination(url))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{content}\n\n{markdown}")
}

fn summarize_widget(widget: Option<&Value>) -> String {
    let prompt = widget
        .and_then(|widget| widget.get("prompt"))
        .and_then(Value::as_str)
        .unwrap_or("Question");
    let labels = widget
        .and_then(|widget| widget.get("options"))
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .map(|option| option.get("label").and_then(Value::as_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    if labels.is_empty() { prompt.to_string() } else { format!("{prompt} — {labels}") }
}

pub fn create_send_message_tool_call(tool_call: SendMessageToolCall) -> AgentToolCall {
    AgentToolCall {
        tool: Some(agent_tool_call::Tool::SendMessageToolCall(tool_call)),
        tool_call_id: None,
        started_at_ms: None,
        completed_at_ms: None,
    }
}

pub fn encode_send_message_wire(message: &Value) -> Result<SendMessageArgs, String> {
    let message_type = message
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "SendMessage message requires type".to_string())?;
    match message_type {
        "text" => Ok(wire_text(encode_text_content(
            message.get("content").and_then(Value::as_str).unwrap_or_default(),
            message.get("images").and_then(Value::as_array).map(Vec::as_slice),
        ))),
        "attachment" => Ok(SendMessageArgs {
            message: Some(send_message_args::Message::Attachment(
                SendMessageAttachment {
                    url: message.get("url").and_then(Value::as_str).unwrap_or_default().to_string(),
                    alt: message.get("alt").and_then(Value::as_str).map(str::to_string),
                },
            )),
        }),
        "widget" => Ok(wire_text(summarize_widget(message.get("widget")))),
        "cursor-agent" => {
            let bc_id = message.get("bcId").and_then(Value::as_str).unwrap_or_default();
            let title = message.get("title").and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty());
            Ok(wire_text(match title {
                Some(title) => format!("Referenced Cursor cloud agent {bc_id} ({title})"),
                None => format!("Referenced Cursor cloud agent {bc_id}"),
            }))
        }
        "secret-request" => {
            let label = message
                .get("secretRequest")
                .or_else(|| message.get("secret"))
                .and_then(|value| value.get("label"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(wire_text(summarize_secret_request(label)))
        }
        "permission-request" => {
            let permission = message.get("permission").unwrap_or(&Value::Null);
            Ok(wire_text(summarize_permission_request(
                permission.get("title").and_then(Value::as_str).unwrap_or_default(),
                permission.get("reason").and_then(Value::as_str).unwrap_or_default(),
            )))
        }
        "auto-review-approval" => {
            let approval = message.get("approval").unwrap_or(&Value::Null);
            Ok(wire_text(format!(
                "Auto-review requested approval for: {}. Status: {}.",
                approval.get("summary").and_then(Value::as_str).unwrap_or_default(),
                approval.get("status").and_then(Value::as_str).unwrap_or_default(),
            )))
        }
        "local-tool-permission" => {
            let ask = message.get("ask").unwrap_or(&Value::Null);
            Ok(wire_text(format!(
                "Asked the user for permission to use their computer ({}: {}). Status: {}.",
                ask.get("action").and_then(Value::as_str).unwrap_or_default(),
                ask.get("target").and_then(Value::as_str).unwrap_or_default(),
                ask.get("status").and_then(Value::as_str).unwrap_or_default(),
            )))
        }
        "connector" => {
            let connector = message.get("connector").and_then(Value::as_str).unwrap_or_default();
            Ok(wire_text(if message.get("variant").and_then(Value::as_str) == Some("connected") {
                format!("Confirmed the {connector} connector is connected")
            } else {
                format!("Asked the user to connect the {connector} connector")
            }))
        }
        "connectors" => {
            let connectors = message.get("connectors").and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            Ok(wire_text(format!("Asked the user to connect: {connectors}")))
        }
        "listener-connect" => Ok(wire_text(format!(
            "Asked the user to connect {} for listener routines",
            if message.get("platform").and_then(Value::as_str) == Some("slack") { "Slack" } else { "GitHub" }
        ))),
        "email-draft" => {
            let draft = message.get("draft").unwrap_or(&Value::Null);
            let from = draft.get("from").and_then(Value::as_str);
            let to = draft.get("to").and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let cc = draft.get("cc").and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
            let subject = draft.get("subject").and_then(Value::as_str).unwrap_or_default();
            let body = draft.get("body").and_then(Value::as_str).unwrap_or_default();
            Ok(wire_text(format!(
                "Draft email{}\nTo: {}{}\nSubject: {}\n\n{}",
                from.map(|value| format!("\nFrom: {value}")).unwrap_or_default(),
                to,
                cc.map(|value| format!("\nCc: {value}")).unwrap_or_default(),
                subject,
                body
            )))
        }
        "slack-draft" => {
            let draft = message.get("draft").unwrap_or(&Value::Null);
            let workspace = draft.get("workspace").and_then(Value::as_str);
            let target = draft.get("target").and_then(Value::as_str).unwrap_or_default();
            let thread = draft.get("thread").and_then(Value::as_str).unwrap_or("New message");
            let body = draft.get("body").and_then(Value::as_str).unwrap_or_default();
            Ok(wire_text(format!(
                "Draft Slack message{} to {}\nThread: {}\n\n{}",
                workspace.map(|value| format!(" in {value}")).unwrap_or_default(),
                target,
                thread,
                body
            )))
        }
        other => Err(format!("Unsupported send-message type: {other}")),
    }
}

fn send_message_args_to_json(args: &SendMessageArgs) -> Value {
    match args.message.as_ref() {
        Some(send_message_args::Message::Text(text)) => {
            json!({"message":{"case":"text","value":{"content":text.content}}})
        }
        Some(send_message_args::Message::Attachment(attachment)) => {
            json!({"message":{"case":"attachment","value":{"url":attachment.url,"alt":attachment.alt}}})
        }
        None => json!({"message":{"case":null,"value":null}}),
    }
}

pub fn encode_send_message(message: &Value) -> Result<Value, String> {
    encode_send_message_wire(message).map(|args| send_message_args_to_json(&args))
}

pub fn is_box_root_path(path: &str) -> bool {
    ["/workspace", "/home", "/root"]
        .iter()
        .any(|root| path == *root || path.starts_with(&format!("{root}/")))
}


pub fn image_mime_from_path(path: &str) -> Option<&'static str> {
    let extension = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    match extension.as_str() {
        "avif" => Some("image/avif"),
        "bmp" => Some("image/bmp"),
        "gif" => Some("image/gif"),
        "ico" => Some("image/x-icon"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn resolve_box_media_attachment<Download, PersistImage, PersistMedia>(
    box_path: &str,
    remote_box_has_desktop: bool,
    download: Download,
    persist_image: PersistImage,
    persist_media_bytes: PersistMedia,
) -> Option<String>
where
    Download: FnOnce(&str) -> Option<Vec<u8>>,
    PersistImage: FnOnce(&[u8], &str) -> Option<String>,
    PersistMedia: FnOnce(&str, &[u8]) -> Option<String>,
{
    if !remote_box_has_desktop || !is_box_root_path(box_path) {
        return None;
    }
    let data = download(box_path)?;
    if let Some(mime) = image_mime_from_path(box_path) {
        return persist_image(&data, mime);
    }
    let name = std::path::Path::new(box_path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment");
    persist_media_bytes(name, &data)
}
