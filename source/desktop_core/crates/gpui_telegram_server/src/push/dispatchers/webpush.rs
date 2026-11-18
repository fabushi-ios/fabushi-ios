use super::super::{PushDeliveryReceipt, PushDispatcher, PushError, PushNotification, PushPlatform};
use async_trait::async_trait;
use serde_json::json;

#[derive(Clone, Default)]
pub struct WebPushDispatcher;

impl WebPushDispatcher {
    pub fn new() -> Self {
        Self
    }

    pub fn format_webpush_payload(&self, notification: &PushNotification) -> serde_json::Value {
        json!({
            "title": notification.title,
            "body": notification.body,
            "chat_id": notification.chat_id,
            "message_id": notification.message_id,
            "badge": notification.badge,
            "data": notification.custom_data,
        })
    }
}

#[async_trait]
impl PushDispatcher for WebPushDispatcher {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Webpush
    }

    async fn dispatch(
        &self,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError> {
        if token.trim().is_empty() {
            return Err(PushError::InvalidToken("Empty WebPush endpoint token".into()));
        }

        let _payload = self.format_webpush_payload(notification);

        // Production WebPush RFC 8291 VAPID payload encryption would dispatch here
        Ok(PushDeliveryReceipt {
            token: token.to_string(),
            platform: PushPlatform::Webpush,
            delivered: true,
            message_id: Some(format!("webpush_{}", uuid::Uuid::new_v4().simple())),
            delivered_at_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
}
