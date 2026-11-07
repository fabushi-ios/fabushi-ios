//! Target Symbol: DataComposeStash
//! Telegram Desktop 1:1 Submodule: `data/data_compose_stash`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataComposeStash {
    pub peer_id: u64,
    pub text: String,
    pub reply_to_msg_id: Option<i32>,
    pub attached_file_paths: Vec<String>,
    pub cursor_position: usize,
    pub timestamp: i64,
}

impl DataComposeStash {
    pub fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            timestamp: chrono::Utc::now().timestamp(),
            ..Default::default()
        }
    }

    pub fn stash(&mut self, text: impl Into<String>, reply_to: Option<i32>, files: Vec<String>) {
        self.text = text.into();
        self.reply_to_msg_id = reply_to;
        self.attached_file_paths = files;
        self.timestamp = chrono::Utc::now().timestamp();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.attached_file_paths.is_empty() && self.reply_to_msg_id.is_none()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.reply_to_msg_id = None;
        self.attached_file_paths.clear();
    }
}
