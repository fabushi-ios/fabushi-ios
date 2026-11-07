//! Target Symbol: DataSavedMessages
//! Telegram Desktop 1:1 Submodule: `data/data_saved_messages`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSublistHeader {
    pub author_peer_id: u64,
    pub title: String,
    pub message_count: usize,
    pub last_message_id: i32,
    pub last_date: i64,
    pub is_pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSavedMessages {
    pub sublists: HashMap<u64, SavedSublistHeader>,
    pub pinned_sublist_ids: Vec<u64>,
    pub total_saved_count: usize,
}

impl Default for DataSavedMessages {
    fn default() -> Self {
        Self {
            sublists: HashMap::new(),
            pinned_sublist_ids: Vec::new(),
            total_saved_count: 0,
        }
    }
}

impl DataSavedMessages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_or_update_sublist(&mut self, header: SavedSublistHeader) {
        let peer_id = header.author_peer_id;
        self.sublists.insert(peer_id, header);
        self.recalculate_total();
    }

    pub fn remove_sublist(&mut self, peer_id: u64) {
        self.sublists.remove(&peer_id);
        self.pinned_sublist_ids.retain(|&id| id != peer_id);
        self.recalculate_total();
    }

    pub fn pin_sublist(&mut self, peer_id: u64) {
        if let Some(sublist) = self.sublists.get_mut(&peer_id) {
            sublist.is_pinned = true;
            if !self.pinned_sublist_ids.contains(&peer_id) {
                self.pinned_sublist_ids.push(peer_id);
            }
        }
    }

    pub fn unpin_sublist(&mut self, peer_id: u64) {
        if let Some(sublist) = self.sublists.get_mut(&peer_id) {
            sublist.is_pinned = false;
            self.pinned_sublist_ids.retain(|&id| id != peer_id);
        }
    }

    pub fn recalculate_total(&mut self) {
        self.total_saved_count = self.sublists.values().map(|s| s.message_count).sum();
    }
}
