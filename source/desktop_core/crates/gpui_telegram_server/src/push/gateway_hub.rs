use super::{PushDeliveryReceipt, PushError, PushNotification, PushPlatform};
use crate::db::ServerDatabase;
use async_trait::async_trait;
use gpui_telegram_core::types::UserId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[async_trait]
pub trait PushDispatcher: Send + Sync {
    fn platform(&self) -> PushPlatform;
    async fn dispatch(
        &self,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError>;
}

#[derive(Clone)]
pub struct PushGatewayHub {
    dispatchers: Arc<RwLock<HashMap<PushPlatform, Arc<dyn PushDispatcher>>>>,
    db: Arc<ServerDatabase>,
}

impl PushGatewayHub {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        Self {
            dispatchers: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    pub fn register_dispatcher(&self, dispatcher: Arc<dyn PushDispatcher>) {
        self.dispatchers.write().insert(dispatcher.platform(), dispatcher);
    }

    pub async fn send_to_token(
        &self,
        platform: PushPlatform,
        token: &str,
        notification: &PushNotification,
    ) -> Result<PushDeliveryReceipt, PushError> {
        let dispatcher = self
            .dispatchers
            .read()
            .get(&platform)
            .cloned()
            .ok_or(PushError::NoDispatcher(platform))?;

        dispatcher.dispatch(token, notification).await
    }

    pub async fn send_to_user(
        &self,
        user_id: UserId,
        notification: &PushNotification,
    ) -> Vec<Result<PushDeliveryReceipt, PushError>> {
        let tokens = match self.db.get_user_push_tokens(user_id) {
            Ok(t) => t,
            Err(e) => {
                return vec![Err(PushError::PlatformError(format!(
                    "Failed to query user tokens: {e}"
                )))];
            }
        };

        let mut results = Vec::new();
        for record in tokens {
            if let Some(platform) = PushPlatform::from_str_loose(&record.platform) {
                let res = self.send_to_token(platform, &record.token, notification).await;
                results.push(res);
            }
        }
        results
    }
}
