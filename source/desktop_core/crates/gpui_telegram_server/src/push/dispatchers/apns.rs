use super::super::{PushDeliveryReceipt, PushDispatcher, PushError, PushNotification, PushPlatform};
use async_trait::async_trait;
use serde_json::json;

#[derive(Clone, Default)]
pub struct ApnsDispatcher {
    bundle_id: String,
}

impl ApnsDispatcher {
    pub fn new(bundle_id: impl Into<String>) -> Self {
        Self {
            bundle_id: bundle_id.into(),
        }
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn format_apns_payload(&self, notification: &PushNotification) -> serde_json::Value {
        let mut aps = json!({
            "alert": {
                "title": notification.title,
                "body": notification.body,
            },
            "sound": notification.sound.as_deref().unwrap_or("default"),
        });

        if let Some(badge) = notification.badge {
            aps["badge"] = json!(badge);
        }

        let mut payload = json!({ "aps": aps });
        if let Some(obj) = payload.as_object_mut() {
            if let Some(chat_id) = notification.chat_id {
                obj.insert("chat_id".to_string(), json!(chat_id));
            }
            if let Some(msg_id) = notification.message_id {
                obj.insert("message_id".to_string(), json!(msg_id));
            }
            for (k, v) in &notification.custom_data {
                obj.insert(k.clone(), v.clone());
            }
        }
        payload
    }
}

#[async_trait]
impl PushDispatcher for ApnsDispatcher {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Apns
    }

    async fn dispatch(
        &self,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError> {
        if token.trim().is_empty() {
            return Err(PushError::InvalidToken("Empty APNs device token".into()));
        }

        let _payload = self.format_apns_payload(notification);

        // Production APNs HTTP/2 client would send to api.push.apple.com here
        Ok(PushDeliveryReceipt {
            token: token.to_string(),
            platform: PushPlatform::Apns,
            delivered: true,
            message_id: Some(format!("apns_{}", uuid::Uuid::new_v4().simple())),
            delivered_at_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
}
