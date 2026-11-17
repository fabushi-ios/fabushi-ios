use super::super::{PushDeliveryReceipt, PushDispatcher, PushError, PushNotification, PushPlatform};
use async_trait::async_trait;
use serde_json::json;

#[derive(Clone, Default)]
pub struct FcmDispatcher {
    project_id: String,
}

impl FcmDispatcher {
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn format_fcm_payload(&self, token: &str, notification: &PushNotification) -> serde_json::Value {
        let mut data_map = serde_json::Map::new();
        if let Some(chat_id) = notification.chat_id {
            data_map.insert("chat_id".to_string(), json!(chat_id.to_string()));
        }
        if let Some(msg_id) = notification.message_id {
            data_map.insert("message_id".to_string(), json!(msg_id.to_string()));
        }
        for (k, v) in &notification.custom_data {
            data_map.insert(k.clone(), json!(v.to_string()));
        }

        json!({
            "message": {
                "token": token,
                "notification": {
                    "title": notification.title,
                    "body": notification.body,
                },
                "android": {
                    "notification": {
                        "sound": notification.sound.as_deref().unwrap_or("default"),
                    }
                },
                "data": data_map,
            }
        })
    }
}

#[async_trait]
impl PushDispatcher for FcmDispatcher {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Fcm
    }

    async fn dispatch(
        &self,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError> {
        if token.trim().is_empty() {
            return Err(PushError::InvalidToken("Empty FCM device token".into()));
        }

        let _payload = self.format_fcm_payload(token, notification);

        // Production FCM HTTP v1 client would send to fcm.googleapis.com/v1/projects/{project_id}/messages:send
        Ok(PushDeliveryReceipt {
            token: token.to_string(),
            platform: PushPlatform::Fcm,
            delivered: true,
            message_id: Some(format!("fcm_{}", uuid::Uuid::new_v4().simple())),
            delivered_at_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
}
