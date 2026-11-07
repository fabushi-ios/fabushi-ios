//! Target Symbol: GiftAuctionsComponents
//! Telegram Desktop 1:1 Submodule: `GiftAuctionsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GiftAuction {
    pub gift_id: u64,
    pub min_bid: u64,
    pub current_bid: u64,
    pub ends_at: i64,
    pub highest_bidder_id: u64,
}

/// Target Symbol for Module 03 Ledger: GiftAuctionsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GiftAuctionsComponents {
    pub active_auctions: Vec<GiftAuction>,
}

impl GiftAuctionsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn place_bid(&mut self, gift_id: u64, bidder_id: u64, amount: u64) -> bool {
        if let Some(auction) = self.active_auctions.iter_mut().find(|a| a.gift_id == gift_id) {
            if amount > auction.current_bid && amount >= auction.min_bid {
                auction.current_bid = amount;
                auction.highest_bidder_id = bidder_id;
                true
            } else {
                false
            }
        } else {
            false
        }
    }
}
