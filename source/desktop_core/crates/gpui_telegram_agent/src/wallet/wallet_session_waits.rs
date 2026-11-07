//! Wallet Session Collectible Waiting Queue
//! Upstream: `Telegram/SourceFiles/wallet/wallet_session_waits.cpp`, `wallet_session_waits.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const EXPECTED_COLLECTIBLE_TIMEOUT_MS: u64 = 60000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectibleWaitEntry {
    pub nft_address: String,
    pub expected_owner: String,
    pub registered_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSessionWaits {
    pub entries: Vec<CollectibleWaitEntry>,
    pub timeout_ms: u64,
}

impl Default for WalletSessionWaits {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletSessionWaits {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            timeout_ms: EXPECTED_COLLECTIBLE_TIMEOUT_MS,
        }
    }

    pub fn add_wait(&mut self, nft_address: impl Into<String>, expected_owner: impl Into<String>, now_ms: u64) {
        let addr = nft_address.into();
        let owner = expected_owner.into();
        if !self.entries.iter().any(|e| e.nft_address == addr) {
            self.entries.push(CollectibleWaitEntry {
                nft_address: addr,
                expected_owner: owner,
                registered_at_ms: now_ms,
            });
        }
    }

    pub fn check_and_remove(&mut self, nft_address: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.nft_address == nft_address) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn prune_expired(&mut self, now_ms: u64) -> usize {
        let timeout = self.timeout_ms;
        let before = self.entries.len();
        self.entries.retain(|e| now_ms.saturating_sub(e.registered_at_ms) < timeout);
        before.saturating_sub(self.entries.len())
    }

    pub fn is_waiting(&self, nft_address: &str) -> bool {
        self.entries.iter().any(|e| e.nft_address == nft_address)
    }
}
