//! Telegram Peer to TON User Addresses Directory & Resolver
//! Upstream: `Telegram/SourceFiles/wallet/wallet_user_addresses.cpp`, `wallet_user_addresses.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const USER_ADDRESSES_PER_REQUEST: usize = 100;
pub const PUBLIC_KEY_SIZE_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedUserAddress {
    pub user_id: i64,
    pub raw_address: String,
    pub bounceable_address: String,
    pub resolved_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletUserAddresses {
    pub cache: HashMap<i64, ResolvedUserAddress>,
    pub pending_resolutions: Vec<i64>,
}

impl Default for WalletUserAddresses {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletUserAddresses {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            pending_resolutions: Vec::new(),
        }
    }

    pub fn insert_resolved(&mut self, user_id: i64, raw: impl Into<String>, bounceable: impl Into<String>, now_ms: u64) {
        let entry = ResolvedUserAddress {
            user_id,
            raw_address: raw.into(),
            bounceable_address: bounceable.into(),
            resolved_at_ms: now_ms,
        };
        self.cache.insert(user_id, entry);
        self.pending_resolutions.retain(|&id| id != user_id);
    }

    pub fn get_address(&self, user_id: i64) -> Option<&ResolvedUserAddress> {
        self.cache.get(&user_id)
    }

    pub fn mark_for_resolution(&mut self, user_id: i64) {
        if !self.cache.contains_key(&user_id) && !self.pending_resolutions.contains(&user_id) {
            self.pending_resolutions.push(user_id);
        }
    }

    pub fn next_batch_to_resolve(&self) -> &[i64] {
        let len = self.pending_resolutions.len().min(USER_ADDRESSES_PER_REQUEST);
        &self.pending_resolutions[..len]
    }
}
