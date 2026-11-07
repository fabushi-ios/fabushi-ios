//! Target Symbol: DataEmojiStatuses
//! Telegram Desktop 1:1 Submodule: `data/data_emoji_statuses`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmojiStatusItem {
    pub custom_emoji_document_id: u64,
    pub expires_at: Option<i64>,
    pub is_collectible: bool,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataEmojiStatuses {
    pub current_status: Option<EmojiStatusItem>,
    pub recent_statuses: Vec<EmojiStatusItem>,
}

impl DataEmojiStatuses {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_status(&mut self, document_id: u64, duration_seconds: Option<i64>) {
        let expires_at = duration_seconds.map(|dur| chrono::Utc::now().timestamp() + dur);
        let item = EmojiStatusItem {
            custom_emoji_document_id: document_id,
            expires_at,
            is_collectible: false,
            title: None,
        };
        self.current_status = Some(item.clone());
        if !self.recent_statuses.iter().any(|s| s.custom_emoji_document_id == document_id) {
            self.recent_statuses.insert(0, item);
            if self.recent_statuses.len() > 16 {
                self.recent_statuses.truncate(16);
            }
        }
    }

    pub fn clear_status(&mut self) {
        self.current_status = None;
    }

    pub fn is_active(&self, now: i64) -> bool {
        if let Some(status) = &self.current_status {
            if let Some(exp) = status.expires_at {
                return now < exp;
            }
            return true;
        }
        false
    }
}
