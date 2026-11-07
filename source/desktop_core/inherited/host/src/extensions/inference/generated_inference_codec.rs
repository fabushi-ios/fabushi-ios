use prost::{Enumeration, Message, Oneof};

use super::provider_session::ProviderMessage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum InferenceReason {
    Unspecified = 0,
    GeminiVideoSubagent = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enumeration)]
#[repr(i32)]
pub enum InferenceMessageRole {
    Unspecified = 0,
    User = 1,
    Assistant = 2,
    Tool = 3,
    System = 4,
}

#[derive(Clone, PartialEq, Message)]
pub struct InferenceCoreMessage {
    #[prost(enumeration = "InferenceMessageRole", tag = "1")]
    pub role: i32,
    #[prost(oneof = "inference_core_message::Content", tags = "2")]
    pub content: Option<inference_core_message::Content>,
}

pub mod inference_core_message {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Content {
        #[prost(string, tag = "2")]
        Text(String),
    }
}

pub fn core_message_to_proto(message: &ProviderMessage) -> InferenceCoreMessage {
    let role = match message.role.as_str() {
        "user" => InferenceMessageRole::User,
        "assistant" => InferenceMessageRole::Assistant,
        "tool" => InferenceMessageRole::Tool,
        "system" => InferenceMessageRole::System,
        _ => InferenceMessageRole::Unspecified,
    };
    InferenceCoreMessage {
        role: role as i32,
        content: Some(inference_core_message::Content::Text(message.content.clone())),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentFollowupCategorizationRequest {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(string, tag = "2")]
    pub replying_to_request_id: String,
    #[prost(message, repeated, tag = "3")]
    pub messages: Vec<InferenceCoreMessage>,
    #[prost(string, optional, tag = "4")]
    pub conversation_id: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub agent_mode: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub model_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentPostTurnLabelingRequest {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(message, repeated, tag = "2")]
    pub messages: Vec<InferenceCoreMessage>,
    #[prost(string, optional, tag = "3")]
    pub conversation_id: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub agent_mode: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub model_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GetSignedUrlForAttachedMediaRequest {
    #[prost(string, optional, tag = "1")]
    pub key: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub mime_type: Option<String>,
    #[prost(string, tag = "3")]
    pub conversation_id: String,
    #[prost(int64, optional, tag = "4")]
    pub content_length_bytes: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GetSignedUrlForAttachedMediaResponse {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub post_url: String,
    #[prost(string, tag = "3")]
    pub get_url: String,
    #[prost(int64, tag = "4")]
    pub expires_at_unix_ms: i64,
    #[prost(int64, tag = "5")]
    pub refresh_after_unix_ms: i64,
    #[prost(map = "string, string", tag = "6")]
    pub post_fields: std::collections::HashMap<String, String>,
    #[prost(string, tag = "7")]
    pub put_url: String,
}
