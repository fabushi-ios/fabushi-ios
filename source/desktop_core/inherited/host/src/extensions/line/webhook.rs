//! LINE Messaging API webhook event models and signature verification (OC-S3).

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::super::channel::types::{ChannelKind, InboundAttachmentMeta, InboundChannelMessage};
use super::super::channel::webhook_ingress::{hmac_sha256, subtle_slices_equal};

/// Verifies LINE webhook request signature header `X-Line-Signature`.
pub fn verify_line_signature(
    channel_secret: &str,
    body: &[u8],
    signature_header: &str,
) -> bool {
    if channel_secret.is_empty() || signature_header.is_empty() {
        return false;
    }

    let digest = hmac_sha256(channel_secret.as_bytes(), body);
    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(digest);

    subtle_slices_equal(signature_header.trim().as_bytes(), expected_b64.as_bytes())
}

/// Source descriptor for a LINE webhook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineEventSource {
    #[serde(rename = "type")]
    pub source_type: String, // "user", "group", or "room"
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    #[serde(rename = "groupId")]
    pub group_id: Option<String>,
    #[serde(rename = "roomId")]
    pub room_id: Option<String>,
}

impl LineEventSource {
    pub fn sender_id(&self) -> String {
        self.user_id
            .clone()
            .or_else(|| self.group_id.clone())
            .or_else(|| self.room_id.clone())
            .unwrap_or_else(|| "unknown_line_sender".to_string())
    }

    pub fn conversation_id(&self) -> String {
        self.group_id
            .clone()
            .or_else(|| self.room_id.clone())
            .or_else(|| self.user_id.clone())
            .unwrap_or_else(|| "unknown_line_conv".to_string())
    }

    pub fn is_dm(&self) -> bool {
        self.source_type == "user"
    }
}

/// Content payload for a LINE message event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineMessageContent {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String, // "text", "image", "file", etc.
    pub text: Option<String>,
    #[serde(rename = "fileName")]
    pub file_name: Option<String>,
    #[serde(rename = "fileSize")]
    pub file_size: Option<usize>,
}

/// A parsed single LINE webhook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineWebhookEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "message", "follow", "unfollow", "join", "leave", "postback"
    pub timestamp: i64,
    pub source: LineEventSource,
    #[serde(rename = "replyToken")]
    pub reply_token: Option<String>,
    pub message: Option<LineMessageContent>,
}

/// Wrapper for the array of events delivered by LINE webhook POST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineWebhookPayload {
    pub destination: Option<String>,
    pub events: Vec<LineWebhookEvent>,
}

/// Parses raw LINE webhook JSON into a list of normalized inbound messages.
pub fn parse_line_webhook_body(
    body: &[u8],
) -> Result<Vec<InboundChannelMessage>, String> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    let payload: LineWebhookPayload = serde_json::from_slice(body)
        .map_err(|e| format!("Invalid LINE webhook payload JSON: {e}"))?;

    let mut results = Vec::new();
    for ev in payload.events {
        if ev.event_type != "message" {
            // Non-message lifecycle events (follow, join, postback)
            continue;
        }

        let Some(ref msg) = ev.message else {
            continue;
        };

        let sender_id = ev.source.sender_id();
        let conv_id = ev.source.conversation_id();
        let is_dm = ev.source.is_dm();

        let mut attachments = Vec::new();
        let text = match msg.message_type.as_str() {
            "text" => msg.text.clone().unwrap_or_default(),
            "image" => {
                attachments.push(InboundAttachmentMeta {
                    attachment_id: msg.id.clone(),
                    mime_type: "image/jpeg".to_string(),
                    file_name: Some(format!("{}.jpg", msg.id)),
                    size_bytes: None,
                    download_url: None,
                });
                "[LINE Image]".to_string()
            }
            "file" => {
                attachments.push(InboundAttachmentMeta {
                    attachment_id: msg.id.clone(),
                    mime_type: "application/octet-stream".to_string(),
                    file_name: msg.file_name.clone(),
                    size_bytes: msg.file_size,
                    download_url: None,
                });
                format!("[LINE File: {}]", msg.file_name.as_deref().unwrap_or(&msg.id))
            }
            other => format!("[LINE message type: {other}]"),
        };

        results.push(InboundChannelMessage {
            channel: ChannelKind::Line,
            account_id: payload.destination.clone(),
            message_id: msg.id.clone(),
            sender_id,
            sender_name: None,
            conversation_id: conv_id,
            thread_id: None,
            text,
            attachments,
            is_dm,
            raw_payload: serde_json::to_value(&ev).ok(),
            received_at: ev.timestamp / 1000,
        });
    }

    Ok(results)
}

/// Checks whether an incoming payload is a LINE follow/unfollow event.
pub fn is_line_follow_event(event: &LineWebhookEvent) -> bool {
    event.event_type == "follow" || event.event_type == "unfollow"
}

/// Checks whether an incoming payload is a group join/leave event.
pub fn is_line_group_membership_event(event: &LineWebhookEvent) -> bool {
    event.event_type == "join" || event.event_type == "leave"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_signature_and_parse() {
        let secret = "secret_channel_123";
        let body = serde_json::json!({
            "destination": "U112233",
            "events": [{
                "type": "message",
                "timestamp": 1690000000000_i64,
                "source": {
                    "type": "user",
                    "userId": "U445566"
                },
                "replyToken": "reply_token_abc",
                "message": {
                    "id": "msg_999",
                    "type": "text",
                    "text": "Hello from LINE user"
                }
            }]
        });

        let body_bytes = serde_json::to_vec(&body).unwrap();
        let digest = hmac_sha256(secret.as_bytes(), &body_bytes);
        let sig = base64::engine::general_purpose::STANDARD.encode(digest);

        assert!(verify_line_signature(secret, &body_bytes, &sig));
        assert!(!verify_line_signature(secret, &body_bytes, "invalid_sig"));

        let parsed = parse_line_webhook_body(&body_bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        let m = &parsed[0];
        assert_eq!(m.channel, ChannelKind::Line);
        assert_eq!(m.sender_id, "U445566");
        assert_eq!(m.text, "Hello from LINE user");
        assert!(m.is_dm);
    }

    #[test]
    fn test_line_lifecycle_events() {
        let ev = LineWebhookEvent {
            event_type: "follow".to_string(),
            timestamp: 1690000000000,
            source: LineEventSource {
                source_type: "user".to_string(),
                user_id: Some("U_friend".to_string()),
                group_id: None,
                room_id: None,
            },
            reply_token: None,
            message: None,
        };
        assert!(is_line_follow_event(&ev));
        assert!(!is_line_group_membership_event(&ev));
    }
}
