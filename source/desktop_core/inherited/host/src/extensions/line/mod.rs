//! LINE extension: Webhook ingress, Messaging API client, and plugin runtime contract (OC-S3).

pub mod client;
pub mod webhook;

use std::collections::HashMap;

pub use client::{LineClient, LineCredentials};
pub use webhook::{
    is_line_follow_event, is_line_group_membership_event, parse_line_webhook_body,
    verify_line_signature, LineEventSource, LineMessageContent, LineWebhookEvent,
    LineWebhookPayload,
};

use super::channel::types::{
    ChannelAdapter, ChannelKind, DeliveryReceipt, DeliveryStatus, InboundChannelMessage,
    OutboundDeliveryPayload,
};

/// Configuration options for the LINE extension.
#[derive(Debug, Clone, Default)]
pub struct LineExtensionConfig {
    pub credentials: Option<LineCredentials>,
    pub enable_loading_animation: bool,
    pub max_retries: usize,
}

/// Primary runtime extension for LINE Messaging API integration.
#[derive(Clone)]
pub struct LineExtension {
    config: LineExtensionConfig,
    client: LineClient,
}

impl LineExtension {
    pub fn new(config: LineExtensionConfig) -> Self {
        let client = LineClient::new(config.credentials.clone());
        Self { config, client }
    }

    pub fn client(&self) -> &LineClient {
        &self.client
    }

    /// Verifies the `X-Line-Signature` header against the configured channel secret.
    pub fn verify_signature(&self, body: &[u8], signature_header: &str) -> bool {
        let Some(ref creds) = self.config.credentials else {
            return true;
        };
        verify_line_signature(&creds.channel_secret, body, signature_header)
    }

    /// Ingests and processes a raw LINE webhook event body.
    pub fn process_webhook_payload(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Vec<InboundChannelMessage>, String> {
        // Verify signature if header is present
        let sig = headers.get("x-line-signature")
            .or_else(|| headers.get("X-Line-Signature"));

        if let Some(s) = sig {
            if !self.verify_signature(body, s) {
                return Err("LINE signature verification failed".to_string());
            }
        }

        parse_line_webhook_body(body)
    }

    /// Sends an outbound response text to a LINE conversation.
    pub fn send_text(&self, to: &str, text: &str) -> Result<DeliveryReceipt, String> {
        let dispatch_id = self.client.push_text(to, text)?;
        Ok(DeliveryReceipt {
            channel: ChannelKind::Line,
            message_id: dispatch_id,
            conversation_id: to.to_string(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }

    /// Sends an outbound rich flex message card to a LINE conversation.
    pub fn send_flex(&self, to: &str, title: &str, text_content: &str) -> Result<DeliveryReceipt, String> {
        let flex = LineClient::build_flex_bubble(title, text_content);
        let dispatch_id = self.client.push_message(to, vec![flex])?;
        Ok(DeliveryReceipt {
            channel: ChannelKind::Line,
            message_id: dispatch_id,
            conversation_id: to.to_string(),
            delivered_at: chrono::Utc::now().timestamp(),
            status: DeliveryStatus::Delivered,
            error_detail: None,
        })
    }

    /// Sends a broadcast / multicast message to multiple LINE recipients.
    pub fn multicast(&self, recipients: &[String], text: &str) -> Result<String, String> {
        let msg = serde_json::json!({
            "type": "text",
            "text": text
        });
        self.client.multicast_message(recipients, vec![msg])
    }

    /// Broadcasts an alert to all channel followers.
    pub fn broadcast(&self, text: &str) -> Result<String, String> {
        let msg = serde_json::json!({
            "type": "text",
            "text": text
        });
        self.client.broadcast_message(vec![msg])
    }

    /// Signals typing / loading indicator for a LINE chat.
    pub fn signal_loading(&self, chat_id: &str) -> Result<(), String> {
        if self.config.enable_loading_animation {
            self.client.display_loading_animation(chat_id, Some(20))
        } else {
            Ok(())
        }
    }
}

/// ChannelAdapter wrapper bridging `LineExtension` into the unified `ChannelRuntime`.
pub struct LineChannelAdapter {
    extension: LineExtension,
}

impl LineChannelAdapter {
    pub fn new(extension: LineExtension) -> Self {
        Self { extension }
    }

    pub fn extension(&self) -> &LineExtension {
        &self.extension
    }
}

impl ChannelAdapter for LineChannelAdapter {
    fn channel_kind(&self) -> ChannelKind {
        ChannelKind::Line
    }

    fn verify_webhook(
        &self,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<bool, String> {
        let sig = headers.get("x-line-signature")
            .or_else(|| headers.get("X-Line-Signature"));
        match sig {
            Some(s) => Ok(self.extension.verify_signature(body, s)),
            None => Ok(self.extension.config.credentials.is_none()),
        }
    }

    fn parse_webhook_event(
        &self,
        _headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Option<InboundChannelMessage>, String> {
        let msgs = parse_line_webhook_body(body)?;
        Ok(msgs.into_iter().next())
    }

    fn deliver(
        &self,
        payload: &OutboundDeliveryPayload,
    ) -> Result<DeliveryReceipt, String> {
        self.extension.send_text(&payload.target_id, &payload.text)
    }

    fn send_typing_indicator(
        &self,
        target_id: &str,
        _thread_id: Option<&str>,
    ) -> Result<(), String> {
        self.extension.signal_loading(target_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_extension_lifecycle() {
        let creds = LineCredentials::new("123", "secret", "tok");
        let ext = LineExtension::new(LineExtensionConfig {
            credentials: Some(creds),
            enable_loading_animation: true,
            max_retries: 3,
        });

        assert_eq!(ext.client().credentials().unwrap().channel_id, "123");
        let res = ext.send_text("U_bob", "Hello Bob").unwrap();
        assert_eq!(res.status, DeliveryStatus::Delivered);

        let flex_res = ext.send_flex("U_bob", "Title", "Body").unwrap();
        assert_eq!(flex_res.status, DeliveryStatus::Delivered);

        let multi_res = ext.multicast(&["U1".into(), "U2".into()], "Notice").unwrap();
        assert!(multi_res.starts_with("line_multi_"));

        let bcast_res = ext.broadcast("Broadcast Alert").unwrap();
        assert!(bcast_res.starts_with("line_bcast_"));
    }
}
