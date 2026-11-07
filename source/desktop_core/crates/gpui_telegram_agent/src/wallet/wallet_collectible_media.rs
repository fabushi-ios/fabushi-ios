//! Telegram Desktop 1:1 Parity Submodule: `WalletCollectibleMedia`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_collectible_media.cpp`, `Telegram/SourceFiles/wallet/wallet_collectible_media.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use gpui_telegram_core::error::Result;

pub const RESOLVE_BUDGET: usize = 10;
pub const CHAIN_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectibleMediaEntry {
    pub document_id: u64,
    pub media_url: String,
    pub is_lottie: bool,
    pub is_loaded: bool,
    pub byte_count: usize,
    pub requested_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCollectibleMedia {
    pub entries: HashMap<u64, CollectibleMediaEntry>,
    pub active_request_count: usize,
    pub max_concurrent_budget: usize,
}

impl Default for WalletCollectibleMedia {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCollectibleMedia {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            active_request_count: 0,
            max_concurrent_budget: RESOLVE_BUDGET,
        }
    }

    pub fn request_media(&mut self, document_id: u64, url: &str, is_lottie: bool) -> bool {
        if self.entries.contains_key(&document_id) {
            return true;
        }
        if self.active_request_count >= self.max_concurrent_budget {
            return false;
        }

        self.entries.insert(
            document_id,
            CollectibleMediaEntry {
                document_id,
                media_url: url.to_string(),
                is_lottie,
                is_loaded: false,
                byte_count: 0,
                requested_at_ms: chrono::Utc::now().timestamp_millis() as u64,
            },
        );
        self.active_request_count += 1;
        true
    }

    pub fn mark_loaded(&mut self, document_id: u64, bytes: usize) {
        if let Some(entry) = self.entries.get_mut(&document_id) {
            if !entry.is_loaded {
                entry.is_loaded = true;
                entry.byte_count = bytes;
                self.active_request_count = self.active_request_count.saturating_sub(1);
            }
        }
    }

    pub fn is_loaded(&self, document_id: u64) -> bool {
        self.entries.get(&document_id).map(|e| e.is_loaded).unwrap_or(false)
    }

    pub fn prune_expired(&mut self, now_ms: u64) -> usize {
        let mut expired = Vec::new();
        for (&id, entry) in &self.entries {
            if !entry.is_loaded && now_ms.saturating_sub(entry.requested_at_ms) >= CHAIN_TIMEOUT_MS {
                expired.push(id);
            }
        }
        for id in &expired {
            self.entries.remove(id);
            self.active_request_count = self.active_request_count.saturating_sub(1);
        }
        expired.len()
    }
}
