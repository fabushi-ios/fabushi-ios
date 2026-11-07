use chrono::{SecondsFormat, TimeZone, Utc};
use serde_json::{Map, Value};
use url::Url;

use crate::selected_image_inputs::{
    SelectedImageInput, image_mime_from_path, load_selected_image_inputs,
    video_mime_from_path,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserMessageOptions {
    pub composed_at_ms: Option<f64>,
    pub rich_text: Option<String>,
    pub reply_to: Option<String>,
    pub batch_id: Option<String>,
    pub branched: bool,
    pub client_nonce: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserAttachmentOptions {
    pub file_name: Option<String>,
    pub batch_id: Option<String>,
    pub client_nonce: Option<String>,
    pub byte_size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub reply_to: Option<String>,
    pub branched: bool,
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: String,
    pub by: String,
}

pub fn toggle_reaction(
    reactions: Option<&[Reaction]>,
    emoji: &str,
    by: &str,
) -> Option<Vec<Reaction>> {
    let current = reactions.unwrap_or(&[]);
    let has = current
        .iter()
        .any(|reaction| reaction.emoji == emoji && reaction.by == by);
    let next = if has {
        current
            .iter()
            .filter(|reaction| !(reaction.emoji == emoji && reaction.by == by))
            .cloned()
            .collect::<Vec<_>>()
    } else {
        let mut next = current.to_vec();
        next.push(Reaction {
            emoji: emoji.to_string(),
            by: by.to_string(),
        });
        next
    };
    (!next.is_empty()).then_some(next)
}

pub fn build_composed_offline_note(composed_at_ms: f64) -> String {
    if !composed_at_ms.is_finite() {
        return String::new();
    }
    let millis = composed_at_ms.trunc();
    if millis < i64::MIN as f64 || millis > i64::MAX as f64 {
        return String::new();
    }
    Utc.timestamp_millis_opt(millis as i64)
        .single()
        .map(|date| {
            format!(
                "[Composed offline at {}]",
                date.to_rfc3339_opts(SecondsFormat::Millis, true)
            )
        })
        .unwrap_or_default()
}

pub fn skippable_prompt_summary(message: &Value) -> Option<String> {
    if message.get("type").and_then(Value::as_str) != Some("widget") {
        return None;
    }
    let widget = message.get("widget").and_then(Value::as_object);
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
                .map(|option| {
                    option
                        .get("label")
                        .map(js_join_string)
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .unwrap_or_default();
    Some(if labels.is_empty() {
        prompt.to_string()
    } else {
        format!("{prompt} — {labels}")
    })
}

fn js_join_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) | Value::Object(_) => "[object Object]".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttachmentChannels {
    pub image_attachment_paths: Vec<String>,
    pub video_attachment_paths: Vec<String>,
    pub file_attachment_paths: Vec<String>,
}

pub fn split_attachment_paths_by_channel<I, S>(paths: I) -> AttachmentChannels
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut channels = AttachmentChannels::default();
    for path in paths {
        let path = path.as_ref();
        if image_mime_from_path(path).is_some() {
            channels.image_attachment_paths.push(path.to_string());
        } else if video_mime_from_path(path).is_some() {
            channels.video_attachment_paths.push(path.to_string());
        } else {
            channels.file_attachment_paths.push(path.to_string());
        }
    }
    channels
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedVideo {
    pub path: String,
    pub mime_type: String,
    pub filename: String,
    pub fps: u32,
}

pub fn build_selected_videos<I, S>(paths: I) -> Vec<SelectedVideo>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    paths
        .into_iter()
        .map(|path| {
            let path = path.as_ref();
            SelectedVideo {
                path: path.to_string(),
                mime_type: video_mime_from_path(path)
                    .unwrap_or("video/mp4")
                    .to_string(),
                filename: std::path::Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string(),
                fps: 4,
            }
        })
        .collect()
}

pub fn shape_send_prompt_media_args(args: &Value) -> Value {
    let Some(object) = args.as_object() else {
        return args.clone();
    };
    let Some(raw_paths) = object.get("attachmentPaths").and_then(Value::as_array) else {
        return args.clone();
    };
    let Some(paths) = raw_paths
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
    else {
        return args.clone();
    };

    let names = object
        .get("attachmentNames")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| value.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let channels = split_attachment_paths_by_channel(paths.iter().copied());
    let selected_images = load_selected_image_inputs(&channels.image_attachment_paths);
    let selected_videos = build_selected_videos(&channels.video_attachment_paths);

    let mut file_names = Vec::with_capacity(channels.file_attachment_paths.len());
    for (index, path) in paths.iter().enumerate() {
        if image_mime_from_path(path).is_some() || video_mime_from_path(path).is_some() {
            continue;
        }
        let name = names
            .get(index)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
                    .to_string()
            });
        file_names.push(name);
    }

    let mut shaped = object.clone();
    shaped.insert(
        "attachmentPaths".into(),
        Value::Array(
            channels
                .file_attachment_paths
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    shaped.insert(
        "attachmentNames".into(),
        Value::Array(file_names.into_iter().map(Value::String).collect()),
    );

    if !channels.image_attachment_paths.is_empty() {
        shaped.insert(
            "selectedImages".into(),
            Value::Array(
                selected_images
                    .into_iter()
                    .map(|image| {
                        let mut value = Map::new();
                        value.insert(
                            "data".into(),
                            Value::Array(
                                image
                                    .data
                                    .into_iter()
                                    .map(|byte| Value::Number(u64::from(byte).into()))
                                    .collect(),
                            ),
                        );
                        value.insert("path".into(), Value::String(image.path));
                        if let Some(mime_type) = image.mime_type {
                            value.insert(
                                "mimeType".into(),
                                Value::String(mime_type.to_string()),
                            );
                        }
                        Value::Object(value)
                    })
                    .collect(),
            ),
        );
    }

    if !channels.video_attachment_paths.is_empty() {
        shaped.insert(
            "selectedVideos".into(),
            Value::Array(
                selected_videos
                    .into_iter()
                    .map(|video| {
                        let mut value = Map::new();
                        value.insert("path".into(), Value::String(video.path));
                        value.insert("mimeType".into(), Value::String(video.mime_type));
                        value.insert("filename".into(), Value::String(video.filename));
                        value.insert("fps".into(), Value::Number(video.fps.into()));
                        Value::Object(value)
                    })
                    .collect(),
            ),
        );
    }

    Value::Object(shaped)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundImage {
    pub data: String,
    pub mime_type: String,
}

pub fn collect_inbound_images(envelopes: &[Value]) -> Vec<InboundImage> {
    envelopes
        .iter()
        .filter_map(|envelope| envelope.get("images").and_then(Value::as_array))
        .flatten()
        .filter_map(|image| {
            Some(InboundImage {
                data: image.get("data")?.as_str()?.to_string(),
                mime_type: image.get("mimeType")?.as_str()?.to_string(),
            })
        })
        .collect()
}

pub fn load_agent_inbound_images(images: Option<&[Value]>) -> Vec<SelectedImageInput> {
    let paths = images
        .unwrap_or(&[])
        .iter()
        .filter_map(|image| image.get("url").and_then(Value::as_str))
        .filter_map(|url| Url::parse(url).ok())
        .filter_map(|url| url.to_file_path().ok())
        .filter_map(|path| path.to_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    load_selected_image_inputs(paths)
}

pub fn is_user_message_entry(entry: &Value) -> bool {
    match entry.get("kind").and_then(Value::as_str) {
        Some("message") => entry.get("role").and_then(Value::as_str) == Some("user"),
        Some("user-attachment") => true,
        _ => false,
    }
}

pub fn quotable_entry_text(entry: &Value) -> String {
    if entry.get("kind").and_then(Value::as_str) == Some("message") {
        return entry.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
    }
    if entry.get("kind").and_then(Value::as_str) == Some("send-message")
        && entry.get("message").and_then(|message| message.get("type")).and_then(Value::as_str) == Some("text")
    {
        return entry.get("message").and_then(|message| message.get("content")).and_then(Value::as_str).unwrap_or_default().to_string();
    }
    String::new()
}

pub fn describe_message_quote(entry: &Value, max_chars: Option<usize>) -> String {
    let normalized = quotable_entry_text(entry).split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    }
    if let Some(max_chars) = max_chars {
        if normalized.chars().count() > max_chars {
            return format!("{}…", normalized.chars().take(max_chars).collect::<String>());
        }
    }
    normalized
}

pub fn describe_reacted_message_quote(entry: &Value) -> String {
    describe_message_quote(entry, Some(80))
}

pub fn describe_replied_message_quote(entry: &Value) -> String {
    describe_message_quote(entry, None)
}

pub fn create_user_message(
    id: impl Into<String>,
    content: impl Into<String>,
    options: UserMessageOptions,
    now_ms: f64,
) -> Value {
    let mut entry = Map::new();
    entry.insert("kind".into(), Value::String("message".into()));
    entry.insert("id".into(), Value::String(id.into()));
    entry.insert("role".into(), Value::String("user".into()));
    entry.insert("content".into(), Value::String(content.into()));
    entry.insert("isStreaming".into(), Value::Bool(false));
    let composed_at_ms = options.composed_at_ms.filter(|value| value.is_finite());
    insert_f64(&mut entry, "timestampMs", composed_at_ms.unwrap_or(now_ms));
    if let Some(rich_text) = options.rich_text.filter(|value| !value.is_empty()) {
        entry.insert("richText".into(), Value::String(rich_text));
    }
    if let Some(reply_to) = options.reply_to { entry.insert("replyTo".into(), Value::String(reply_to)); }
    if let Some(batch_id) = options.batch_id { entry.insert("batchId".into(), Value::String(batch_id)); }
    if options.branched { entry.insert("branched".into(), Value::Bool(true)); }
    if let Some(client_nonce) = options.client_nonce.filter(|value| !value.is_empty()) {
        entry.insert("clientNonce".into(), Value::String(client_nonce));
    }
    if let Some(composed_at_ms) = composed_at_ms { insert_f64(&mut entry, "sentWhileOfflineAtMs", composed_at_ms); }
    Value::Object(entry)
}

pub fn create_send_message_entry(id: impl Into<String>, message: Value, timestamp_ms: f64) -> Value {
    let mut entry = Map::new();
    entry.insert("kind".into(), Value::String("send-message".into()));
    entry.insert("id".into(), Value::String(id.into()));
    if let Some(reply_to) = message.get("reply_to").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        entry.insert("replyTo".into(), Value::String(reply_to.to_string()));
    }
    entry.insert("message".into(), message);
    insert_f64(&mut entry, "timestampMs", timestamp_ms);
    Value::Object(entry)
}

pub fn stamp_box_request_entry(entry: &Value, request_id: &str, instruction: &str) -> Value {
    let mut next = entry.as_object().cloned().unwrap_or_default();
    next.insert("boxRequestId".into(), Value::String(request_id.to_string()));
    next.insert("boxInstruction".into(), Value::String(instruction.to_string()));
    Value::Object(next)
}

pub fn create_user_attachment_entry(
    id: impl Into<String>,
    file_path: impl Into<String>,
    options: UserAttachmentOptions,
) -> Value {
    let mut entry = Map::new();
    entry.insert("kind".into(), Value::String("user-attachment".into()));
    entry.insert("id".into(), Value::String(id.into()));
    entry.insert("file_path".into(), Value::String(file_path.into()));
    if let Some(file_name) = options.file_name.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()) {
        entry.insert("file_name".into(), Value::String(file_name));
    }
    if let Some(batch_id) = options.batch_id { entry.insert("batchId".into(), Value::String(batch_id)); }
    if let Some(client_nonce) = options.client_nonce.filter(|value| !value.is_empty()) {
        entry.insert("clientNonce".into(), Value::String(client_nonce));
    }
    if let Some(byte_size) = options.byte_size { entry.insert("byteSize".into(), Value::Number(byte_size.into())); }
    if let Some(width) = options.width { entry.insert("width".into(), Value::Number(width.into())); }
    if let Some(height) = options.height { entry.insert("height".into(), Value::Number(height.into())); }
    if let Some(reply_to) = options.reply_to { entry.insert("replyTo".into(), Value::String(reply_to)); }
    if options.branched { entry.insert("branched".into(), Value::Bool(true)); }
    Value::Object(entry)
}

pub fn stat_attached_file_size(path: &str) -> Option<u64> {
    std::fs::metadata(path).ok().filter(|metadata| metadata.is_file()).map(|metadata| metadata.len())
}

pub fn stat_attached_file_sizes<I, S>(paths: I) -> std::collections::HashMap<String, u64>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    paths
        .into_iter()
        .filter_map(|path| {
            let path = path.as_ref();
            stat_attached_file_size(path).map(|size| (path.to_string(), size))
        })
        .collect()
}


