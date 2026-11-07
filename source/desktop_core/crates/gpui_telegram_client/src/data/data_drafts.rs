//! Target Symbol: DataDrafts
//! Telegram Desktop 1:1 Submodule: `data/data_drafts`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DraftKey {
    pub peer_id: u64,
    pub topic_root_id: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftMessage {
    pub text: String,
    pub reply_to_msg_id: Option<i32>,
    pub cursor_position: usize,
    pub date: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataDrafts {
    pub drafts: HashMap<DraftKey, DraftMessage>,
}

impl DataDrafts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_draft(&mut self, peer_id: u64, topic_root_id: i32, draft: DraftMessage) {
        let key = DraftKey {
            peer_id,
            topic_root_id,
        };
        if draft.text.trim().is_empty() {
            self.drafts.remove(&key);
        } else {
            self.drafts.insert(key, draft);
        }
    }

    pub fn get_draft(&self, peer_id: u64, topic_root_id: i32) -> Option<&DraftMessage> {
        let key = DraftKey {
            peer_id,
            topic_root_id,
        };
        self.drafts.get(&key)
    }

    pub fn clear_draft(&mut self, peer_id: u64, topic_root_id: i32) {
        let key = DraftKey {
            peer_id,
            topic_root_id,
        };
        self.drafts.remove(&key);
    }
}
