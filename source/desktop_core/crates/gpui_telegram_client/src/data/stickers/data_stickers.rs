//! Target Symbol: DataStickersStickers
//! Telegram Desktop 1:1 Submodule: `data/stickers/data_stickers`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub const STICKERS_DEFAULT_SET_ID: u64 = 0;
pub const STICKERS_RECENT_SET_ID: u64 = 0xFFFFFFFFFFFFFFFE;
pub const STICKERS_FAVED_SET_ID: u64 = 0xFFFFFFFFFFFFFFFA;
pub const STICKERS_FEATURED_SET_ID: u64 = 0xFFFFFFFFFFFFFFFB;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataStickersStickers {
    pub installed_set_ids: Vec<u64>,
    pub recent_document_ids: Vec<u64>,
    pub faved_document_ids: Vec<u64>,
    pub featured_set_ids: Vec<u64>,
}

impl DataStickersStickers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_recent(&mut self, document_id: u64) {
        self.recent_document_ids.retain(|&id| id != document_id);
        self.recent_document_ids.insert(0, document_id);
        if self.recent_document_ids.len() > 64 {
            self.recent_document_ids.truncate(64);
        }
    }

    pub fn toggle_fave(&mut self, document_id: u64) -> bool {
        if let Some(pos) = self.faved_document_ids.iter().position(|&id| id == document_id) {
            self.faved_document_ids.remove(pos);
            false
        } else {
            self.faved_document_ids.insert(0, document_id);
            true
        }
    }

    pub fn is_faved(&self, document_id: u64) -> bool {
        self.faved_document_ids.contains(&document_id)
    }

    pub fn is_installed(&self, set_id: u64) -> bool {
        self.installed_set_ids.contains(&set_id)
    }
}
