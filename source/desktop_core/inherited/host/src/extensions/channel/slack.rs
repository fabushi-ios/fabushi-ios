//! Slack Web and Events API channel adapter (OC-S3).

use std::collections::HashMap;
use serde_json::Value;

use super::outbound_delivery::chunk_text_preserving_code_blocks;
use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus, InboundAttachmentMeta,
    InboundChannelMessage, OutboundDeliveryPayload,
};
use super::webhook_ingress::{get_header_ignore_case, verify_slack_signature};

/// Configuration for Slack channel adapter.
#[derive(Debug, Clone)]
pub struct SlackAdapterConfig {
    pub bot_token: Option<String>,
    pub signing_secret: Option<String>,
    pub app_id: Option<String>,
}

impl Default for SlackAdapterConfig {
    fn default() -> Self {
        Self {
            bot_token: None,
            signing_secret: None,
            app_id: None,
        }
    }
}

/// Channel adapter for Slack workspace messaging and Events API webhooks.
pub struct SlackAdapter {
    config: SlackAdapterConfig,
}

impl SlackAdapter {
    pub fn new(config: SlackAdapterConfig) -> Self {
        Self { config }
    }
}

impl ChannelAdapter for SlackAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Slack
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let Some(ref secret) = self.config.signing_secret else {
            // Permissive if no secret configured
            return Ok(true);
        };

        let sig = get_header_ignore_case(headers, "X-Slack-Signature");
        let ts = get_header_ignore_case(headers, "X-Slack-Request-Timestamp");

        match (sig, ts) {
            (Some(s), Some(t)) => Ok(verify_slack_signature(secret, t, body, s)),
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
            .map_err(|e| format!("Invalid Slack event JSON: {e}"))?;

        // Slack URL verification challenge
        if val.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
            return Ok(None);
        }

        // Process event_callback
        let event = if val.get("type").and_then(|v| v.as_str()) == Some("event_callback") {
            val.get("event")
        } else {
            Some(&val)
        };

        let Some(ev) = event else {
            return Ok(None);
        };

        // Filter out bot messages / message_changed
        if let Some(subtype) = ev.get("subtype").and_then(|v| v.as_str()) {
            if subtype == "bot_message" || subtype == "message_changed" || subtype == "message_deleted" {
                return Ok(None);
            }
        }
        if ev.get("bot_id").is_some() {
            return Ok(None);
        }

        let channel_id = ev.get("channel").and_then(|v| v.as_str()).unwrap_or("unknown_channel");
        let user_id = ev.get("user").and_then(|v| v.as_str()).unwrap_or("unknown_user");
        let text = ev.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let ts = ev.get("ts").and_then(|v| v.as_str()).unwrap_or("0");
        let thread_ts = ev.get("thread_ts").and_then(|v| v.as_str()).map(String::from);

        let channel_type = ev.get("channel_type").and_then(|v| v.as_str()).unwrap_or("");
        let is_dm = channel_type == "im" || channel_id.starts_with('D');

        let mut attachments = Vec::new();
        if let Some(files) = ev.get("files").and_then(|v| v.as_array()) {
            for f in files {
                let fid = f.get("id").and_then(|v| v.as_str()).unwrap_or("file").to_string();
                let mtype = f.get("mimetype").and_then(|v| v.as_str()).unwrap_or("application/octet-stream").to_string();
                let name = f.get("name").and_then(|v| v.as_str()).map(String::from);
                let size = f.get("size").and_then(|v| v.as_u64()).map(|s| s as usize);
                let url = f.get("url_private_download").or_else(|| f.get("url_private")).and_then(|v| v.as_str()).map(String::from);

                attachments.push(InboundAttachmentMeta {
                    attachment_id: fid,
                    mime_type: mtype,
                    file_name: name,
                    size_bytes: size,
                    download_url: url,
                });
            }
        }

        Ok(Some(InboundChannelMessage {
            channel: ChannelKind::Slack,
            account_id: self.config.app_id.clone(),
            message_id: ts.to_string(),
            sender_id: user_id.to_string(),
            sender_name: None,
            conversation_id: channel_id.to_string(),
            thread_id: thread_ts,
            text: text.to_string(),
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
        let chunks = chunk_text_preserving_code_blocks(&payload.text, 4000);
        let delivered_id = format!("slack_msg_{}", uuid::Uuid::new_v4());

        // In production host runtime, delivers via Slack Web API:
        // POST https://slack.com/api/chat.postMessage
        Ok(DeliveryReceipt {
            channel: ChannelKind::Slack,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: if chunks.len() > 1 {
                Some(format!("Delivered in {} chunks", chunks.len()))
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
        // Slack RTM / WebSocket typing indicator or assistant thread status
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slack_parse_event() {
        let adapter = SlackAdapter::new(SlackAdapterConfig::default());
        let body = serde_json::json!({
            "type": "event_callback",
            "event": {
                "type": "message",
                "channel": "C123456",
                "user": "U789012",
                "text": "Hello Slack team!",
                "ts": "1672531199.000100",
                "channel_type": "channel"
            }
        });
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let parsed = adapter.parse_webhook_event(&HashMap::new(), &body_bytes).unwrap().unwrap();

        assert_eq!(parsed.channel, ChannelKind::Slack);
        assert_eq!(parsed.sender_id, "U789012");
        assert_eq!(parsed.conversation_id, "C123456");
        assert_eq!(parsed.text, "Hello Slack team!");
        assert!(!parsed.is_dm);
    }
}
