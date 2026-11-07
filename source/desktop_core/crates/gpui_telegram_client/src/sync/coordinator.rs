use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::sync::PtsUpdate;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Default, Clone)]
pub struct ClientSyncCoordinator {
    last_seen_pts: Arc<RwLock<HashMap<ChatId, u64>>>,
}

impl ClientSyncCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_pts(&self, chat_id: ChatId) -> u64 {
        *self.last_seen_pts.read().get(&chat_id).unwrap_or(&0)
    }

    pub fn apply_update(&self, update: PtsUpdate) -> Result<()> {
        let mut map = self.last_seen_pts.write();
        let last = map.entry(update.chat_id).or_insert(0);

        if update.pts == *last + 1 {
            *last = update.pts;
            Ok(())
        } else if update.pts <= *last {
            // Idempotent duplicate update
            Ok(())
        } else {
            // Gap detected! Needs difference sync
            Err(TelegramError::PtsSequenceGap {
                last_seen: *last,
                incoming: update.pts,
            })
        }
    }
}
