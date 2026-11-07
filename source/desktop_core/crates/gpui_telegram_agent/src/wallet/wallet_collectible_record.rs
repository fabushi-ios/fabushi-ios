//! Telegram Desktop 1:1 Parity Submodule: `WalletCollectibleRecord`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_collectible_record.cpp`, `Telegram/SourceFiles/wallet/wallet_collectible_record.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectibleStarGift {
    pub gift_id: u64,
    pub slug: String,
    pub title: String,
    pub num: i32,
    pub availability_total: i32,
    pub availability_remains: i32,
}

pub fn same_served_gift_data(
    a_gift: Option<&CollectibleStarGift>,
    b_gift: Option<&CollectibleStarGift>,
) -> bool {
    match (a_gift, b_gift) {
        (None, None) => true,
        (Some(a), Some(b)) => a.gift_id == b.gift_id && a.slug == b.slug,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletCollectibleRecord {
    pub address: String,
    pub name: String,
    pub owner_address: String,
    pub collection_name: String,
    pub init_ton_amount: i64,
    pub gift: Option<CollectibleStarGift>,
}

impl Default for WalletCollectibleRecord {
    fn default() -> Self {
        Self {
            address: String::new(),
            name: String::new(),
            owner_address: String::new(),
            collection_name: String::new(),
            init_ton_amount: 0,
            gift: None,
        }
    }
}

impl WalletCollectibleRecord {
    pub fn new(address: &str, name: &str, owner_address: &str) -> Self {
        Self {
            address: address.to_string(),
            name: name.to_string(),
            owner_address: owner_address.to_string(),
            collection_name: String::new(),
            init_ton_amount: 0,
            gift: None,
        }
    }

    pub fn same_served_gift(&self, other: &Self) -> bool {
        self.address == other.address && same_served_gift_data(self.gift.as_ref(), other.gift.as_ref())
    }

    pub fn is_limited(&self) -> bool {
        if let Some(gift) = &self.gift {
            gift.availability_total > 0
        } else {
            false
        }
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.address == other.address
            && self.name == other.name
            && self.owner_address == other.owner_address
            && same_served_gift_data(self.gift.as_ref(), other.gift.as_ref())
    }
}
