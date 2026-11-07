//! Core domain types for OpenClaw multi-channel runtime and adapters (OC-S3).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

/// Supported external messaging channel platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Discord,
    Slack,
    Telegram,
    Feishu,
    Line,
    WhatsApp,
    Signal,
    IMessage,
    Matrix,
    Mattermost,
    Nostr,
    Twitch,
    Zalo,
    Nextcloud,
    Tlon,
    BlueBubbles,
    GoogleChat,
    MSTeams,
    Custom,
}

impl ChannelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Telegram => "telegram",
            Self::Feishu => "feishu",
            Self::Line => "line",
            Self::WhatsApp => "whatsapp",
            Self::Signal => "signal",
            Self::IMessage => "imessage",
            Self::Matrix => "matrix",
            Self::Mattermost => "mattermost",
            Self::Nostr => "nostr",
            Self::Twitch => "twitch",
            Self::Zalo => "zalo",
            Self::Nextcloud => "nextcloud",
            Self::Tlon => "tlon",
            Self::BlueBubbles => "bluebubbles",
            Self::GoogleChat => "googlechat",
            Self::MSTeams => "msteams",
            Self::Custom => "custom",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Self> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "discord" => Some(Self::Discord),
            "slack" => Some(Self::Slack),
            "telegram" | "tg" => Some(Self::Telegram),
            "feishu" | "lark" => Some(Self::Feishu),
            "line" => Some(Self::Line),
            "whatsapp" | "wa" => Some(Self::WhatsApp),
            "signal" => Some(Self::Signal),
            "imessage" => Some(Self::IMessage),
            "matrix" => Some(Self::Matrix),
            "mattermost" => Some(Self::Mattermost),
            "nostr" => Some(Self::Nostr),
            "twitch" => Some(Self::Twitch),
            "zalo" => Some(Self::Zalo),
            "nextcloud" => Some(Self::Nextcloud),
            "tlon" | "urbit" => Some(Self::Tlon),
            "bluebubbles" => Some(Self::BlueBubbles),
            "googlechat" | "gchat" => Some(Self::GoogleChat),
            "msteams" | "teams" => Some(Self::MSTeams),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Outbound message formatting preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageFormat {
    #[default]
    Markdown,
    PlainText,
    Html,
    SlackMrkdwn,
}

/// Normalized inbound message received from an external channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundChannelMessage {
    pub channel: ChannelKind,
    pub account_id: Option<String>,
    pub message_id: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub conversation_id: String,
    pub thread_id: Option<String>,
    pub text: String,
    pub attachments: Vec<InboundAttachmentMeta>,
    pub is_dm: bool,
    pub raw_payload: Option<serde_json::Value>,
    pub received_at: i64,
}

/// Metadata for an inbound attachment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundAttachmentMeta {
    pub attachment_id: String,
    pub mime_type: String,
    pub file_name: Option<String>,
    pub size_bytes: Option<usize>,
    pub download_url: Option<String>,
}

/// Unified outbound delivery payload across all channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundDeliveryPayload {
    pub channel: ChannelKind,
    pub account_id: Option<String>,
    pub target_id: String,
    pub text: String,
    pub reply_to_message_id: Option<String>,
    pub thread_id: Option<String>,
    pub attachments: Vec<String>,
    pub format: MessageFormat,
    pub metadata: HashMap<String, String>,
}

impl OutboundDeliveryPayload {
    pub fn new(channel: ChannelKind, target_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            channel,
            account_id: None,
            target_id: target_id.into(),
            text: text.into(),
            reply_to_message_id: None,
            thread_id: None,
            attachments: Vec::new(),
            format: MessageFormat::default(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to_message_id = Some(reply_to.into());
        self
    }

    pub fn with_thread(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }

    pub fn with_format(mut self, format: MessageFormat) -> Self {
        self.format = format;
        self
    }
}

/// Delivery receipt returned when a message has been accepted or dispatched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub channel: ChannelKind,
    pub message_id: String,
    pub conversation_id: String,
    pub delivered_at: i64,
    pub status: DeliveryStatus,
    pub error_detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    Queued,
    RateLimited,
    Failed,
}

/// Result of evaluating inbound webhook or message dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDispatchResult {
    pub accepted: bool,
    pub action_taken: DispatchAction,
    pub reason: Option<String>,
    pub response_payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchAction {
    ForwardedToAgent,
    HandledByBuiltinCommand,
    SuppressedDuplicate,
    RejectedPolicy,
    RejectedAuth,
    IgnoredEcho,
}

/// Trait defining the contract for all external channel adapters.
pub trait ChannelAdapter: Send + Sync {
    /// Return the kind of channel this adapter handles.
    fn channel_kind(&self) -> ChannelKind;

    /// Verifies inbound webhook request authentication/signature.
    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String>;

    /// Parse an inbound webhook payload into a normalized inbound message.
    fn parse_webhook_event(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String>;

    /// Send an outbound delivery payload to the destination platform.
    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String>;

    /// Send a typing indicator heartbeat to the target conversation.
    fn send_typing_indicator(
        &self,
        _target_id: &str,
        _thread_id: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
}

pub type SharedChannelAdapter = Arc<dyn ChannelAdapter>;
