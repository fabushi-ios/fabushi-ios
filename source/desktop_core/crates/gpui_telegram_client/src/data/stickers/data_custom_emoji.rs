//! Target Symbol: DataCustomEmojiStickers
//! Telegram Desktop 1:1 Submodule: `data/stickers/data_custom_emoji`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEmojiEntry {
    pub document_id: u64,
    pub set_id: u64,
    pub alt: String,
    pub is_text_color: bool,
    pub is_free: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataCustomEmojiStickers {
    pub emoji_cache: HashMap<u64, CustomEmojiEntry>,
}

impl DataCustomEmojiStickers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, entry: CustomEmojiEntry) {
        self.emoji_cache.insert(entry.document_id, entry);
    }

    pub fn get_emoji(&self, document_id: u64) -> Option<&CustomEmojiEntry> {
        self.emoji_cache.get(&document_id)
    }

    pub fn is_text_color(&self, document_id: u64) -> bool {
        self.emoji_cache
            .get(&document_id)
            .map(|e| e.is_text_color)
            .unwrap_or(false)
    }
}
