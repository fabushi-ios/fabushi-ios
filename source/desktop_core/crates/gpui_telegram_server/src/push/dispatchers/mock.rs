use super::super::{PushDeliveryReceipt, PushDispatcher, PushError, PushNotification, PushPlatform};
use async_trait::async_trait;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct MockPushDispatcher {
    dispatched: Arc<RwLock<Vec<(String, PushNotification)>>>,
}

impl MockPushDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dispatched_count(&self) -> usize {
        self.dispatched.read().len()
    }

    pub fn get_dispatched(&self) -> Vec<(String, PushNotification)> {
        self.dispatched.read().clone()
    }

    pub fn clear(&self) {
        self.dispatched.write().clear();
    }
}

#[async_trait]
impl PushDispatcher for MockPushDispatcher {
    fn platform(&self) -> PushPlatform {
        PushPlatform::Mock
    }

    async fn dispatch(
        &self,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError> {
        if token.is_empty() {
            return Err(PushError::InvalidToken("Empty mock token".into()));
        }

        self.dispatched.write().push((token.to_string(), notification.clone()));

        Ok(PushDeliveryReceipt {
            token: token.to_string(),
            platform: PushPlatform::Mock,
            delivered: true,
            message_id: Some(uuid::Uuid::new_v4().to_string()),
            delivered_at_ms: chrono::Utc::now().timestamp_millis(),
        })
    }
}
