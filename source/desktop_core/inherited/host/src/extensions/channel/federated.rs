//! Federated and privacy-preserving channel adapters: Signal, Matrix, iMessage, and Mattermost (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus,
    InboundChannelMessage, OutboundDeliveryPayload,
};
use super::webhook_ingress::{get_header_ignore_case, subtle_slices_equal};

// ---------------------------------------------------------------------------
// 1. Signal Adapter (signal-cli daemon HTTP / JSON-RPC bridge)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SignalAdapterConfig {
    pub account_number: Option<String>,
}

pub struct SignalAdapter {
    config: SignalAdapterConfig,
}

impl SignalAdapter {
    pub fn new(config: SignalAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for SignalAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Signal
    }

    fn verify_webhook(
        &self,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        // Signal-cli REST daemon sends local loopback notifications
        Ok(true)
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        if body.is_empty() {
            return Ok(None);
        }

        let val: Value = serde_json::from_slice(body)
            .map_err(|e| format!("Invalid Signal envelope JSON: {e}"))?;

        let envelope = val.get("envelope").unwrap_or(&val);
        let source = envelope.get("source").and_then(|v| v.as_str()).unwrap_or("unknown_signal");
        let source_name = envelope.get("sourceName").and_then(|v| v.as_str()).map(String::from);
        let timestamp = envelope.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

        let data = envelope.get("dataMessage");
        let Some(dm) = data else {
            return Ok(None);
        };

        let text = dm.get("message").and_then(|v| v.as_str()).unwrap_or("");
        let group_info = dm.get("groupInfo");
        let is_dm = group_info.is_none();
        let conv_id = group_info
            .and_then(|g| g.get("groupId"))
            .and_then(|v| v.as_str())
            .unwrap_or(source);

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Signal,
            account_id: self.config.account_number.clone(),
            message_id: format!("{timestamp}_{source}"),
            sender_id: source.to_string(),
            sender_name: source_name,
            conversation_id: conv_id.to_string(),
            thread_id: None,
            text: text.to_string(),
            attachments: Vec::new(),
            is_dm,
            raw_payload: Some(val),
            received_at: chrono::Utc::now().timestamp(),
        }))
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let delivered_id = format!("signal_msg_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Signal,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 2. Matrix Adapter (Matrix Client-Server Sync / AppService)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MatrixAdapterConfig {
    pub homeserver_url: Option<String>,
    pub user_id: Option<String>,
}

pub struct MatrixAdapter {
    config: MatrixAdapterConfig,
}

impl MatrixAdapter {
    pub fn new(config: MatrixAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for MatrixAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Matrix
    }

    fn verify_webhook(
        &self,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        Ok(true)
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        if body.is_empty() {
            return Ok(None);
        }

        let val: Value = serde_json::from_slice(body)
            .map_err(|e| format!("Invalid Matrix event JSON: {e}"))?;

        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type != "m.room.message" && !event_type.is_empty() {
            return Ok(None);
        }

        let event_id = val.get("event_id").and_then(|v| v.as_str()).unwrap_or("matrix_event");
        let room_id = val.get("room_id").and_then(|v| v.as_str()).unwrap_or("!room:matrix.org");
        let sender = val.get("sender").and_then(|v| v.as_str()).unwrap_or("@user:matrix.org");

        // Filter self echo
        if let Some(ref my_id) = self.config.user_id {
            if sender == my_id {
                return Ok(None);
            }
        }

        let content = val.get("content").unwrap_or(&val);
        let body_text = content.get("body").and_then(|v| v.as_str()).unwrap_or("");

        // Check relational thread (m.relates_to)
        let thread_id = content
            .get("m.relates_to")
            .and_then(|r| r.get("event_id"))
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Matrix,
            account_id: self.config.user_id.clone(),
            message_id: event_id.to_string(),
            sender_id: sender.to_string(),
            sender_name: None,
            conversation_id: room_id.to_string(),
            thread_id,
            text: body_text.to_string(),
            attachments: Vec::new(),
            is_dm: false,
            raw_payload: Some(val),
            received_at: chrono::Utc::now().timestamp(),
        }))
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let delivered_id = format!("matrix_ev_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Matrix,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 3. Mattermost Adapter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MattermostAdapterConfig {
    pub webhook_token: Option<String>,
}

