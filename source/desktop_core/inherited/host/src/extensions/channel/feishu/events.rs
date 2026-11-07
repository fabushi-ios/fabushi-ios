//! Feishu / Lark event schemas and deserialization (OC-S3).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::types::{ChannelKind, InboundAttachmentMeta, InboundChannelMessage};

/// Feishu webhook URL verification challenge payload (v1/v2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuChallengePayload {
    pub challenge: Option<String>,
    pub token: Option<String>,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
}

/// Normalized Feishu message event extracted from webhook event callback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeishuNormalizedEvent {
    pub message_id: String,
    pub chat_id: String,
    pub chat_type: String, // "p2p" or "group"
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub msg_type: String,  // "text", "image", "file", "post"
    pub text_content: String,
    pub root_id: Option<String>,
    pub parent_id: Option<String>,
    pub attachments: Vec<InboundAttachmentMeta>,
}

/// Helper identifying whether an incoming Feishu webhook payload represents an auth challenge.
pub fn is_feishu_challenge(raw: &[u8]) -> Option<String> {
    if let Ok(val) = serde_json::from_slice::<Value>(raw) {
        if let Some(challenge) = val.get("challenge").and_then(|v| v.as_str()) {
            return Some(challenge.to_string());
        }
    }
    None
}

/// Extracts verification token from Feishu raw payload or event header.
pub fn extract_feishu_token(raw: &[u8]) -> Option<String> {
    let val: Value = serde_json::from_slice(raw).ok()?;
    val.get("token")
        .or_else(|| val.get("header").and_then(|h| h.get("token")))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Parses Feishu event callback JSON into a normalized inbound message.
pub fn parse_feishu_event_json(
    raw: &[u8],
) -> Result<Option<InboundChannelMessage>, String> {
    if raw.is_empty() {
        return Ok(None);
    }

    let val: Value = serde_json::from_slice(raw)
        .map_err(|e| format!("Invalid Feishu event JSON: {e}"))?;

    // Check challenge request
    if val.get("challenge").is_some() {
        return Ok(None);
    }

    // Inspect v2 schema: { "schema": "2.0", "header": { ... }, "event": { ... } }
    let event = val.get("event").unwrap_or(&val);
    let message = event.get("message");

    let Some(m) = message else {
        return Ok(None);
    };

    let msg_id = m.get("message_id")
        .and_then(|v| v.as_str())
        .unwrap_or("feishu_msg");

    let chat_id = m.get("chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_chat");

    let chat_type = m.get("chat_type")
        .and_then(|v| v.as_str())
        .unwrap_or("p2p");
    let is_dm = chat_type == "p2p";

    let sender = event.get("sender").or_else(|| m.get("sender"));
    let sender_id = sender
        .and_then(|s| s.get("sender_id"))
        .and_then(|sid| sid.get("open_id").or_else(|| sid.get("user_id")))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown_feishu_sender");

    let sender_type = sender
        .and_then(|s| s.get("sender_type"))
        .and_then(|v| v.as_str());
    // Filter out bot messages
    if sender_type == Some("app") || sender_type == Some("bot") {
        return Ok(None);
    }

    let root_id = m.get("root_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let msg_type = m.get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text");

    let mut text = String::new();
    let mut attachments = Vec::new();

    if let Some(content_str) = m.get("content").and_then(|v| v.as_str()) {
        if let Ok(content_val) = serde_json::from_str::<Value>(content_str) {
            match msg_type {
                "text" => {
                    text = content_val.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                }
                "image" => {
                    let image_key = content_val.get("image_key").and_then(|v| v.as_str()).unwrap_or("");
                    attachments.push(InboundAttachmentMeta {
                        attachment_id: image_key.to_string(),
                        mime_type: "image/jpeg".to_string(),
                        file_name: Some(format!("{image_key}.jpg")),
                        size_bytes: None,
                        download_url: None,
                    });
                }
                "file" => {
                    let file_key = content_val.get("file_key").and_then(|v| v.as_str()).unwrap_or("");
                    let file_name = content_val.get("file_name").and_then(|v| v.as_str()).map(String::from);
                    attachments.push(InboundAttachmentMeta {
                        attachment_id: file_key.to_string(),
                        mime_type: "application/octet-stream".to_string(),
                        file_name,
                        size_bytes: None,
                        download_url: None,
                    });
                }
                _ => {
                    text = content_str.to_string();
                }
            }
        } else {
            text = content_str.to_string();
        }
    }

    Ok(Some(InboundChannelMessage {
        channel: ChannelKind::Feishu,
        account_id: None,
        message_id: msg_id.to_string(),
        sender_id: sender_id.to_string(),
        sender_name: None,
        conversation_id: chat_id.to_string(),
        thread_id: root_id,
        text,
        attachments,
        is_dm,
        raw_payload: Some(val),
        received_at: chrono::Utc::now().timestamp(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_challenge_extraction() {
        let challenge_body = b"{\"challenge\":\"abc12345\",\"token\":\"tok999\",\"type\":\"url_verification\"}";
        assert_eq!(is_feishu_challenge(challenge_body), Some("abc12345".to_string()));
        assert_eq!(extract_feishu_token(challenge_body), Some("tok999".to_string()));
    }

    #[test]
    fn test_feishu_parse_message() {
        let sample = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "tok_feishu_1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_c998877"
                    },
                    "sender_type": "user"
                },
                "message": {
                    "message_id": "om_12345",
                    "chat_id": "oc_67890",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"Hello Feishu!\"}"
                }
            }
        });

        let bytes = serde_json::to_vec(&sample).unwrap();
        let parsed = parse_feishu_event_json(&bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::Feishu);
        assert_eq!(parsed.sender_id, "ou_c998877");
        assert_eq!(parsed.conversation_id, "oc_67890");
        assert_eq!(parsed.text, "Hello Feishu!");
        assert!(parsed.is_dm);
    }
}
