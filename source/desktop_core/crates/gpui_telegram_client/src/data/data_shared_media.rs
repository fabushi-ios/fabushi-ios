//! Target Symbol: DataSharedMedia
//! Telegram Desktop 1:1 Submodule: `data/data_shared_media`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use super::data_media_types::SharedMediaType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSharedMedia {
    pub peer_id: u64,
    pub indices: HashMap<SharedMediaType, Vec<i32>>,
    pub total_counts: HashMap<SharedMediaType, usize>,
}

impl Default for DataSharedMedia {
    fn default() -> Self {
        Self {
            peer_id: 0,
            indices: HashMap::new(),
            total_counts: HashMap::new(),
        }
    }
}

impl DataSharedMedia {
    pub fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            indices: HashMap::new(),
            total_counts: HashMap::new(),
        }
    }

    pub fn insert_message(&mut self, media_type: SharedMediaType, msg_id: i32) {
        let list = self.indices.entry(media_type).or_default();
        if !list.contains(&msg_id) {
            list.push(msg_id);
            list.sort_unstable_by(|a, b| b.cmp(a)); // Descending by ID
            let count = list.len();
            self.total_counts.insert(media_type, count);
        }
    }

    pub fn remove_message(&mut self, msg_id: i32) {
        for (mtype, list) in &mut self.indices {
            list.retain(|&id| id != msg_id);
            self.total_counts.insert(*mtype, list.len());
        }
    }

    pub fn get_slice(&self, media_type: SharedMediaType, offset: usize, limit: usize) -> &[i32] {
        if let Some(list) = self.indices.get(&media_type) {
            let start = offset.min(list.len());
            let end = (offset + limit).min(list.len());
            &list[start..end]
        } else {
            &[]
        }
    }

    pub fn count_for(&self, media_type: SharedMediaType) -> usize {
        self.total_counts.get(&media_type).copied().unwrap_or(0)
    }
}
