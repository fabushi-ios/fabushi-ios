//! Minimal Rust/prost bindings for frozen Grok Bot 0.18 agent.v1 tool-call wire types.
//!
//! The complete recovered TypeScript descriptors remain canonical under
//! `source/packages/proto/generated/agent/v1/**`. This module intentionally
//! binds only the message subset that the Rust Runner must emit/consume today.
//! Field numbers and oneof tags are copied from those frozen descriptors so
//! Rust serialization is wire-compatible without moving ownership out of Runner.

use prost::Message;

pub const FROZEN_AGENT_PB_BLOB_SHA: &str =
    "3cd3d020e69b54d1a6df27c0f967afe7e9fc942c";
pub const FROZEN_SEND_MESSAGE_PB_BLOB_SHA: &str =
    "fff9a91c8bf835d33b9d38b855c8785a9ebab03e";
pub const FROZEN_COMMUNICATE_UPDATE_PB_BLOB_SHA: &str =
    "20af9fc29e574015c5a40ab314e15002dd232cc5";

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageText {
    #[prost(string, tag = "1")]
    pub content: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageAttachment {
    #[prost(string, tag = "1")]
    pub url: String,
    #[prost(string, optional, tag = "2")]
    pub alt: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageArgs {
    #[prost(oneof = "send_message_args::Message", tags = "1, 2")]
    pub message: Option<send_message_args::Message>,
}

pub mod send_message_args {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Message {
        #[prost(message, tag = "1")]
        Text(super::SendMessageText),
        #[prost(message, tag = "2")]
        Attachment(super::SendMessageAttachment),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageSuccess {
    #[prost(uint64, tag = "1")]
    pub timestamp: u64,
    #[prost(string, tag = "2")]
    pub message_id: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageError {
    #[prost(string, tag = "1")]
    pub error: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageResult {
    #[prost(oneof = "send_message_result::Result", tags = "1, 2")]
    pub result: Option<send_message_result::Result>,
}

pub mod send_message_result {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(super::SendMessageSuccess),
        #[prost(message, tag = "2")]
        Error(super::SendMessageError),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct SendMessageToolCall {
    #[prost(message, optional, tag = "1")]
    pub args: Option<SendMessageArgs>,
    #[prost(message, optional, tag = "2")]
    pub result: Option<SendMessageResult>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommunicateUpdateArgs {
    #[prost(string, optional, tag = "1")]
    pub current_step: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub final_summary: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub completed_subtitle: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommunicateUpdateSuccess {
    #[prost(string, tag = "1")]
    pub current_step: String,
    #[prost(uint32, tag = "3")]
    pub message_index: u32,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommunicateUpdateError {
    #[prost(string, tag = "1")]
    pub error: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct CommunicateUpdateResult {
    #[prost(oneof = "communicate_update_result::Result", tags = "1, 2")]
    pub result: Option<communicate_update_result::Result>,
}

pub mod communicate_update_result {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(super::CommunicateUpdateSuccess),
        #[prost(message, tag = "2")]
        Error(super::CommunicateUpdateError),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct CommunicateUpdateToolCall {
    #[prost(message, optional, tag = "1")]
    pub args: Option<CommunicateUpdateArgs>,
    #[prost(message, optional, tag = "2")]
    pub result: Option<CommunicateUpdateResult>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AgentToolCall {
    #[prost(oneof = "agent_tool_call::Tool", tags = "48, 55")]
    pub tool: Option<agent_tool_call::Tool>,
    #[prost(string, optional, tag = "57")]
    pub tool_call_id: Option<String>,
    #[prost(uint64, optional, tag = "59")]
    pub started_at_ms: Option<u64>,
    #[prost(uint64, optional, tag = "60")]
    pub completed_at_ms: Option<u64>,
}

pub mod agent_tool_call {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Tool {
        #[prost(message, tag = "48")]
        CommunicateUpdateToolCall(super::CommunicateUpdateToolCall),
        #[prost(message, tag = "55")]
        SendMessageToolCall(super::SendMessageToolCall),
    }
}
