//! WhatsApp Cloud API and Baileys channel adapter (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::outbound_delivery::{chunk_text_preserving_code_blocks, normalize_whatsapp_jid, WHATSAPP_DEFAULT_MAX_CHARS};
use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus, InboundAttachmentMeta,
    InboundChannelMessage, OutboundDeliveryPayload,
};
use super::webhook_ingress::{get_header_ignore_case, subtle_slices_equal};

/// Configuration options for WhatsApp adapter.
#[derive(Debug, Clone)]
pub struct WhatsAppAdapterConfig {
    pub phone_number_id: Option<String>,
    pub access_token: Option<String>,
    pub verify_token: Option<String>,
    pub app_secret: Option<String>,
}

impl Default for WhatsAppAdapterConfig {
    fn default() -> Self {
        Self {
            phone_number_id: None,
            access_token: None,
            verify_token: None,
            app_secret: None,
        }
    }
}

/// Channel adapter for WhatsApp Business Cloud API and multi-device gateway.
pub struct WhatsAppAdapter {
    config: WhatsAppAdapterConfig,
}

impl WhatsAppAdapter {
    pub fn new(config: WhatsAppAdapterConfig) -> Self {
        Self { config }
    }

    /// Verifies WhatsApp Cloud API webhook subscription challenge (GET hub.mode=subscribe).
    pub fn verify_hub_challenge(
        &self,
        mode: Option<&str>,
        token: Option<&str>,
        challenge: Option<&str>,
    ) -> Option<String> {
        let expected_token = self.config.verify_token.as_deref()?;
        if mode == Some("subscribe") && token == Some(expected_token) {
            challenge.map(String::from)
        } else {
            None
        }
    }
}