pub fn strip_reply_to(message: &Value) -> Value {
    if protected_reply_type(message) { return message.clone(); }
    shape_threadable(message, None)
}

pub fn with_reply_to(message: &Value, reply_to: &str) -> Value {
    if protected_reply_type(message) { return message.clone(); }
    shape_threadable(message, Some(reply_to))
}

fn protected_reply_type(message: &Value) -> bool {
    matches!(
        message.get("type").and_then(Value::as_str),
        Some("auto-review-approval") | Some("local-tool-permission") | Some("email-draft") | Some("slack-draft")
    )
}

fn shape_threadable(message: &Value, reply_to: Option<&str>) -> Value {
    let Some(kind) = message.get("type").and_then(Value::as_str) else { return message.clone(); };
    let fields: &[&str] = match kind {
        "text" => &["content", "images"],
        "widget" => &["widget"],
        "cursor-agent" => &["bcId"],
        "secret-request" => &["secretRequest"],
        "permission-request" => &["permission"],
        "connector" => &["connector", "variant", "serverId", "reason", "suggestions"],
        "connectors" => &["connectors", "reason"],
        "listener-connect" => &["platform", "reason"],
        "attachment" => &["url", "file_name", "alt", "channel", "width", "height"],
        _ => return message.clone(),
    };
    let mut shaped = Map::new();
    shaped.insert("type".into(), Value::String(kind.to_string()));
    for field in fields {
        if let Some(value) = message.get(*field).filter(|value| !value.is_null()) {
            shaped.insert((*field).to_string(), value.clone());
        }
    }
    if let Some(reply_to) = reply_to { shaped.insert("reply_to".into(), Value::String(reply_to.to_string())); }
    Value::Object(shaped)
}

fn insert_f64(map: &mut Map<String, Value>, key: &str, value: f64) {
    if let Some(number) = serde_json::Number::from_f64(value) {
        map.insert(key.to_string(), Value::Number(number));
    }
}
