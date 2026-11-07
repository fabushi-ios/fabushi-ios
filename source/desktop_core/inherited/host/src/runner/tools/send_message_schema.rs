use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const SEND_MESSAGE_TYPES: &[&str] =
    &["text", "attachment", "widget", "cursor-agent", "secret-request"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendMessageType {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "attachment")]
    Attachment,
    #[serde(rename = "widget")]
    Widget,
    #[serde(rename = "cursor-agent")]
    CursorAgent,
    #[serde(rename = "secret-request")]
    SecretRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageImage {
    pub url: String,
    #[serde(default)]
    pub alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageSecret {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub connector: String,
    pub field: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMessageInput {
    #[serde(rename = "type")]
    pub message_type: SendMessageType,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub images: Option<Vec<SendMessageImage>>,
    #[serde(default)]
    pub alt: Option<String>,
    #[serde(default, rename = "reply_to")]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub widget: Option<Value>,
    #[serde(default, rename = "bcId")]
    pub bc_id: Option<String>,
    #[serde(default)]
    pub secret: Option<SendMessageSecret>,
}

impl SendMessageInput {
    pub fn normalize(mut self) -> Self {
        fn trim(value: &mut Option<String>) {
            if let Some(current) = value.take() {
                *value = Some(current.trim().to_string());
            }
        }
        trim(&mut self.content);
        trim(&mut self.url);
        trim(&mut self.alt);
        trim(&mut self.reply_to);
        trim(&mut self.channel);
        trim(&mut self.bc_id);
        if let Some(images) = self.images.as_mut() {
            for image in images {
                image.url = image.url.trim().to_string();
                if let Some(alt) = image.alt.take() {
                    image.alt = Some(alt.trim().to_string());
                }
            }
        }
        if let Some(secret) = self.secret.as_mut() {
            secret.label = secret.label.trim().to_string();
            secret.connector = secret.connector.trim().to_string();
            secret.field = secret.field.trim().to_string();
            if let Some(description) = secret.description.take() {
                secret.description = Some(description.trim().to_string());
            }
        }
        if let Some(widget) = self.widget.take() {
            self.widget = Some(normalize_widget(widget));
        }
        self
    }
}

fn normalize_widget(value: Value) -> Value {
    let Some(object) = value.as_object() else {
        return value;
    };
    let mut normalized = serde_json::Map::new();
    for key in ["prompt", "helpText", "options", "allowCustom", "dismissOnMoveOn"] {
        let Some(raw) = object.get(key) else { continue; };
        let next = match key {
            "prompt" | "helpText" => raw
                .as_str()
                .map(|value| Value::String(value.trim().to_string()))
                .unwrap_or_else(|| raw.clone()),
            "options" => raw.as_array().map(|options| {
                Value::Array(options.iter().map(normalize_widget_option).collect())
            }).unwrap_or_else(|| raw.clone()),
            _ => raw.clone(),
        };
        normalized.insert(key.to_string(), next);
    }
    Value::Object(normalized)
}

fn normalize_widget_option(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return value.clone();
    };
    let mut normalized = serde_json::Map::new();
    for key in ["label", "value", "description", "style"] {
        let Some(raw) = object.get(key) else { continue; };
        let next = if matches!(key, "label" | "value" | "description") {
            raw.as_str()
                .map(|value| Value::String(value.trim().to_string()))
                .unwrap_or_else(|| raw.clone())
        } else {
            raw.clone()
        };
        normalized.insert(key.to_string(), next);
    }
    Value::Object(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMessageIssue {
    pub path: Vec<String>,
    pub message: String,
}

pub fn parse_send_message_input(value: &Value) -> Result<SendMessageInput, String> {
    serde_json::from_value::<SendMessageInput>(value.clone())
        .map(SendMessageInput::normalize)
        .map_err(|error| format!("invalid SendMessage input: {error}"))
}

pub fn is_valid_attachment_url(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "file" | "https"))
}

fn provided_string(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|value| !value.is_empty())
}

fn issue(field: &str, message: impl Into<String>) -> SendMessageIssue {
    SendMessageIssue {
        path: vec![field.to_string()],
        message: message.into(),
    }
}

fn widget_issue(path: Vec<String>, message: impl Into<String>) -> SendMessageIssue {
    SendMessageIssue {
        path,
        message: message.into(),
    }
}

