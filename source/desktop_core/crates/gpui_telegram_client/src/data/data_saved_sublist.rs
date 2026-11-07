//! Target Symbol: DataSavedSublist
//! Telegram Desktop 1:1 Submodule: `data/data_saved_sublist`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSavedSublist {
    pub sublist_peer_id: u64,
    pub message_ids: Vec<i32>,
    pub full_count: usize,
    pub is_hidden_author: bool,
    pub is_fee_removed: bool,
    pub unread_count: u32,
}

impl Default for DataSavedSublist {
    fn default() -> Self {
        Self {
            sublist_peer_id: 0,
            message_ids: Vec::new(),
            full_count: 0,
            is_hidden_author: false,
            is_fee_removed: false,
            unread_count: 0,
        }
    }
}

impl DataSavedSublist {
    pub fn new(sublist_peer_id: u64) -> Self {
        Self {
            sublist_peer_id,
            ..Default::default()
        }
    }

    pub fn append_message(&mut self, msg_id: i32) {
        if !self.message_ids.contains(&msg_id) {
            self.message_ids.push(msg_id);
            self.full_count = self.message_ids.len();
        }
    }

    pub fn remove_message(&mut self, msg_id: i32) -> bool {
        let initial_len = self.message_ids.len();
        self.message_ids.retain(|&id| id != msg_id);
        self.full_count = self.message_ids.len();
        self.message_ids.len() < initial_len
    }

    pub fn set_hidden_author(&mut self, hidden: bool) {
        self.is_hidden_author = hidden;
    }

    pub fn toggle_fee_removed(&mut self, fee_removed: bool) {
        self.is_fee_removed = fee_removed;
    }
}
