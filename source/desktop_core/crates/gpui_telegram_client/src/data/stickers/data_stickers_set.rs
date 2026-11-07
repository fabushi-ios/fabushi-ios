//! Target Symbol: DataStickersSetStickers
//! Telegram Desktop 1:1 Submodule: `data/stickers/data_stickers_set`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStickersSetStickers {
    pub id: u64,
    pub access_hash: u64,
    pub title: String,
    pub short_name: String,
    pub count: u32,
    pub hash: i32,
    pub document_ids: Vec<u64>,
    pub is_installed: bool,
    pub is_archived: bool,
    pub is_emoji: bool,
    pub is_text_color: bool,
}

impl Default for DataStickersSetStickers {
    fn default() -> Self {
        Self {
            id: 0,
            access_hash: 0,
            title: String::new(),
            short_name: String::new(),
            count: 0,
            hash: 0,
            document_ids: Vec::new(),
            is_installed: false,
            is_archived: false,
            is_emoji: false,
            is_text_color: false,
        }
    }
}

impl DataStickersSetStickers {
    pub fn new(id: u64, access_hash: u64, title: impl Into<String>, short_name: impl Into<String>) -> Self {
        Self {
            id,
            access_hash,
            title: title.into(),
            short_name: short_name.into(),
            ..Default::default()
        }
    }

    pub fn with_documents(mut self, doc_ids: Vec<u64>) -> Self {
        self.count = doc_ids.len() as u32;
        self.document_ids = doc_ids;
        self
    }
}
