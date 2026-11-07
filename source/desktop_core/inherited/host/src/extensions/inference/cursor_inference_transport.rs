use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use prost::Message;
use prost_types::{ListValue, Struct, Value as ProtoValue, value::Kind};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::cursor_backend::{
    create_cursor_checksum, resolve_sand_ghost_mode_header,
};
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, get_configured_backend_url, sand_box_namespace, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;

use super::cursor_session::{RequestLineage, RequestedModel, sand_default_model_selection};
use super::generated_inference_codec::InferenceReason;
use super::provider_session::{
    ProviderMessage, ProviderPartialToolCall, ProviderSessionError, ProviderTokenUsage,
    RoutedToolDefinition,
};

pub const CURSOR_INFERENCE_STREAM_PATH: &str =
    "/aiserver.v1.InferenceService/Stream";
pub const CONNECT_STREAM_CONTENT_TYPE: &str = "application/connect+proto";
pub const CONNECT_END_STREAM_FLAG: u8 = 0x02;
pub const CONNECT_COMPRESSED_FLAG: u8 = 0x01;

pub trait CursorInferenceAuth: Send + Sync {
    fn access_token(&self) -> Result<String, String>;
    fn machine_id(&self) -> Result<String, String>;
    fn backend_url(&self) -> Result<String, String> {
        get_configured_backend_url().map_err(|error| error.to_string())
    }
    fn requested_model(&self) -> RequestedModel {
        let selection = sand_default_model_selection();
        RequestedModel {
            model_id: selection.model_id,
            max_mode: Some(selection.max_mode),
            parameters: selection.parameters,
        }
    }
    fn conversation_id(&self) -> Option<String> {
        None
    }
    fn inference_reason(&self) -> Option<InferenceReason> {
        None
    }
    fn lineage(&self) -> Option<RequestLineage> {
        None
    }
    fn on_request_id(&self, _request_id: &str, _messages: &[ProviderMessage]) {}
    fn record_post_turn_labeling(&self, _messages: &[ProviderMessage]) {}
}

impl CursorInferenceAuth for HostAuthExtension {
    fn access_token(&self) -> Result<String, String> {
        self.get_access_token().map_err(|error| error.to_string())
    }