fn validate_widget(widget: &Value) -> Vec<SendMessageIssue> {
    let mut issues = Vec::new();
    let Some(object) = widget.as_object() else {
        issues.push(widget_issue(vec!["widget".into()], "widget must be an object"));
        return issues;
    };

    match object.get("prompt") {
        Some(Value::String(prompt)) if !prompt.is_empty() => {}
        _ => issues.push(widget_issue(
            vec!["widget".into(), "prompt".into()],
            "widget prompt must be a non-empty string",
        )),
    }

    if let Some(help_text) = object.get("helpText") {
        if !help_text.as_str().is_some_and(|value| !value.is_empty()) {
            issues.push(widget_issue(
                vec!["widget".into(), "helpText".into()],
                "widget helpText must be a non-empty string when provided",
            ));
        }
    }

    let options = match object.get("options") {
        Some(Value::Array(options)) if (1..=6).contains(&options.len()) => Some(options),
        Some(Value::Array(options)) => {
            issues.push(widget_issue(
                vec!["widget".into(), "options".into()],
                format!("widget options must contain between 1 and 6 choices; got {}", options.len()),
            ));
            Some(options)
        }
        _ => {
            issues.push(widget_issue(
                vec!["widget".into(), "options".into()],
                "widget options must be an array with between 1 and 6 choices",
            ));
            None
        }
    };

    if let Some(options) = options {
        for (index, option) in options.iter().enumerate() {
            let prefix = vec!["widget".into(), "options".into(), index.to_string()];
            let Some(option) = option.as_object() else {
                let mut path = prefix.clone();
                issues.push(widget_issue(path, "widget option must be an object"));
                continue;
            };
            match option.get("label") {
                Some(Value::String(label)) if !label.is_empty() => {}
                _ => {
                    let mut path = prefix.clone();
                    path.push("label".into());
                    issues.push(widget_issue(path, "widget option label must be a non-empty string"));
                }
            }
            for key in ["value", "description"] {
                if let Some(raw) = option.get(key) {
                    if !raw.as_str().is_some_and(|value| !value.is_empty()) {
                        let mut path = prefix.clone();
                        path.push(key.into());
                        issues.push(widget_issue(
                            path,
                            format!("widget option {key} must be a non-empty string when provided"),
                        ));
                    }
                }
            }
            if let Some(style) = option.get("style") {
                if !style.as_str().is_some_and(|style| {
                    matches!(style, "default" | "primary" | "danger")
                }) {
                    let mut path = prefix.clone();
                    path.push("style".into());
                    issues.push(widget_issue(
                        path,
                        "widget option style must be default, primary, or danger",
                    ));
                }
            }
        }
    }

    for key in ["allowCustom", "dismissOnMoveOn"] {
        if let Some(raw) = object.get(key) {
            if !raw.is_boolean() {
                issues.push(widget_issue(
                    vec!["widget".into(), key.into()],
                    format!("widget {key} must be a boolean when provided"),
                ));
            }
        }
    }

    issues
}