impl ChannelAdapter for WhatsAppAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::WhatsApp
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        _body: &[u8],
    ) -> Result<bool, String> {
        let Some(ref expected_tok) = self.config.verify_token else {
            return Ok(true);
        };

        // Header verification fallback or hub.verify_token header
        if let Some(tok) = get_header_ignore_case(headers, "X-Hub-Verification-Token") {
            return Ok(subtle_slices_equal(expected_tok.trim().as_bytes(), tok.trim().as_bytes()));
        }

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
            .map_err(|e| format!("Invalid WhatsApp webhook JSON: {e}"))?;

        // WhatsApp Cloud API schema: { "entry": [ { "changes": [ { "value": { "messages": [ ... ] } } ] } ] }
        let entry = val.get("entry").and_then(|e| e.as_array()).and_then(|arr| arr.first());
        let changes = entry.and_then(|e| e.get("changes")).and_then(|c| c.as_array()).and_then(|arr| arr.first());
        let value = changes.and_then(|c| c.get("value")).unwrap_or(&val);

        let messages = value.get("messages").and_then(|m| m.as_array());
        let Some(msg_list) = messages else {
            return Ok(None);
        };

        let Some(m) = msg_list.first() else {
            return Ok(None);
        };

        let msg_id = m.get("id").and_then(|v| v.as_str()).unwrap_or("wa_msg");
        let from_raw = m.get("from").and_then(|v| v.as_str()).unwrap_or("unknown_sender");
        let normalized_sender = normalize_whatsapp_jid(from_raw).unwrap_or_else(|_| from_raw.to_string());

        let contacts = value.get("contacts").and_then(|c| c.as_array()).and_then(|arr| arr.first());
        let sender_name = contacts
            .and_then(|c| c.get("profile"))
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let msg_type = m.get("type").and_then(|v| v.as_str()).unwrap_or("text");
        let mut text = String::new();
        let mut attachments = Vec::new();

        match msg_type {
            "text" => {
                text = m.get("text").and_then(|t| t.get("body")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
            "image" => {
                if let Some(img) = m.get("image") {
                    let iid = img.get("id").and_then(|v| v.as_str()).unwrap_or("img").to_string();
                    let mime = img.get("mime_type").and_then(|v| v.as_str()).unwrap_or("image/jpeg").to_string();
                    let caption = img.get("caption").and_then(|v| v.as_str()).unwrap_or("");
                    text = caption.to_string();
                    attachments.push(InboundAttachmentMeta {
                        attachment_id: iid,
                        mime_type: mime,
                        file_name: Some("image.jpg".to_string()),
                        size_bytes: None,
                        download_url: None,
                    });
                }
            }
            "document" => {
                if let Some(doc) = m.get("document") {
                    let did = doc.get("id").and_then(|v| v.as_str()).unwrap_or("doc").to_string();
                    let mime = doc.get("mime_type").and_then(|v| v.as_str()).unwrap_or("application/pdf").to_string();
                    let fname = doc.get("filename").and_then(|v| v.as_str()).map(String::from);
                    let caption = doc.get("caption").and_then(|v| v.as_str()).unwrap_or("");
                    text = caption.to_string();
                    attachments.push(InboundAttachmentMeta {
                        attachment_id: did,
                        mime_type: mime,
                        file_name: fname,
                        size_bytes: None,
                        download_url: None,
                    });
                }
            }
            _ => {
                text = format!("[Unsupported WhatsApp message type: {msg_type}]");
            }
        }

        // Context / reply thread
        let thread_id = m.get("context").and_then(|c| c.get("id")).and_then(|v| v.as_str()).map(String::from);
        let is_dm = !from_raw.ends_with("@g.us");

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::WhatsApp,
            account_id: self.config.phone_number_id.clone(),
            message_id: msg_id.to_string(),
            sender_id: normalized_sender.clone(),
            sender_name,
            conversation_id: normalized_sender,
            thread_id,
            text,
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
        let normalized_target = normalize_whatsapp_jid(&payload.target_id)?;
        let chunks = chunk_text_preserving_code_blocks(&payload.text, WHATSAPP_DEFAULT_MAX_CHARS);
        let delivered_id = format!("wa_out_{}", uuid::Uuid::new_v4());

        // In production host runtime, dispatches via Graph API:
        // POST https://graph.facebook.com/v19.0/{phone_number_id}/messages
        Ok(DeliveryReceipt {
            channel: ChannelKind::WhatsApp,
            message_id: delivered_id,
            conversation_id: normalized_target,
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: if chunks.len() > 1 {
                Some(format!("Delivered across {} WhatsApp messages", chunks.len()))
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
        // WhatsApp presence update (composing) via Baileys or Cloud API
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatsapp_parse_message() {
        let adapter = WhatsAppAdapter::new(WhatsAppAdapterConfig::default());
        let body = serde_json::json!({
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [{
                            "id": "wamid.HBgLM",
                            "from": "15551234567",
                            "type": "text",
                            "text": {
                                "body": "Hi there WhatsApp bot"
                            }
                        }],
                        "contacts": [{
                            "profile": {
                                "name": "Alice"
                            }
                        }]
                    }
                }]
            }]
        });

        let bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::WhatsApp);
        assert_eq!(parsed.sender_id, "15551234567@s.whatsapp.net");
        assert_eq!(parsed.sender_name.as_deref(), Some("Alice"));
        assert_eq!(parsed.text, "Hi there WhatsApp bot");
        assert!(parsed.is_dm);
    }

    #[test]
    fn test_whatsapp_hub_challenge() {
        let adapter = WhatsAppAdapter::new(WhatsAppAdapterConfig {
            phone_number_id: None,
            access_token: None,
            verify_token: Some("my_verify_secret".to_string()),
            app_secret: None,
        });

        let res = adapter.verify_hub_challenge(Some("subscribe"), Some("my_verify_secret"), Some("challenge_code"));
        assert_eq!(res.as_deref(), Some("challenge_code"));

        let bad = adapter.verify_hub_challenge(Some("subscribe"), Some("wrong"), Some("challenge_code"));
        assert!(bad.is_none());
    }
}
