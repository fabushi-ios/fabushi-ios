use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use url::Url;

use prost::Message;
use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

use super::box_help_tool::{BoxHelpOutcome, BoxHelpRequest};
use super::send_message_encoding::{
    create_send_message_tool_call, encode_send_message_wire,
};
use crate::runner::agent_v1_wire::{
    AgentToolCall, SendMessageError, SendMessageResult, SendMessageSuccess, SendMessageToolCall,
    send_message_result,
};
use crate::runner::turn_observation::TurnObservationHandle;
use super::send_message_schema::{
    SendMessageInput, SendMessageType, parse_send_message_input, send_message_input_schema,
    validate_send_message,
};
use super::sand_secret_request::{clamp_secret_description, clamp_secret_label};

pub const SAND_SEND_MESSAGE_TOOL_NAME: &str = "SendMessage";
pub const SAND_AWAITING_USER_SEND_MESSAGE_BLOCKED: &str =
    "This turn is already waiting on the user (you sent a question widget or handed the box back to them), so this message was not delivered. Wait for the user — their response arrives as the next message — then say this on your next turn.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAttachmentSource {
    pub url: String,
    pub file_name: Option<String>,
}

pub fn file_path_from_file_url(raw: &str) -> Option<PathBuf> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

pub fn identity_attachment_source(raw: &str) -> ResolvedAttachmentSource {
    let file_name = file_path_from_file_url(raw)
        .and_then(|path| path.file_name().map(|value| value.to_string_lossy().into_owned()))
        .filter(|value| !value.is_empty());
    ResolvedAttachmentSource {
        url: raw.to_string(),
        file_name,
    }
}

pub trait SendMessageSink: Send + Sync {
    fn is_awaiting_user_selection(&self) -> bool {
        false
    }

    fn resolve_cloud_agent_title(&self, _bc_id: &str) -> Option<String> {
        None
    }

    fn request_box_help(
        &self,
        _request: BoxHelpRequest,
        _timestamp_ms: u64,
        _tool_call_id: &str,
    ) -> Result<BoxHelpOutcome, ProviderSessionError> {
        Err(ProviderSessionError::Tool(
            "request_box_help is unavailable for this Runner".into(),
        ))
    }

    fn resolve_attachment_source(
        &self,
        source_url: &str,
        _tool_call_id: &str,
    ) -> Result<ResolvedAttachmentSource, ProviderSessionError> {
        Ok(identity_attachment_source(source_url))
    }

    fn read_media_dimensions(&self, _resolved_url: &str) -> Option<(u32, u32)> {
        None
    }

    fn abort_attachment_custody(&self, _tool_call_id: &str) {}

    fn send_message(
        &self,
        message: Value,
        timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError>;
}

#[derive(Clone, Default)]
pub struct SendMessageDeliveryCounter {
    sent_message_count: Arc<AtomicU64>,
}

impl SendMessageDeliveryCounter {
    pub fn count(&self) -> u64 {
        self.sent_message_count.load(Ordering::SeqCst)
    }

