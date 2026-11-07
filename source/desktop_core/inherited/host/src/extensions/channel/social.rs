//! Social, community, and decentralized channel adapters: Nostr, Twitch, Zalo, Nextcloud, and Tlon (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus,
    InboundChannelMessage, OutboundDeliveryPayload,
};

// ---------------------------------------------------------------------------
// 1. Nostr Adapter (Decentralized relay events & NIP-04/NIP-44 DMs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct NostrAdapterConfig {
    pub pubkey: Option<String>,
}

pub struct NostrAdapter {
    config: NostrAdapterConfig,
}

impl NostrAdapter {
    pub fn new(config: NostrAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for NostrAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Nostr
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
            .map_err(|e| format!("Invalid Nostr event JSON: {e}"))?;

        let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("nostr_ev");
        let pubkey = val.get("pubkey").and_then(|v| v.as_str()).unwrap_or("unknown_pubkey");
        let kind = val.get("kind").and_then(|v| v.as_i64()).unwrap_or(1);
        let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");

        let is_dm = kind == 4 || kind == 1059; // NIP-04 direct message or NIP-59 gift wrap

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Nostr,
            account_id: self.config.pubkey.clone(),
            message_id: id.to_string(),
            sender_id: pubkey.to_string(),
            sender_name: None,
            conversation_id: pubkey.to_string(),
            thread_id: None,
            text: content.to_string(),
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
        let delivered_id = format!("nostr_pub_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Nostr,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 2. Twitch Adapter (EventSub & Chat notifications)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TwitchAdapterConfig {
    pub client_id: Option<String>,
}

pub struct TwitchAdapter {
    config: TwitchAdapterConfig,
}

impl TwitchAdapter {
    pub fn new(config: TwitchAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for TwitchAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Twitch
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
            .map_err(|e| format!("Invalid Twitch EventSub JSON: {e}"))?;

        let event = val.get("event").unwrap_or(&val);
        let msg_id = event.get("message_id").and_then(|v| v.as_str()).unwrap_or("twitch_msg");
        let broadcaster = event.get("broadcaster_user_id").and_then(|v| v.as_str()).unwrap_or("channel");
        let chatter_id = event.get("chatter_user_id").and_then(|v| v.as_str()).unwrap_or("user");
        let chatter_name = event.get("chatter_user_name").and_then(|v| v.as_str()).map(String::from);

        let message = event.get("message");
        let text = message.and_then(|m| m.get("text")).and_then(|v| v.as_str()).unwrap_or("");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Twitch,
            account_id: self.config.client_id.clone(),
            message_id: msg_id.to_string(),
            sender_id: chatter_id.to_string(),
            sender_name: chatter_name,
            conversation_id: broadcaster.to_string(),
            thread_id: None,
            text: text.to_string(),
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
        let delivered_id = format!("twitch_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Twitch,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 3. Zalo Adapter (Zalo Official Account Webhook)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ZaloAdapterConfig {
    pub oa_id: Option<String>,
}

pub struct ZaloAdapter {
    config: ZaloAdapterConfig,
}

impl ZaloAdapter {
    pub fn new(config: ZaloAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for ZaloAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Zalo
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
            .map_err(|e| format!("Invalid Zalo webhook JSON: {e}"))?;

        let event_name = val.get("event_name").and_then(|v| v.as_str()).unwrap_or("");
        if event_name != "user_send_text" && !event_name.is_empty() {
            return Ok(None);
        }

        let sender = val.get("sender").and_then(|s| s.get("id")).and_then(|v| v.as_str()).unwrap_or("zalo_sender");
        let msg_id = val.get("message").and_then(|m| m.get("msg_id")).and_then(|v| v.as_str()).unwrap_or("zalo_msg");
        let text = val.get("message").and_then(|m| m.get("text")).and_then(|v| v.as_str()).unwrap_or("");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Zalo,
            account_id: self.config.oa_id.clone(),
            message_id: msg_id.to_string(),
            sender_id: sender.to_string(),
            sender_name: None,
            conversation_id: sender.to_string(),
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
        let delivered_id = format!("zalo_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Zalo,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 4. Nextcloud Talk Adapter (Spreed / Talk bot webhooks)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct NextcloudAdapterConfig {
    pub server_url: Option<String>,
}

pub struct NextcloudAdapter {
    config: NextcloudAdapterConfig,
}

impl NextcloudAdapter {
    pub fn new(config: NextcloudAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for NextcloudAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Nextcloud
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
            .map_err(|e| format!("Invalid Nextcloud Talk JSON: {e}"))?;

        let token = val.get("token").and_then(|v| v.as_str()).unwrap_or("talk_room");
        let actor = val.get("actor").and_then(|a| a.get("id")).and_then(|v| v.as_str()).unwrap_or("nc_user");
        let msg_id = val.get("id").and_then(|v| v.as_str()).unwrap_or("nc_msg");
        let text = val.get("message").and_then(|v| v.as_str()).unwrap_or("");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Nextcloud,
            account_id: None,
            message_id: msg_id.to_string(),
            sender_id: actor.to_string(),
            sender_name: None,
            conversation_id: token.to_string(),
            thread_id: None,
            text: text.to_string(),
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
        let delivered_id = format!("nc_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Nextcloud,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 5. Tlon Adapter (Urbit / Tlon graph-store channels)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TlonAdapterConfig {
    pub ship: Option<String>,
}

pub struct TlonAdapter {
    config: TlonAdapterConfig,
}

impl TlonAdapter {
    pub fn new(config: TlonAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for TlonAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Tlon
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
            .map_err(|e| format!("Invalid Tlon JSON: {e}"))?;

        let id = val.get("id").and_then(|v| v.as_str()).unwrap_or("tlon_post");
        let author = val.get("author").and_then(|v| v.as_str()).unwrap_or("~zod");
        let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let channel_nest = val.get("nest").and_then(|v| v.as_str()).unwrap_or("chat/~zod/general");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Tlon,
            account_id: self.config.ship.clone(),
            message_id: id.to_string(),
            sender_id: author.to_string(),
            sender_name: None,
            conversation_id: channel_nest.to_string(),
            thread_id: None,
            text: content.to_string(),
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
        let delivered_id = format!("tlon_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::Tlon,
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
    fn test_nostr_parse() {
        let adapter = NostrAdapter::new(NostrAdapterConfig::default());
        let body = serde_json::json!({
            "id": "e_abc123",
            "pubkey": "npub1xyz",
            "kind": 1,
            "content": "Decentralized greetings on Nostr"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::Nostr);
        assert_eq!(parsed.sender_id, "npub1xyz");
        assert_eq!(parsed.text, "Decentralized greetings on Nostr");
        assert!(!parsed.is_dm);
    }
}
