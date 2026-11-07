//! Target Symbol: DataSubscriptions
//! Telegram Desktop 1:1 Submodule: `DataSubscriptions`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PeerSubscription {
    pub credits: u64,
    pub period_days: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubscriptionEntry {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub until: i64,
    pub subscription: PeerSubscription,
    pub bare_peer_id: u64,
    pub cancelled: bool,
    pub expired: bool,
}

/// Target Symbol for Module 03 Ledger: DataSubscriptions coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSubscriptions {
    pub subscriptions: Vec<SubscriptionEntry>,
}

impl DataSubscriptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: SubscriptionEntry) {
        if let Some(pos) = self.subscriptions.iter().position(|s| s.id == entry.id) {
            self.subscriptions[pos] = entry;
        } else {
            self.subscriptions.push(entry);
        }
    }

    pub fn find(&self, id: &str) -> Option<&SubscriptionEntry> {
        self.subscriptions.iter().find(|s| s.id == id)
    }

    pub fn active_subscriptions(&self, now: i64) -> Vec<&SubscriptionEntry> {
        self.subscriptions.iter().filter(|s| !s.cancelled && !s.expired && s.until > now).collect()
    }
}
