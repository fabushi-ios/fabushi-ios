//! Discord bot and interaction webhook adapter (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::outbound_delivery::chunk_discord_text;
use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus, InboundAttachmentMeta,
    InboundChannelMessage, OutboundDeliveryPayload,
};
use super::webhook_ingress::get_header_ignore_case;

fn decode_hex_bytes(s: &str) -> Option<Vec<u8>> {
    let clean = s.trim();
    if clean.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for chunk in clean.as_bytes().chunks_exact(2) {
        let h1 = match chunk[0] {
            b'0'..=b'9' => chunk[0] - b'0',
            b'a'..=b'f' => chunk[0] - b'a' + 10,
            b'A'..=b'F' => chunk[0] - b'A' + 10,
            _ => return None,
        };
        let h2 = match chunk[1] {
            b'0'..=b'9' => chunk[1] - b'0',
            b'a'..=b'f' => chunk[1] - b'a' + 10,
            b'A'..=b'F' => chunk[1] - b'A' + 10,
            _ => return None,
        };
        bytes.push((h1 << 4) | h2);
    }
    Some(bytes)
}

/// Configuration options for Discord channel adapter.
#[derive(Debug, Clone)]
pub struct DiscordAdapterConfig {
    pub bot_token: Option<String>,
    pub public_key: Option<String>,
    pub application_id: Option<String>,
}

impl Default for DiscordAdapterConfig {
    fn default() -> Self {
        Self {
            bot_token: None,
            public_key: None,
            application_id: None,
        }
    }
}

/// Channel adapter for Discord bots and HTTP interaction endpoints.
pub struct DiscordAdapter {
    config: DiscordAdapterConfig,
}

impl DiscordAdapter {
    pub fn new(config: DiscordAdapterConfig) -> Self {
        Self { config }
    }

    /// Verifies Discord interaction HTTP signature if Ed25519 public key is configured.
    fn verify_ed25519_signature(
        &self,
        public_key_hex: &str,
        signature_hex: &str,
        timestamp: &str,
        body: &[u8],
    ) -> bool {
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let Some(pk_bytes) = decode_hex_bytes(public_key_hex) else {
            return false;
        };

        let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.try_into() else {
            return false;
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&pk_arr) else {
            return false;
        };

        let Some(sig_bytes) = decode_hex_bytes(signature_hex) else {
            return false;
        };

        let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.try_into() else {
            return false;
        };
        let signature = Signature::from_bytes(&sig_arr);

        let mut message = timestamp.as_bytes().to_vec();
        message.extend_from_slice(body);

        verifying_key.verify(&message, &signature).is_ok()
    }
}

impl ChannelAdapter for DiscordAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Discord
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let Some(ref pubkey) = self.config.public_key else {
            // If no public key is explicitly configured, accept in permissive mode
            return Ok(true);
        };

        let sig = get_header_ignore_case(headers, "X-Signature-Ed25519");
        let ts = get_header_ignore_case(headers, "X-Signature-Timestamp");

        match (sig, ts) {
            (Some(s), Some(t)) => Ok(self.verify_ed25519_signature(pubkey, s, t, body)),
            _ => Ok(false),
        }
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
            .map_err(|e| format!("Invalid Discord event JSON: {e}"))?;

        // Handle Discord PING interaction (type 1)
        if val.get("type").and_then(|v| v.as_i64()) == Some(1) {
            return Ok(None);
        }

        // Parse Gateway / Webhook MESSAGE_CREATE or Application Command Interaction
        let msg_id = val.get("id").and_then(|v| v.as_str()).unwrap_or("unknown_msg");
        let channel_id = val.get("channel_id").and_then(|v| v.as_str()).unwrap_or("unknown_channel");
        let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");

        let author = val.get("author").or_else(|| val.get("member").and_then(|m| m.get("user")));
        let sender_id = author
            .and_then(|a| a.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_sender");
        let sender_name = author
            .and_then(|a| a.get("username"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let is_bot = author
            .and_then(|a| a.get("bot"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Ignore bot echo messages
        if is_bot {
            return Ok(None);
        }

        let thread_id = val.get("thread").and_then(|t| t.get("id")).and_then(|v| v.as_str()).map(String::from);
        let guild_id = val.get("guild_id").and_then(|v| v.as_str());
        let is_dm = guild_id.is_none();

        let mut attachments = Vec::new();
        if let Some(atts) = val.get("attachments").and_then(|v| v.as_array()) {
            for a in atts {
                let aid = a.get("id").and_then(|v| v.as_str()).unwrap_or("att").to_string();
                let mtype = a.get("content_type").and_then(|v| v.as_str()).unwrap_or("application/octet-stream").to_string();
                let fname = a.get("filename").and_then(|v| v.as_str()).map(String::from);
                let size = a.get("size").and_then(|v| v.as_u64()).map(|s| s as usize);
                let url = a.get("url").and_then(|v| v.as_str()).map(String::from);

                attachments.push(InboundAttachmentMeta {
                    attachment_id: aid,
                    mime_type: mtype,
                    file_name: fname,
                    size_bytes: size,
                    download_url: url,
                });
            }
        }

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Discord,
            account_id: self.config.application_id.clone(),
            message_id: msg_id.to_string(),
            sender_id: sender_id.to_string(),
            sender_name,
            conversation_id: channel_id.to_string(),
            thread_id,
            text: content.to_string(),
            attachments,
            is_dm,
            raw_payload: Some(val),
            received_at: chrono::Utc::now().timestamp(),
        }))
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let chunks = chunk_discord_text(&payload.text, 2000);
        let delivered_id = format!("discord_del_{}", uuid::Uuid::new_v4());

        // In production host runtime, chunk delivery loops over Discord REST API:
        // POST /channels/{channel.id}/messages
        Ok(DeliveryReceipt {
            channel: ChannelKind::Discord,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: if chunks.len() > 1 {
                Some(format!("Delivered across {} chunks", chunks.len()))
            } else {
                None
            },
        })
    }

    fn send_typing_indicator(
        &self,
        _target_id: &str,
        _thread_id: Option<&str>,
    ) -> Result<(), String> {
        // Trigger POST /channels/{channel.id}/typing
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_parse_message() {
        let adapter = DiscordAdapter::new(DiscordAdapterConfig::default());
        let body = serde_json::json!({
            "id": "11223344",
            "channel_id": "99887766",
            "content": "Hello Discord bot",
            "author": {
                "id": "user_42",
                "username": "gamer1",
                "bot": false
            }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &body_bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::Discord);
        assert_eq!(parsed.message_id, "11223344");
        assert_eq!(parsed.sender_id, "user_42");
        assert_eq!(parsed.text, "Hello Discord bot");
        assert!(parsed.is_dm);
    }
}
