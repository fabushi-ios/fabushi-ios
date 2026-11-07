//! Target Symbol: RecentSharedMediaGiftsComponents
//! Telegram Desktop 1:1 Submodule: `RecentSharedMediaGiftsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SharedMediaGift {
    pub gift_id: u64,
    pub date: i64,
    pub title: String,
}

/// Target Symbol for Module 03 Ledger: RecentSharedMediaGiftsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecentSharedMediaGiftsComponents {
    pub gifts: Vec<SharedMediaGift>,
}

impl RecentSharedMediaGiftsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, gift: SharedMediaGift) {
        self.gifts.push(gift);
    }

    pub fn count(&self) -> usize {
        self.gifts.len()
    }
}
