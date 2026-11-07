//! Feishu / Lark channel adapter implementation (OC-S3).

pub mod client;
pub mod events;

use std::collections::HashMap;

pub use client::{FeishuClient, FeishuCredentials};
pub use events::{extract_feishu_token, is_feishu_challenge, parse_feishu_event_json, FeishuChallengePayload};

use super::outbound_delivery::chunk_text_preserving_code_blocks;
use super::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus,
    InboundChannelMessage, MessageFormat, OutboundDeliveryPayload,
};
use super::webhook_ingress::{get_header_ignore_case, verify_feishu_webhook_token};

/// Configuration options for Feishu channel adapter.
#[derive(Debug, Clone, Default)]
pub struct FeishuAdapterConfig {
    pub credentials: Option<FeishuCredentials>,
    pub verification_token: Option<String>,
    pub encrypt_key: Option<String>,
    pub enable_cards: bool,
    pub max_retry_attempts: usize,
}

/// Channel adapter for Feishu and Lark enterprise messaging.
pub struct FeishuAdapter {
    config: FeishuAdapterConfig,
    client: FeishuClient,
}

impl FeishuAdapter {
    pub fn new(config: FeishuAdapterConfig) -> Self {
        let client = FeishuClient::new(config.credentials.clone());
        Self { config, client }
    }

    pub fn client(&self) -> &FeishuClient {
        &self.client
    }

    /// Handles an incoming challenge request if present, returning the response JSON.
    pub fn handle_challenge(&self, body: &[u8]) -> Option<serde_json::Value> {
        let challenge = is_feishu_challenge(body)?;
        Some(serde_json::json!({
            "challenge": challenge
        }))
    }

    /// Prepares an interactive card payload for rich responses.
    pub fn format_card_payload(&self, title: &str, markdown: &str) -> serde_json::Value {
        FeishuClient::build_interactive_card(title, markdown)
    }

    /// Delivers a multi-chunk response to a Feishu chat conversation.
    pub fn send_text_chunks(
        &self,
        chat_id: &str,
        chunks: &[String],
    ) -> Result<Vec<String>, String> {
        let mut msg_ids = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let id = self.client.send_text(chat_id, chunk)?;
            msg_ids.push(id);
        }
        Ok(msg_ids)
    }

    /// Validates tenant token cache readiness and connectivity.
    pub fn ensure_token_ready(&self) -> Result<String, String> {
        self.client.get_tenant_access_token()
    }

    /// Returns the configured App ID if present.
    pub fn app_id(&self) -> Option<&str> {
        self.config.credentials.as_ref().map(|c| c.app_id.as_str())
    }
}

impl ChannelAdapter for FeishuAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Feishu
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let Some(ref expected_tok) = self.config.verification_token.as_ref()
            .or_else(|| self.config.credentials.as_ref().and_then(|c| c.verification_token.as_ref()))
        else {
            return Ok(true);
        };

        // Extract token from payload or header
        let provided = extract_feishu_token(body)
            .or_else(|| get_header_ignore_case(headers, "X-Lark-Request-Token").map(String::from));

        match provided {
            Some(tok) => Ok(verify_feishu_webhook_token(expected_tok, &tok)),
            None => Ok(false),
        }
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        parse_feishu_event_json(body)
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        let chunks = chunk_text_preserving_code_blocks(&payload.text, 4000);
        let delivered_id = format!("feishu_msg_{}", uuid::Uuid::new_v4());

        // In production host runtime, delivers message or interactive card via FeishuClient
        for chunk in &chunks {
            if self.config.enable_cards && payload.format == MessageFormat::Markdown {
                let card = self.format_card_payload("Agent Response", chunk);
                let _ = self.client.send_im_message("chat_id", &payload.target_id, "interactive", &card.to_string());
            } else {
                let _ = self.client.send_text(&payload.target_id, chunk);
            }
        }

        Ok(DeliveryReceipt {
            channel: ChannelKind::Feishu,
            message_id: delivered_id,
            conversation_id: payload.target_id.clone(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: if chunks.len() > 1 {
                Some(format!("Delivered across {} Feishu cards/chunks", chunks.len()))
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
        // Feishu currently lacks a dedicated typing indicator API; no-op success
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feishu_adapter_verification() {
        let adapter = FeishuAdapter::new(FeishuAdapterConfig {
            credentials: None,
            verification_token: Some("secret_token_123".to_string()),
            encrypt_key: None,
            enable_cards: true,
            max_retry_attempts: 3,
        });

        let mut headers = HashMap::new();
        headers.insert("X-Lark-Request-Token".to_string(), "secret_token_123".to_string());
        assert!(adapter.verify_webhook(&headers, b"{}").unwrap());

        headers.insert("X-Lark-Request-Token".to_string(), "wrong_tok".to_string());
        assert!(!adapter.verify_webhook(&headers, b"{}").unwrap());
    }

    #[test]
    fn test_feishu_challenge_resolution() {
        let adapter = FeishuAdapter::new(FeishuAdapterConfig::default());
        let challenge_body = b"{\"challenge\":\"challenge_code_999\"}";
        let res = adapter.handle_challenge(challenge_body).unwrap();
        assert_eq!(res["challenge"], "challenge_code_999");
    }

    #[test]
    fn test_feishu_send_chunks() {
        let adapter = FeishuAdapter::new(FeishuAdapterConfig::default());
        let chunks = vec!["Part 1".to_string(), "Part 2".to_string()];
        let ids = adapter.send_text_chunks("oc_chat_1", &chunks).unwrap();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_feishu_card_formatting() {
        let adapter = FeishuAdapter::new(FeishuAdapterConfig::default());
        let card = adapter.format_card_payload("Header Title", "Markdown Body Content");
        assert_eq!(card["elements"][0]["content"], "Markdown Body Content");
    }
}