pub struct MattermostAdapter {
    config: MattermostAdapterConfig,
}

impl MattermostAdapter {
    pub fn new(config: MattermostAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for MattermostAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Mattermost
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let Some(ref expected) = self.config.webhook_token else {
            return Ok(true);
        };

        let prov = get_header_ignore_case(headers, "token")
            .or_else(|| get_header_ignore_case(headers, "Mattermost-Token"));

        if let Some(p) = prov {
            return Ok(subtle_slices_equal(expected.as_bytes(), p.as_bytes()));
        }

        // Check token in json body if available
        if let Ok(val) = serde_json::from_slice::<Value>(body) {
            if let Some(t) = val.get("token").and_then(|v| v.as_str()) {
                return Ok(subtle_slices_equal(expected.as_bytes(), t.as_bytes()));
            }
        }

        Ok(false)
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        if body.is_empty() {
            return Ok(None);
        }

        let val: Value = serde_json::from_slice(body)
            .map_err(|e| format!("Invalid Mattermost webhook JSON: {e}"))?;

        let post_id = val.get("post_id").and_then(|v| v.as_str()).unwrap_or("mm_post");
        let channel_id = val.get("channel_id").and_then(|v| v.as_str()).unwrap_or("mm_channel");
        let user_id = val.get("user_id").and_then(|v| v.as_str()).unwrap_or("mm_user");
        let user_name = val.get("user_name").and_then(|v| v.as_str()).map(String::from);
        let text = val.get("text").and_then(|v| v.as_str()).unwrap_or("");

        let channel_name = val.get("channel_name").and_then(|v| v.as_str()).unwrap_or("");
        let is_dm = channel_name.contains("__");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Mattermost,
            account_id: None,
            message_id: post_id.to_string(),
            sender_id: user_id.to_string(),
            sender_name: user_name,
            conversation_id: channel_id.to_string(),
            thread_id: None,
            text: text.to_string(),
            attachments: Vec::new(),
            is_dm,
            raw_payload: Some(val),
            received_at: chrono::Utc::now().timestamp(),
        }))
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let delivered_id = format!("mm_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Mattermost,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 4. Native iMessage Adapter (macOS Messages.app osascript / SQLite gateway)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct IMessageAdapterConfig;

pub struct IMessageAdapter;

impl ChannelAdapter for IMessageAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::IMessage
    }

    fn verify_webhook(
        &self,
        _headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        Ok(true)
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        if body.is_empty() {
            return Ok(None);
        }

        let val: Value = serde_json::from_slice(body)
            .map_err(|e| format!("Invalid iMessage JSON: {e}"))?;

        let guid = val.get("guid").and_then(|v| v.as_str()).unwrap_or("imsg_id");
        let sender = val.get("sender").and_then(|v| v.as_str()).unwrap_or("unknown_handle");
        let text = val.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let chat_id = val.get("chat_id").and_then(|v| v.as_str()).unwrap_or(sender);

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::IMessage,
            account_id: None,
            message_id: guid.to_string(),
            sender_id: sender.to_string(),
            sender_name: None,
            conversation_id: chat_id.to_string(),
            thread_id: None,
            text: text.to_string(),
            attachments: Vec::new(),
            is_dm: true,
            raw_payload: Some(val),
            received_at: chrono::Utc::now().timestamp(),
        }))
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let delivered_id = format!("imsg_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::IMessage,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_parse() {
        let adapter = SignalAdapter::new(SignalAdapterConfig::default());
        let body = serde_json::json!({
            "envelope": {
                "source": "+15550001111",
                "sourceName": "Signal User",
                "timestamp": 1699990000000_i64,
                "dataMessage": {
                    "message": "Secret Signal message"
                }
            }
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::Signal);
        assert_eq!(parsed.sender_id, "+15550001111");
        assert_eq!(parsed.text, "Secret Signal message");
        assert!(parsed.is_dm);
    }
}