    fn machine_id(&self) -> Result<String, String> {
        self.get_machine_id().map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorCheckpointToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CursorCheckpointMessage {
    Text {
        role: String,
        text: String,
    },
    AssistantTool {
        text: String,
        tool_calls: Vec<CursorCheckpointToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        result: Value,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorCheckpoint {
    pub conversation: Vec<CursorCheckpointMessage>,
    pub text: String,
    pub completed_steps: usize,
    pub tool_calls_completed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CursorInferenceEvent {
    ModelCallIdentity(String),
    TextDelta(String),
    ThinkingDelta {
        text: String,
        signature: Option<String>,
    },
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        args: String,
        complete: bool,
        tool_index: Option<i32>,
    },
    Usage(ProviderTokenUsage),
    Finished,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceStreamRequest {
    #[prost(message, repeated, tag = "1")]
    messages: Vec<InferenceCoreMessage>,
    #[prost(message, repeated, tag = "2")]
    tools: Vec<InferenceAgentTool>,
    #[prost(message, optional, tag = "7")]
    requested_model: Option<InferenceRequestedModel>,
    #[prost(string, optional, tag = "8")]
    conversation_id: Option<String>,
    #[prost(enumeration = "InferenceReason", optional, tag = "11")]
    inference_reason: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceRequestedModel {
    #[prost(string, tag = "1")]
    model_id: String,
    #[prost(bool, tag = "2")]
    max_mode: bool,
    #[prost(message, repeated, tag = "3")]
    parameters: Vec<InferenceModelParameterValue>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceModelParameterValue {
    #[prost(string, tag = "1")]
    id: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceCoreMessage {
    #[prost(int32, tag = "1")]
    role: i32,
    #[prost(oneof = "inference_core_message::Content", tags = "2, 6")]
    content: Option<inference_core_message::Content>,
    #[prost(message, repeated, tag = "4")]
    tool_calls: Vec<InferenceToolCall>,
}

mod inference_core_message {
    use prost::Oneof;

    use super::InferenceToolResultContent;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Content {
        #[prost(string, tag = "2")]
        Text(String),
        #[prost(message, tag = "6")]
        ToolContent(InferenceToolResultContent),
    }
}

#[derive(Clone, PartialEq, Message)]
struct InferenceToolCall {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, tag = "2")]
    tool_name: String,
    #[prost(message, optional, tag = "3")]
    args: Option<Struct>,
    #[prost(string, optional, tag = "4")]
    raw_tool_call_args: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceAgentTool {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(string, tag = "2")]
    description: String,
    #[prost(message, optional, tag = "3")]
    parameters: Option<Struct>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceToolResultContent {
    #[prost(message, repeated, tag = "1")]
    parts: Vec<InferenceToolResultPart>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceToolResultPart {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, tag = "2")]
    tool_name: String,
    #[prost(message, optional, tag = "3")]
    result: Option<ProtoValue>,
    #[prost(bool, tag = "4")]
    is_error: bool,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceStreamResponse {
    #[prost(oneof = "inference_stream_response::Response", tags = "1, 2, 3, 5, 8, 9")]
    response: Option<inference_stream_response::Response>,
}

mod inference_stream_response {
    use prost::Oneof;

    use super::{
        InferenceExtendedUsageInfo, InferenceStreamError, InferenceTextStreamPart,
        InferenceThinkingStreamPart, InferenceToolCallStreamPart, InferenceUsageInfo,
    };

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Response {
        #[prost(message, tag = "1")]
        TextPart(InferenceTextStreamPart),
        #[prost(message, tag = "2")]
        ToolCallPart(InferenceToolCallStreamPart),
        #[prost(message, tag = "3")]
        Usage(InferenceUsageInfo),
        #[prost(message, tag = "5")]
        ExtendedUsage(InferenceExtendedUsageInfo),
        #[prost(message, tag = "8")]
        Error(InferenceStreamError),
        #[prost(message, tag = "9")]
        ThinkingPart(InferenceThinkingStreamPart),
    }
}

#[derive(Clone, PartialEq, Message)]
struct InferenceTextStreamPart {
    #[prost(string, tag = "1")]
    text: String,
    #[prost(bool, tag = "2")]
    is_final: bool,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceThinkingStreamPart {
    #[prost(string, tag = "1")]
    text: String,
    #[prost(string, optional, tag = "2")]
    signature: Option<String>,
    #[prost(bool, tag = "3")]
    is_final: bool,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceToolCallStreamPart {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, tag = "2")]
    tool_name: String,
    #[prost(string, tag = "3")]
    args: String,
    #[prost(bool, tag = "4")]
    is_complete: bool,
    #[prost(int32, optional, tag = "5")]
    tool_index: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceUsageInfo {
    #[prost(int32, tag = "1")]
    prompt_tokens: i32,
    #[prost(int32, tag = "2")]
    completion_tokens: i32,
    #[prost(int32, optional, tag = "3")]
    total_tokens: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceExtendedUsageInfo {
    #[prost(int32, tag = "1")]
    input_tokens: i32,
    #[prost(int32, tag = "2")]
    output_tokens: i32,
    #[prost(int32, tag = "3")]
    cache_read_tokens: i32,
    #[prost(int32, tag = "4")]
    cache_write_tokens: i32,
    #[prost(int32, tag = "5")]
    max_tokens: i32,
}

#[derive(Clone, PartialEq, Message)]
struct InferenceStreamError {
    #[prost(string, tag = "1")]
    message: String,
    #[prost(string, tag = "2")]
    code: String,
    #[prost(bool, tag = "3")]
    is_input_token_limit_error: bool,
    #[prost(bool, tag = "4")]
    is_output_token_limit_error: bool,
    #[prost(int32, tag = "5")]
    error_type: i32,
}

fn non_negative(value: i32) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn response_event(response: InferenceStreamResponse) -> Result<Option<CursorInferenceEvent>, ProviderSessionError> {
    use inference_stream_response::Response;

    match response.response {
        Some(Response::TextPart(part)) if part.is_final => Ok(Some(CursorInferenceEvent::Finished)),
        Some(Response::TextPart(part)) if !part.text.is_empty() => {
            Ok(Some(CursorInferenceEvent::TextDelta(part.text)))
        }
        Some(Response::ThinkingPart(part)) if !part.text.is_empty() || part.signature.is_some() => {
            Ok(Some(CursorInferenceEvent::ThinkingDelta {
                text: part.text,
                signature: part.signature,
            }))
        }
        Some(Response::ToolCallPart(part)) => Ok(Some(CursorInferenceEvent::ToolCall {
            tool_call_id: part.tool_call_id,
            tool_name: part.tool_name,
            args: part.args,
            complete: part.is_complete,
            tool_index: part.tool_index,
        })),
        Some(Response::ExtendedUsage(usage)) => Ok(Some(CursorInferenceEvent::Usage(
            ProviderTokenUsage {
                input_tokens: non_negative(usage.input_tokens),
                output_tokens: non_negative(usage.output_tokens),
                cache_read_tokens: non_negative(usage.cache_read_tokens),
                cache_write_tokens: non_negative(usage.cache_write_tokens),
                reasoning_tokens: None,
            },
        ))),
        Some(Response::Usage(usage)) => Ok(Some(CursorInferenceEvent::Usage(
            ProviderTokenUsage {
                input_tokens: non_negative(usage.prompt_tokens),
                output_tokens: non_negative(usage.completion_tokens),
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: None,
            },
        ))),
        Some(Response::Error(error)) => Err(ProviderSessionError::Protocol(format!(
            "Cursor inference stream failed code={} type={}: {}",
            error.code, error.error_type, error.message
        ))),
        _ => Ok(None),
    }
}

pub fn encode_connect_envelope(message: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(message.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
    frame.extend_from_slice(message);
    frame
}

pub fn decode_cursor_connect_frames(
    buffer: &mut Vec<u8>,
) -> Result<Vec<CursorInferenceEvent>, ProviderSessionError> {
    let mut events = Vec::new();
    let mut consumed = 0usize;
    while buffer.len().saturating_sub(consumed) >= 5 {
        let flags = buffer[consumed];
        let length = u32::from_be_bytes([
            buffer[consumed + 1],
            buffer[consumed + 2],
            buffer[consumed + 3],
            buffer[consumed + 4],
        ]) as usize;
        if buffer.len().saturating_sub(consumed + 5) < length {
            break;
        }
        let payload_start = consumed + 5;
        let payload_end = payload_start + length;
        let payload = &buffer[payload_start..payload_end];

        if flags & CONNECT_COMPRESSED_FLAG != 0 {
            return Err(ProviderSessionError::Protocol(
                "Cursor inference returned a compressed Connect envelope without a negotiated encoding"
                    .into(),
            ));
        }
        if flags & CONNECT_END_STREAM_FLAG != 0 {
            if !payload.is_empty() {
                let metadata: Value = serde_json::from_slice(payload).map_err(|error| {
                    ProviderSessionError::Protocol(format!(
                        "Cursor inference returned invalid Connect end-stream metadata: {error}"
                    ))
                })?;
                if let Some(error) = metadata.get("error") {
                    return Err(ProviderSessionError::Protocol(format!(
                        "Cursor inference Connect stream ended with error: {error}"
                    )));
                }
            }
            consumed = payload_end;
            continue;
        }
        if flags != 0 {
            return Err(ProviderSessionError::Protocol(format!(
                "Cursor inference returned unsupported Connect envelope flags: 0x{flags:02x}"
            )));
        }

        let response = InferenceStreamResponse::decode(payload).map_err(|error| {
            ProviderSessionError::Protocol(format!(
                "Cursor inference returned invalid protobuf response: {error}"
            ))
        })?;
        if let Some(event) = response_event(response)? {
            events.push(event);
        }
        consumed = payload_end;
    }
    if consumed > 0 {
        buffer.drain(..consumed);
    }
    Ok(events)
}

fn json_to_proto(value: &Value) -> ProtoValue {
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(value) => Kind::BoolValue(*value),
        Value::Number(value) => Kind::NumberValue(value.as_f64().unwrap_or_default()),
        Value::String(value) => Kind::StringValue(value.clone()),
        Value::Array(values) => Kind::ListValue(ListValue {
            values: values.iter().map(json_to_proto).collect(),
        }),
        Value::Object(values) => Kind::StructValue(Struct {
            fields: values
                .iter()
                .map(|(key, value)| (key.clone(), json_to_proto(value)))
                .collect::<BTreeMap<_, _>>(),
        }),
    };
    ProtoValue { kind: Some(kind) }
}

fn json_object_to_proto(value: &Value) -> Struct {
    match json_to_proto(value).kind {
        Some(Kind::StructValue(value)) => value,
        _ => Struct::default(),
    }
}

fn role_number(role: &str) -> i32 {
    match role {
        "user" => 1,
        "assistant" => 2,
        "tool" => 3,
        "system" => 4,
        _ => 0,
    }
}

fn provider_message_to_checkpoint(message: &ProviderMessage) -> CursorCheckpointMessage {
    CursorCheckpointMessage::Text {
        role: message.role.clone(),
        text: message.content.clone(),
    }
}

fn checkpoint_message_to_proto(message: &CursorCheckpointMessage) -> InferenceCoreMessage {
    match message {
        CursorCheckpointMessage::Text { role, text } => InferenceCoreMessage {
            role: role_number(role),
            content: Some(inference_core_message::Content::Text(text.clone())),
            tool_calls: Vec::new(),
        },
        CursorCheckpointMessage::AssistantTool { text, tool_calls } => InferenceCoreMessage {
            role: 2,
            content: (!text.is_empty()).then(|| {
                inference_core_message::Content::Text(text.clone())
            }),
            tool_calls: tool_calls
                .iter()
                .map(|call| InferenceToolCall {
                    tool_call_id: call.tool_call_id.clone(),
                    tool_name: call.tool_name.clone(),
                    args: serde_json::from_str::<Value>(&call.args)
                        .ok()
                        .map(|value| json_object_to_proto(&value)),
                    raw_tool_call_args: Some(call.args.clone()),
                })
                .collect(),
        },
        CursorCheckpointMessage::ToolResult {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => InferenceCoreMessage {
            role: 3,
            content: Some(inference_core_message::Content::ToolContent(
                InferenceToolResultContent {
                    parts: vec![InferenceToolResultPart {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        result: Some(json_to_proto(result)),
                        is_error: *is_error,
                    }],
                },
            )),
            tool_calls: Vec::new(),
        },
    }
}

fn encode_cursor_conversation_request(
    conversation: &[CursorCheckpointMessage],
    tools: &[RoutedToolDefinition],
    model: &RequestedModel,
    conversation_id: Option<String>,
    inference_reason: Option<InferenceReason>,
) -> Result<Vec<u8>, ProviderSessionError> {
    let request = InferenceStreamRequest {
        messages: conversation.iter().map(checkpoint_message_to_proto).collect(),
        tools: tools
            .iter()
            .map(|tool| InferenceAgentTool {
                name: tool.name.clone(),
                description: tool.description.clone().unwrap_or_default(),
                parameters: Some(json_object_to_proto(&tool.input_schema)),
            })
            .collect(),
        requested_model: Some(InferenceRequestedModel {
            model_id: model.model_id.clone(),
            max_mode: model.max_mode.unwrap_or(false),
            parameters: model
                .parameters
                .iter()
                .cloned()
                .map(|parameter| InferenceModelParameterValue {
                    id: parameter.id,
                    value: parameter.value,
                })
                .collect(),
        }),
        conversation_id,
        inference_reason: inference_reason.map(|reason| reason as i32),
    };
    let mut payload = Vec::with_capacity(request.encoded_len());
    request.encode(&mut payload).map_err(|error| {
        ProviderSessionError::Protocol(format!(
            "could not encode Cursor inference protobuf request: {error}"
        ))
    })?;
    Ok(encode_connect_envelope(&payload))
}

pub fn encode_cursor_inference_request(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
) -> Result<Vec<u8>, ProviderSessionError> {
    let conversation = messages
        .iter()
        .map(provider_message_to_checkpoint)
        .collect::<Vec<_>>();
    let selection = sand_default_model_selection();
    let model = RequestedModel {
        model_id: selection.model_id,
        max_mode: Some(selection.max_mode),
        parameters: selection.parameters,
    };
    encode_cursor_conversation_request(&conversation, tools, &model, None, None)
}

pub fn encode_cursor_inference_request_with_model(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    model: &RequestedModel,
) -> Result<Vec<u8>, ProviderSessionError> {
    let conversation = messages
        .iter()
        .map(provider_message_to_checkpoint)
        .collect::<Vec<_>>();
    encode_cursor_conversation_request(&conversation, tools, model, None, None)
}

pub trait CursorInferenceStreamTransport {
    fn stream_conversation(
        &self,
        conversation: &[CursorCheckpointMessage],
        tools: &[RoutedToolDefinition],
        should_cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(CursorInferenceEvent) -> Result<(), ProviderSessionError>,
    ) -> Result<(), ProviderSessionError>;
}

pub struct CursorInferenceTransport {
    auth: Arc<dyn CursorInferenceAuth>,
}

impl CursorInferenceTransport {
    pub fn new(auth: Arc<dyn CursorInferenceAuth>) -> Self {
        Self { auth }
    }

    pub fn stream(
        &self,
        messages: &[ProviderMessage],
        tools: &[RoutedToolDefinition],
        should_cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(CursorInferenceEvent) -> Result<(), ProviderSessionError>,
    ) -> Result<(), ProviderSessionError> {
        let conversation = messages
            .iter()
            .map(provider_message_to_checkpoint)
            .collect::<Vec<_>>();
        self.stream_conversation(&conversation, tools, should_cancel, on_event)
    }

    pub fn stream_conversation(
        &self,
        conversation: &[CursorCheckpointMessage],
        tools: &[RoutedToolDefinition],
        should_cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(CursorInferenceEvent) -> Result<(), ProviderSessionError>,
    ) -> Result<(), ProviderSessionError> {
        if should_cancel() {
            return Err(ProviderSessionError::Cancelled(
                "Runner cancelled Cursor inference before dispatch".into(),
            ));
        }

        let backend_url = self.auth.backend_url().map_err(ProviderSessionError::Authentication)?;
        let access_token = self.auth.access_token().map_err(ProviderSessionError::Authentication)?;
        let machine_id = self.auth.machine_id().map_err(ProviderSessionError::Authentication)?;
        let ghost_mode = resolve_sand_ghost_mode_header(
            &backend_url,
            &access_token,
            &machine_id,
        );
        let request_id = Uuid::new_v4().to_string();
        let requested_model = self.auth.requested_model();
        let conversation_id = self.auth.conversation_id();
        let inference_reason = self.auth.inference_reason();
        let body = encode_cursor_conversation_request(
            conversation,
            tools,
            &requested_model,
            conversation_id,
            inference_reason,
        )?;
        let label_messages = conversation
            .iter()
            .map(|message| match message {
                CursorCheckpointMessage::Text { role, text } => ProviderMessage {
                    role: role.clone(),
                    content: text.clone(),
                },
                CursorCheckpointMessage::AssistantTool { text, .. } => ProviderMessage {
                    role: "assistant".into(),
                    content: text.clone(),
                },
                CursorCheckpointMessage::ToolResult { result, .. } => ProviderMessage {
                    role: "tool".into(),
                    content: result.to_string(),
                },
            })
            .collect::<Vec<_>>();
        self.auth.on_request_id(&request_id, &label_messages);

        let base = Url::parse(&backend_url)
            .map_err(|error| ProviderSessionError::Configuration(error.to_string()))?;
        let url = base
            .join(CURSOR_INFERENCE_STREAM_PATH)
            .map_err(|error| ProviderSessionError::Configuration(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
        let mut request = client
            .post(url)
            .header(CONTENT_TYPE, CONNECT_STREAM_CONTENT_TYPE)
            .header(ACCEPT, CONNECT_STREAM_CONTENT_TYPE)
            .header("connect-protocol-version", "1")
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(
                "x-cursor-checksum",
                create_cursor_checksum(&machine_id, crate::extensions::auth::credential_renewer::system_now_ms()),
            )
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            .header("x-ghost-mode", ghost_mode)
            .header("x-request-id", &request_id);
        if let Some(lineage) = self.auth.lineage() {
            for (name, value) in lineage.sanitized_headers() {
                request = request.header(name, value);
            }
        }
        let request = request.body(body);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
        runtime.block_on(async {
            let response = request
                .send()
                .await
                .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(ProviderSessionError::Transport(format!(
                    "Cursor inference returned HTTP {}: {}",
                    status.as_u16(),
                    body.chars().take(512).collect::<String>(),
                )));
            }

            on_event(CursorInferenceEvent::ModelCallIdentity(request_id.clone()))?;
            let mut stream = response.bytes_stream();
            let mut buffer = Vec::new();
            loop {
                if should_cancel() {
                    return Err(ProviderSessionError::Cancelled(
                        "Runner cancelled Cursor inference stream".into(),
                    ));
                }
                let next = tokio::select! {
                    next = stream.next() => next,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => continue,
                };
                let Some(next) = next else { break };
                let chunk = next
                    .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
                buffer.extend_from_slice(&chunk);
                for event in decode_cursor_connect_frames(&mut buffer)? {
                    on_event(event)?;
                }
            }
            if !buffer.is_empty() {
                return Err(ProviderSessionError::Protocol(
                    "Cursor inference stream ended with a truncated Connect envelope".into(),
                ));
            }
            Ok(())
        })
    }
}

impl CursorInferenceStreamTransport for CursorInferenceTransport {
    fn stream_conversation(
        &self,
        conversation: &[CursorCheckpointMessage],
        tools: &[RoutedToolDefinition],
        should_cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(CursorInferenceEvent) -> Result<(), ProviderSessionError>,
    ) -> Result<(), ProviderSessionError> {
        CursorInferenceTransport::stream_conversation(
            self,
            conversation,
            tools,
            should_cancel,
            on_event,
        )
    }
}

#[derive(Debug, Default)]
struct PartialCursorToolCall {
    tool_call_id: String,
    tool_name: String,
    args: String,
    complete: bool,
}

fn cursor_tool_call_key(
    call_id: &str,
    tool_index: Option<i32>,
    ordinal: usize,
) -> String {
    if !call_id.is_empty() {
        return format!("id:{call_id}");
    }
    if let Some(index) = tool_index {
        return format!("index:{index}");
    }
    format!("ordinal:{ordinal}")
}

pub fn run_cursor_with_transport_reporting_usage(
    transport: &dyn CursorInferenceStreamTransport,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CursorCheckpoint>,
    on_checkpoint: &mut dyn FnMut(&CursorCheckpoint) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
) -> Result<String, ProviderSessionError> {
    let mut ignore_partial = |_partial: ProviderPartialToolCall| Ok(());
    run_cursor_with_transport_reporting_usage_and_partials(
        transport, messages, tools, execute_tool, on_text_delta, should_cancel,
        resume_from, on_checkpoint, on_usage, &mut ignore_partial,
    )
}

pub fn run_cursor_with_transport_reporting_usage_and_partials(
    transport: &dyn CursorInferenceStreamTransport,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CursorCheckpoint>,
    on_checkpoint: &mut dyn FnMut(&CursorCheckpoint) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    run_cursor_with_transport_reporting_usage_and_partials_with_max_steps(
        transport,
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        on_partial_tool_call,
        8,
        None,
    )
}

pub fn run_cursor_with_transport_reporting_usage_and_partials_with_max_steps(
    transport: &dyn CursorInferenceStreamTransport,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CursorCheckpoint>,
    on_checkpoint: &mut dyn FnMut(&CursorCheckpoint) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
    max_steps: usize,
    tool_step_reminder: Option<&(dyn Fn(&[(String, Value)]) -> Option<String> + Send + Sync)>,
) -> Result<String, ProviderSessionError> {
    run_cursor_with_transport_reporting_usage_and_partials_with_initial_conversation(
        transport,
        messages,
        None,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        on_partial_tool_call,
        max_steps,
        tool_step_reminder,
    )
}

pub fn run_cursor_with_transport_reporting_usage_and_partials_with_initial_conversation(
    transport: &dyn CursorInferenceStreamTransport,
    messages: &[ProviderMessage],
    initial_conversation: Option<&[CursorCheckpointMessage]>,
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CursorCheckpoint>,
    on_checkpoint: &mut dyn FnMut(&CursorCheckpoint) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
    max_steps: usize,
    tool_step_reminder: Option<&(dyn Fn(&[(String, Value)]) -> Option<String> + Send + Sync)>,
) -> Result<String, ProviderSessionError> {
    let max_steps = max_steps.max(1);
    let tool_index = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<BTreeMap<_, _>>();
    let (mut conversation, mut text, mut completed_steps, mut tool_calls_completed) =
        match resume_from {
            Some(checkpoint) => (
                checkpoint.conversation.clone(),
                checkpoint.text.clone(),
                checkpoint.completed_steps,
                checkpoint.tool_calls_completed,
            ),
            None => (
                initial_conversation
                    .map(|conversation| conversation.to_vec())
                    .unwrap_or_else(|| {
                        messages
                            .iter()
                            .map(provider_message_to_checkpoint)
                            .collect::<Vec<_>>()
                    }),
                String::new(),
                0,
                0,
            ),
        };

    if completed_steps >= max_steps {
        return Err(ProviderSessionError::Protocol(format!(
            "Cursor resume checkpoint already exhausted Fabushi's {max_steps}-step tool limit."
        )));
    }

    for _step in completed_steps..max_steps {
        if should_cancel() {
            return Err(ProviderSessionError::Cancelled(
                "Runner cancelled Cursor inference before provider step".into(),
            ));
        }

        let mut partial_calls = BTreeMap::<String, PartialCursorToolCall>::new();
        let mut step_text = String::new();
        let mut model_call_id = String::new();
        transport.stream_conversation(
            &conversation,
            tools,
            should_cancel,
            &mut |event| {
                match event {
                    CursorInferenceEvent::ModelCallIdentity(identity) => {
                        model_call_id = identity;
                    }
                    CursorInferenceEvent::TextDelta(delta) => {
                        step_text.push_str(&delta);
                        text.push_str(&delta);
                        on_text_delta(&delta, &text);
                    }
                    CursorInferenceEvent::ThinkingDelta { .. } => {}
                    CursorInferenceEvent::Usage(usage) => on_usage(usage),
                    CursorInferenceEvent::Finished => {}
                    CursorInferenceEvent::ToolCall {
                        tool_call_id,
                        tool_name,
                        args,
                        complete,
                        tool_index: stream_tool_index,
                    } => {
                        let key = cursor_tool_call_key(
                            &tool_call_id,
                            stream_tool_index,
                            partial_calls.len(),
                        );
                        let partial = partial_calls.entry(key).or_default();
                        if !tool_call_id.is_empty() {
                            partial.tool_call_id = tool_call_id;
                        }
                        if !tool_name.is_empty() {
                            partial.tool_name = tool_name;
                        }
                        if !args.is_empty() {
                            if complete {
                                partial.args = args;
                            } else {
                                partial.args.push_str(&args);
                            }
                        }
                        partial.complete |= complete;
                        if !partial.tool_call_id.is_empty() && !partial.tool_name.is_empty() {
                            if let Some(tool) = tool_index.get(&partial.tool_name).copied() {
                                on_partial_tool_call(ProviderPartialToolCall {
                                    tool: tool.clone(),
                                    tool_call_id: partial.tool_call_id.clone(),
                                    raw_arguments: partial.args.clone(),
                                    model_call_id: (!model_call_id.is_empty())
                                        .then(|| model_call_id.clone()),
                                })?;
                            }
                        }
                    }
                }
                Ok(())
            },
        )?;

        let calls = partial_calls
            .into_values()
            .filter(|call| call.complete)
            .map(|mut call| {
                if call.tool_call_id.is_empty() {
                    call.tool_call_id = Uuid::new_v4().to_string();
                }
                call
            })
            .collect::<Vec<_>>();
        if calls.is_empty() {
            return Ok(text);
        }

        let checkpoint_calls = calls
            .iter()
            .map(|call| CursorCheckpointToolCall {
                tool_call_id: call.tool_call_id.clone(),
                tool_name: call.tool_name.clone(),
                args: call.args.clone(),
            })
            .collect::<Vec<_>>();
        conversation.push(CursorCheckpointMessage::AssistantTool {
            text: step_text,
            tool_calls: checkpoint_calls,
        });

        let calls_in_step = calls.len();
        let mut observed_tool_calls = Vec::with_capacity(calls_in_step);
        for call in calls {
            if should_cancel() {
                return Err(ProviderSessionError::Cancelled(
                    "Runner cancelled before Cursor tool execution".into(),
                ));
            }
            let args = serde_json::from_str::<Value>(&call.args)
                .unwrap_or_else(|_| json!({}));
            observed_tool_calls.push((call.tool_name.clone(), args.clone()));
            let (result, is_error) = match tool_index.get(&call.tool_name).copied() {
                None => (
                    json!({
                        "error": format!("Unknown Fabushi tool: {}", call.tool_name)
                    }),
                    true,
                ),
                Some(tool) => match execute_tool(tool, args, &call.tool_call_id) {
                    Ok(value) => (value, false),
                    Err(error) => (
                        json!({ "error": error.to_string() }),
                        true,
                    ),
                },
            };
            conversation.push(CursorCheckpointMessage::ToolResult {
                tool_call_id: call.tool_call_id,
                tool_name: call.tool_name,
                result,
                is_error,
            });
        }
        if let Some(reminder) =
            tool_step_reminder.and_then(|hook| hook(&observed_tool_calls))
        {
            conversation.push(CursorCheckpointMessage::Text {
                role: "user".into(),
                text: reminder,
            });
        }

        completed_steps = completed_steps.saturating_add(1);
        tool_calls_completed = tool_calls_completed.saturating_add(calls_in_step);
        on_checkpoint(&CursorCheckpoint {
            conversation: conversation.clone(),
            text: text.clone(),
            completed_steps,
            tool_calls_completed,
        })?;
    }

    Err(ProviderSessionError::Protocol(format!(
        "Cursor inference exceeded Fabushi's {max_steps}-step tool limit."
    )))
}

pub fn cursor_tool_result_message(
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    result: &Value,
    is_error: bool,
) -> Vec<u8> {
    let message = InferenceCoreMessage {
        role: 3,
        content: Some(inference_core_message::Content::ToolContent(
            InferenceToolResultContent {
                parts: vec![InferenceToolResultPart {
                    tool_call_id: tool_call_id.into(),
                    tool_name: tool_name.into(),
                    result: Some(json_to_proto(result)),
                    is_error,
                }],
            },
        )),
        tool_calls: Vec::new(),
    };
    message.encode_to_vec()
}

#[doc(hidden)]
pub fn encode_test_response(event: CursorInferenceEvent) -> Vec<u8> {
    use inference_stream_response::Response;

    let response = match event {
        CursorInferenceEvent::ModelCallIdentity(_) => {
            panic!("model-call identity is transport metadata, not a provider frame")
        }
        CursorInferenceEvent::TextDelta(text) => InferenceStreamResponse {
            response: Some(Response::TextPart(InferenceTextStreamPart {
                text,
                is_final: false,
            })),
        },
        CursorInferenceEvent::ThinkingDelta { text, signature } => InferenceStreamResponse {
            response: Some(Response::ThinkingPart(InferenceThinkingStreamPart {
                text,
                signature,
                is_final: false,
            })),
        },
        CursorInferenceEvent::ToolCall {
            tool_call_id,
            tool_name,
            args,
            complete,
            tool_index,
        } => InferenceStreamResponse {
            response: Some(Response::ToolCallPart(InferenceToolCallStreamPart {
                tool_call_id,
                tool_name,
                args,
                is_complete: complete,
                tool_index,
            })),
        },
        CursorInferenceEvent::Usage(usage) => InferenceStreamResponse {
            response: Some(Response::ExtendedUsage(InferenceExtendedUsageInfo {
                input_tokens: i32::try_from(usage.input_tokens).unwrap_or(i32::MAX),
                output_tokens: i32::try_from(usage.output_tokens).unwrap_or(i32::MAX),
                cache_read_tokens: i32::try_from(usage.cache_read_tokens).unwrap_or(i32::MAX),
                cache_write_tokens: i32::try_from(usage.cache_write_tokens).unwrap_or(i32::MAX),
                max_tokens: 0,
            })),
        },
        CursorInferenceEvent::Finished => InferenceStreamResponse {
            response: Some(Response::TextPart(InferenceTextStreamPart {
                text: String::new(),
                is_final: true,
            })),
        },
    };
    encode_connect_envelope(&response.encode_to_vec())
}
