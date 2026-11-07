//! Enterprise and cloud chat adapters: BlueBubbles, Google Chat, and MS Teams (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus,
    InboundChannelMessage, OutboundDeliveryPayload,
};
use super::webhook_ingress::{
    get_header_ignore_case, verify_bluebubbles_webhook_auth, verify_googlechat_bearer,
    verify_msteams_bearer_jwt_shape,
};

// ---------------------------------------------------------------------------
// 1. BlueBubbles Adapter (iMessage macOS Server bridge)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct BlueBubblesAdapterConfig {
    pub server_url: Option<String>,
    pub password: Option<String>,
}

pub struct BlueBubblesAdapter {
    config: BlueBubblesAdapterConfig,
}

impl BlueBubblesAdapter {
    pub fn new(config: BlueBubblesAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for BlueBubblesAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::BlueBubbles
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let expected = self.config.password.as_deref();
        let provided = get_header_ignore_case(headers, "x-password")
            .or_else(|| get_header_ignore_case(headers, "password"));
        let sig = get_header_ignore_case(headers, "x-signature");

        Ok(verify_bluebubbles_webhook_auth(expected, provided, sig, body))
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
            .map_err(|e| format!("Invalid BlueBubbles event JSON: {e}"))?;

        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type != "new-message" && !event_type.is_empty() {
            return Ok(None);
        }

        let data = val.get("data").unwrap_or(&val);
        let guid = data.get("guid").and_then(|v| v.as_str()).unwrap_or("bb_msg");
        let is_from_me = data.get("isFromMe").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_from_me {
            return Ok(None);
        }

        let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let handle = data.get("handle");
        let sender_id = handle
            .and_then(|h| h.get("address"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_handle");

        let chats = data.get("chats").and_then(|c| c.as_array());
        let chat_guid = chats
            .and_then(|arr| arr.first())
            .and_then(|c| c.get("guid"))
            .and_then(|v| v.as_str())
            .unwrap_or(sender_id);

        let is_dm = !chat_guid.contains(";+;chat");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::BlueBubbles,
            account_id: None,
            message_id: guid.to_string(),
            sender_id: sender_id.to_string(),
            sender_name: None,
            conversation_id: chat_guid.to_string(),
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
        let delivered_id = format!("bb_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::BlueBubbles,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 2. Google Chat Adapter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct GoogleChatAdapterConfig {
    pub expected_audience: Option<String>,
}

pub struct GoogleChatAdapter {
    config: GoogleChatAdapterConfig,
}

impl GoogleChatAdapter {
    pub fn new(config: GoogleChatAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for GoogleChatAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::GoogleChat
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        let Some(auth) = get_header_ignore_case(headers, "Authorization") else {
            return Ok(false);
        };
        verify_googlechat_bearer(auth, self.config.expected_audience.as_deref())
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
            .map_err(|e| format!("Invalid Google Chat event JSON: {e}"))?;

        let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if event_type != "MESSAGE" && !event_type.is_empty() {
            return Ok(None);
        }

        let message = val.get("message").unwrap_or(&val);
        let msg_name = message.get("name").and_then(|v| v.as_str()).unwrap_or("gchat_msg");
        let text = message.get("text").and_then(|v| v.as_str()).unwrap_or("");

        let sender = message.get("sender");
        let sender_id = sender
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("users/unknown");
        let sender_name = sender
            .and_then(|s| s.get("displayName"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let space = message.get("space");
        let space_id = space
            .and_then(|s| s.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("spaces/unknown");
        let space_type = space
            .and_then(|s| s.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("DIRECT_MESSAGE");
        let is_dm = space_type == "DIRECT_MESSAGE";

        let thread_name = message
            .get("thread")
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::GoogleChat,
            account_id: None,
            message_id: msg_name.to_string(),
            sender_id: sender_id.to_string(),
            sender_name,
            conversation_id: space_id.to_string(),
            thread_id: thread_name,
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
        let delivered_id = format!("gchat_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::GoogleChat,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }
}

// ---------------------------------------------------------------------------
// 3. MS Teams Adapter (Bot Framework)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct MSTeamsAdapterConfig {
    pub app_id: Option<String>,
}

pub struct MSTeamsAdapter {
    config: MSTeamsAdapterConfig,
}

impl MSTeamsAdapter {
    pub fn new(config: MSTeamsAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for MSTeamsAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::MSTeams
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        let Some(auth) = get_header_ignore_case(headers, "Authorization") else {
            return Ok(false);
        };
        verify_msteams_bearer_jwt_shape(auth)
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
            .map_err(|e| format!("Invalid MS Teams activity JSON: {e}"))?;

        let act_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if act_type != "message" && !act_type.is_empty() {
            return Ok(None);
        }

        let msg_id = val.get("id").and_then(|v| v.as_str()).unwrap_or("teams_msg");
        let text = val.get("text").and_then(|v| v.as_str()).unwrap_or("");

        let from = val.get("from");
        let sender_id = from
            .and_then(|f| f.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_teams_user");
        let sender_name = from
            .and_then(|f| f.get("name"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let conv = val.get("conversation");
        let conv_id = conv
            .and_then(|c| c.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_teams_conv");
        let conv_type = conv
            .and_then(|c| c.get("conversationType"))
            .and_then(|v| v.as_str())
            .unwrap_or("personal");
        let is_dm = conv_type == "personal";

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::MSTeams,
            account_id: self.config.app_id.clone(),
            message_id: msg_id.to_string(),
            sender_id: sender_id.to_string(),
            sender_name,
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
        let delivered_id = format!("teams_del_{}", uuid::Uuid::new_v4());
        Ok(DeliveryReceipt {
            channel: ChannelKind::MSTeams,
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
    fn test_bluebubbles_parse() {
        let adapter = BlueBubblesAdapter::new(BlueBubblesAdapterConfig::default());
        let body = serde_json::json!({
            "type": "new-message",
            "data": {
                "guid": "imessage-101",
                "text": "Hello via iMessage BlueBubbles",
                "isFromMe": false,
                "handle": {
                    "address": "+15559876543"
                },
                "chats": [
                    { "guid": "iMessage;-;+15559876543" }
                ]
            }
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::BlueBubbles);
        assert_eq!(parsed.sender_id, "+15559876543");
        assert_eq!(parsed.text, "Hello via iMessage BlueBubbles");
        assert!(parsed.is_dm);
    }
}