    fn record_success(&self) {
        self.sent_message_count.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct CountingSendMessageSink {
    delegate: Arc<dyn SendMessageSink>,
    counter: SendMessageDeliveryCounter,
}

impl CountingSendMessageSink {
    pub fn new(
        delegate: Arc<dyn SendMessageSink>,
        counter: SendMessageDeliveryCounter,
    ) -> Self {
        Self { delegate, counter }
    }
}

impl SendMessageSink for CountingSendMessageSink {
    fn is_awaiting_user_selection(&self) -> bool {
        self.delegate.is_awaiting_user_selection()
    }

    fn resolve_cloud_agent_title(&self, bc_id: &str) -> Option<String> {
        self.delegate.resolve_cloud_agent_title(bc_id)
    }

    fn request_box_help(
        &self,
        request: BoxHelpRequest,
        timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<BoxHelpOutcome, ProviderSessionError> {
        self.delegate.request_box_help(request, timestamp_ms, tool_call_id)
    }

    fn resolve_attachment_source(
        &self,
        source_url: &str,
        tool_call_id: &str,
    ) -> Result<ResolvedAttachmentSource, ProviderSessionError> {
        self.delegate.resolve_attachment_source(source_url, tool_call_id)
    }

    fn read_media_dimensions(&self, resolved_url: &str) -> Option<(u32, u32)> {
        self.delegate.read_media_dimensions(resolved_url)
    }

    fn abort_attachment_custody(&self, tool_call_id: &str) {
        self.delegate.abort_attachment_custody(tool_call_id);
    }

    fn send_message(
        &self,
        message: Value,
        timestamp_ms: u64,
        tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        let result = self.delegate.send_message(message, timestamp_ms, tool_call_id);
        if result.is_ok() {
            self.counter.record_success();
        }
        result
    }
}

pub trait SendMessageInteractionSink: Send + Sync {
    fn on_tool_call(&self, phase: &'static str, tool_call: &AgentToolCall);
}

pub struct TurnObservationSendMessageSink {
    observation: TurnObservationHandle,
}

impl TurnObservationSendMessageSink {
    pub fn new(observation: TurnObservationHandle) -> Self {
        Self { observation }
    }
}

impl SendMessageInteractionSink for TurnObservationSendMessageSink {
    fn on_tool_call(&self, phase: &'static str, tool_call: &AgentToolCall) {
        if let Ok(observation) = self.observation.lock() {
            observation.observe_communicate_tool_call(
                phase,
                SAND_SEND_MESSAGE_TOOL_NAME,
                tool_call.tool_call_id.as_deref().unwrap_or_default(),
                &tool_call.encode_to_vec(),
            );
        }
    }
}

pub struct SendMessageToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    sink: Arc<dyn SendMessageSink>,
    interaction_sink: Option<Arc<dyn SendMessageInteractionSink>>,
}

impl SendMessageToolBridge {
    pub fn new(delegate: Arc<dyn RoutedToolBridge>, sink: Arc<dyn SendMessageSink>) -> Self {
        Self { delegate, sink, interaction_sink: None }
    }

    pub fn with_interaction_sink(mut self, sink: Arc<dyn SendMessageInteractionSink>) -> Self {
        self.interaction_sink = Some(sink);
        self
    }
}


fn provider_error_message(error: &ProviderSessionError) -> String {
    match error {
        ProviderSessionError::Tool(message) => message.clone(),
        other => other.to_string(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

pub fn send_message_tool_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: SAND_SEND_MESSAGE_TOOL_NAME.to_string(),
        provider_identifier: "fabushi-runner".to_string(),
        tool_name: SAND_SEND_MESSAGE_TOOL_NAME.to_string(),
        description: Some(
            "Send a user-visible message. Use text for normal chat, attachment for standalone files/media, widget for a selectable question, cursor-agent for a cloud-agent reference, and secret-request for secure credential input.".to_string(),
        ),
        input_schema: send_message_input_schema(),
    }
}

fn build_sand_send_message(
    input: &SendMessageInput,
    sink: &dyn SendMessageSink,
    tool_call_id: &str,
) -> Result<Value, ProviderSessionError> {
    let reply = input.reply_to.as_deref().filter(|value| !value.is_empty());
    let channel = input.channel.as_deref().filter(|value| !value.is_empty());
    let message = match input.message_type {
        SendMessageType::Text => {
            let mut value = json!({
                "type": "text",
                "content": input.content.as_deref().unwrap_or_default()
            });
            if let Some(images) = input.images.as_ref().filter(|images| !images.is_empty()) {
                let resolved = images
                    .iter()
                    .map(|image| {
                        let source = sink.resolve_attachment_source(&image.url, tool_call_id)?;
                        let dimensions = sink.read_media_dimensions(&source.url);
                        let mut image_value = json!({
                            "url": source.url,
                            "alt": image.alt,
                        });
                        if let Some((width, height)) = dimensions {
                            image_value["width"] = Value::Number(width.into());
                            image_value["height"] = Value::Number(height.into());
                        }
                        Ok::<_, ProviderSessionError>(image_value)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                value["images"] = Value::Array(resolved);
            }
            value
        }
        SendMessageType::Attachment => {
            let source = sink.resolve_attachment_source(
                input.url.as_deref().unwrap_or_default(),
                tool_call_id,
            )?;
            let mut value = json!({
                "type": "attachment",
                "url": source.url,
                "alt": input.alt.as_deref()
            });
            if let Some(file_name) = source.file_name {
                value["file_name"] = Value::String(file_name);
            }
            if let Some((width, height)) = sink.read_media_dimensions(&source.url) {
                value["width"] = Value::Number(width.into());
                value["height"] = Value::Number(height.into());
            }
            value
        },
        SendMessageType::Widget => json!({
            "type": "widget",
            "widget": input.widget.clone().unwrap_or(Value::Null)
        }),
        SendMessageType::CursorAgent => {
            let bc_id = input.bc_id.as_deref().unwrap_or_default();
            let mut value = json!({
                "type": "cursor-agent",
                "bcId": bc_id
            });
            if let Some(title) = sink
                .resolve_cloud_agent_title(bc_id)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                value["title"] = Value::String(title);
            }
            value
        },
        SendMessageType::SecretRequest => {
            let secret = input.secret.as_ref().ok_or_else(|| {
                ProviderSessionError::Tool("secret is required when type is secret-request".into())
            })?;
            let mut request = json!({
                "label": clamp_secret_label(&secret.label),
                "target": {
                    "kind": "channel-credential",
                    "platform": secret.connector,
                    "field": secret.field
                }
            });
            if let Some(description) = secret.description.as_deref().filter(|value| !value.is_empty()) {
                request["description"] = Value::String(clamp_secret_description(description));
            }
            json!({
                "type": "secret-request",
                "secretRequest": request
            })
        }
    };
    let mut message = message;
    if let Some(reply) = reply {
        message["reply_to"] = Value::String(reply.to_string());
    }
    if matches!(input.message_type, SendMessageType::Text | SendMessageType::Attachment) {
        if let Some(channel) = channel {
            message["channel"] = Value::String(channel.to_string());
        }
    }
    Ok(message)
}

impl SendMessageToolBridge {
    fn emit_completed(
        &self,
        tool_call_id: &str,
        started_at_ms: u64,
        args: crate::runner::agent_v1_wire::SendMessageArgs,
        result: SendMessageResult,
    ) {
        let mut completed = create_send_message_tool_call(SendMessageToolCall {
            args: Some(args),
            result: Some(result),
        });
        completed.tool_call_id = Some(tool_call_id.to_string());
        completed.started_at_ms = Some(started_at_ms);
        completed.completed_at_ms = Some(now_ms());
        if let Some(sink) = self.interaction_sink.as_ref() {
            sink.on_tool_call("completed", &completed);
        }
    }
}

impl RoutedToolBridge for SendMessageToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| {
            tool.name != SAND_SEND_MESSAGE_TOOL_NAME
                && tool.tool_name != SAND_SEND_MESSAGE_TOOL_NAME
        });
        tools.insert(0, send_message_tool_definition());
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if tool.name != SAND_SEND_MESSAGE_TOOL_NAME
            && tool.tool_name != SAND_SEND_MESSAGE_TOOL_NAME
        {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }

        let input = parse_send_message_input(&args).map_err(ProviderSessionError::Tool)?;
        if let Err(issues) = validate_send_message(&input) {
            let message = issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.path.join("."), issue.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ProviderSessionError::Tool(message));
        }
        let message = match build_sand_send_message(&input, self.sink.as_ref(), tool_call_id) {
            Ok(message) => message,
            Err(error) => {
                self.sink.abort_attachment_custody(tool_call_id);
                return Err(error);
            }
        };
        let encoded_args = match encode_send_message_wire(&message) {
            Ok(encoded_args) => encoded_args,
            Err(error) => {
                self.sink.abort_attachment_custody(tool_call_id);
                return Err(ProviderSessionError::Tool(error));
            }
        };
        let started_at_ms = now_ms();
        let mut initial = create_send_message_tool_call(SendMessageToolCall {
            args: Some(encoded_args.clone()),
            result: None,
        });
        initial.tool_call_id = Some(tool_call_id.to_string());
        initial.started_at_ms = Some(started_at_ms);
        if let Some(sink) = self.interaction_sink.as_ref() {
            sink.on_tool_call("initial", &initial);
        }

        if self.sink.is_awaiting_user_selection() {
            let error = SAND_AWAITING_USER_SEND_MESSAGE_BLOCKED.to_string();
            self.sink.abort_attachment_custody(tool_call_id);
            self.emit_completed(
                tool_call_id,
                started_at_ms,
                encoded_args,
                SendMessageResult {
                    result: Some(send_message_result::Result::Error(SendMessageError {
                        error: error.clone(),
                    })),
                },
            );
            return Err(ProviderSessionError::Tool(error));
        }

        let timestamp_ms = now_ms();
        match self.sink.send_message(message, timestamp_ms, tool_call_id) {
            Ok(message_id) => {
                self.emit_completed(
                    tool_call_id,
                    started_at_ms,
                    encoded_args,
                    SendMessageResult {
                        result: Some(send_message_result::Result::Success(SendMessageSuccess {
                            timestamp: timestamp_ms,
                            message_id: message_id.clone().unwrap_or_default(),
                        })),
                    },
                );
                Ok(json!({
                    "sent": true,
                    "timestampMs": timestamp_ms,
                    "messageId": message_id
                }))
            }
            Err(error) => {
                self.emit_completed(
                    tool_call_id,
                    started_at_ms,
                    encoded_args,
                    SendMessageResult {
                        result: Some(send_message_result::Result::Error(SendMessageError {
                            error: provider_error_message(&error),
                        })),
                    },
                );
                Err(error)
            }
        }
    }
}
