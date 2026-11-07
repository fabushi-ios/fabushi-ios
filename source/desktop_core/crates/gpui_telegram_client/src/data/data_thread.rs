//! Target Symbol: DataThread
//! Telegram Desktop 1:1 Submodule: `data/data_thread`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataThread {
    pub peer_id: u64,
    pub thread_id: i32,
    pub unread_count: u32,
    pub unread_mentions: u32,
    pub unread_reactions: u32,
    pub last_message_id: i32,
    pub is_pinned: bool,
    pub is_muted: bool,
}

impl Default for DataThread {
    fn default() -> Self {
        Self {
            peer_id: 0,
            thread_id: 0,
            unread_count: 0,
            unread_mentions: 0,
            unread_reactions: 0,
            last_message_id: 0,
            is_pinned: false,
            is_muted: false,
        }
    }
}

impl DataThread {
    pub fn new(peer_id: u64, thread_id: i32) -> Self {
        Self {
            peer_id,
            thread_id,
            ..Default::default()
        }
    }

    pub fn mark_read(&mut self) {
        self.unread_count = 0;
        self.unread_mentions = 0;
        self.unread_reactions = 0;
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.is_pinned = pinned;
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
    }
}
