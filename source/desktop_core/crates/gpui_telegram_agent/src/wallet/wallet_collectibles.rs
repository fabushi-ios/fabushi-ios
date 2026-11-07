//! Authentic TON Wallet Collectibles Gallery Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_collectibles.cpp, Telegram/SourceFiles/wallet/wallet_collectibles.h
//! Subsystem: wallet | Feature ID: CAP-06

use crate::wallet::wallet_collectible_record::WalletCollectibleRecord;
use serde::{Deserialize, Serialize};

pub const K_PRELOAD_SCREENS: usize = 3;
pub const K_PRELOAD_ROWS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCollectibles {
    pub records: Vec<WalletCollectibleRecord>,
    pub is_loading: bool,
    pub total_count: usize,
    pub preload_screens: usize,
    pub preload_rows: usize,
}

impl WalletCollectibles {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            is_loading: false,
            total_count: 0,
            preload_screens: K_PRELOAD_SCREENS,
            preload_rows: K_PRELOAD_ROWS,
        }
    }

    pub fn add_record(&mut self, record: WalletCollectibleRecord) {
        if !self.records.iter().any(|r| r.same_served_gift(&record)) {
            self.records.push(record);
            self.total_count = self.records.len();
        }
    }

    pub fn remove_record(&mut self, address: &str) -> Option<WalletCollectibleRecord> {
        if let Some(pos) = self.records.iter().position(|r| r.address == address) {
            let removed = self.records.remove(pos);
            self.total_count = self.records.len();
            Some(removed)
        } else {
            None
        }
    }

    pub fn find_by_address(&self, address: &str) -> Option<&WalletCollectibleRecord> {
        self.records.iter().find(|r| r.address == address)
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.is_loading = loading;
    }
}

impl Default for WalletCollectibles {
    fn default() -> Self {
        Self::new()
    }
}