pub fn refine_send_message(value: &SendMessageInput) -> Vec<SendMessageIssue> {
    let mut issues = Vec::new();
    let type_name = match value.message_type {
        SendMessageType::Text => "text",
        SendMessageType::Attachment => "attachment",
        SendMessageType::Widget => "widget",
        SendMessageType::CursorAgent => "cursor-agent",
        SendMessageType::SecretRequest => "secret-request",
    };

    let guarded = [
        ("content", provided_string(&value.content), &["text"][..]),
        ("url", provided_string(&value.url), &["attachment"][..]),
        ("alt", provided_string(&value.alt), &["attachment"][..]),
        ("widget", value.widget.is_some(), &["widget"][..]),
        ("bcId", provided_string(&value.bc_id), &["cursor-agent"][..]),
        ("secret", value.secret.is_some(), &["secret-request"][..]),
    ];
    for (field, provided, allowed) in guarded {
        if provided && !allowed.contains(&type_name) {
            let allowed = allowed
                .iter()
                .map(|name| format!("type:{name}"))
                .collect::<Vec<_>>()
                .join(" or ");
            issues.push(issue(
                field,
                format!(
                    "{field} is only valid with {allowed} and cannot ride a type:{type_name} message - it would be silently dropped. Nothing was sent. Re-send as separate SendMessage calls, one per type: this field on its own properly-typed message ({allowed}), and any text as its own type:text message."
                ),
            ));
        }
    }

    if provided_string(&value.channel)
        && !matches!(
            value.message_type,
            SendMessageType::Text | SendMessageType::Attachment
        )
    {
        issues.push(issue(
            "channel",
            "channel can only be set for type:text or type:attachment, not widgets or cursor-agent cards",
        ));
    }
    if value.images.as_ref().is_some_and(|images| !images.is_empty())
        && value.message_type != SendMessageType::Text
    {
        issues.push(issue(
            "images",
            "images can only be set for type:text; use type:attachment for a standalone attachment",
        ));
    }

    match value.message_type {
        SendMessageType::Text => {
            if !provided_string(&value.content) {
                issues.push(issue("content", "content is required when type is text"));
            }
            for (index, image) in value.images.as_deref().unwrap_or_default().iter().enumerate() {
                if !is_valid_attachment_url(&image.url) {
                    issues.push(SendMessageIssue {
                        path: vec!["images".into(), index.to_string(), "url".into()],
                        message: "each images url must include a file:// or https:// scheme".into(),
                    });
                }
            }
        }
        SendMessageType::Attachment => {
            let Some(url) = value.url.as_deref().filter(|value| !value.is_empty()) else {
                issues.push(issue("url", "url is required when type is attachment"));
                return issues;
            };
            if !is_valid_attachment_url(url) {
                issues.push(issue(
                    "url",
                    "url must include a file:// or https:// scheme when type is attachment",
                ));
            }
        }
        _ => {}
    }
    issues
}

pub fn validate_send_message(value: &SendMessageInput) -> Result<(), Vec<SendMessageIssue>> {
    let mut issues = refine_send_message(value);
    match value.message_type {
        SendMessageType::Widget => match value.widget.as_ref() {
            None => issues.push(issue("widget", "widget is required when type is widget")),
            Some(widget) => issues.extend(validate_widget(widget)),
        }
        SendMessageType::CursorAgent if !provided_string(&value.bc_id) => {
            issues.push(issue("bcId", "bcId is required when type is cursor-agent"));
        }
        SendMessageType::SecretRequest => match value.secret.as_ref() {
            None => issues.push(issue("secret", "secret is required when type is secret-request")),
            Some(secret)
                if secret.label.is_empty()
                    || secret.connector.is_empty()
                    || secret.field.is_empty() =>
            {
                issues.push(issue(
                    "secret",
                    "secret label, connector, and field must be non-empty",
                ));
            }
            _ => {}
        },
        _ => {}
    }
    if issues.is_empty() { Ok(()) } else { Err(issues) }
}

pub fn send_message_input_schema() -> Value {
    json!({
        "type": "object",
        "required": ["type"],
        "additionalProperties": false,
        "properties": {
            "type": {"type": "string", "enum": SEND_MESSAGE_TYPES},
            "content": {"type": "string"},
            "url": {"type": "string"},
            "images": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["url"],
                    "additionalProperties": false,
                    "properties": {
                        "url": {"type": "string"},
                        "alt": {"type": "string"}
                    }
                }
            },
            "alt": {"type": "string"},
            "reply_to": {"type": "string"},
            "channel": {"type": "string"},
            "widget": {
                "type": "object",
                "required": ["prompt", "options"],
                "additionalProperties": false,
                "properties": {
                    "prompt": {"type": "string", "minLength": 1},
                    "helpText": {"type": "string", "minLength": 1},
                    "options": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 6,
                        "items": {
                            "type": "object",
                            "required": ["label"],
                            "additionalProperties": false,
                            "properties": {
                                "label": {"type": "string", "minLength": 1},
                                "value": {"type": "string", "minLength": 1},
                                "description": {"type": "string", "minLength": 1},
                                "style": {"type": "string", "enum": ["default", "primary", "danger"]}
                            }
                        }
                    },
                    "allowCustom": {"type": "boolean"},
                    "dismissOnMoveOn": {"type": "boolean"}
                }
            },
            "bcId": {"type": "string"},
            "secret": {
                "type": "object",
                "required": ["label", "connector", "field"],
                "additionalProperties": false,
                "properties": {
                    "label": {"type": "string"},
                    "description": {"type": "string"},
                    "connector": {"type": "string"},
                    "field": {"type": "string"}
                }
            }
        }
    })
}
