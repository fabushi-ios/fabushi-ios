//! Target Symbol: DataCredits
//! Telegram Desktop 1:1 Submodule: `DataCredits`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreditTopupOption {
    pub credits: u64,
    pub product: String,
    pub currency: String,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CreditsHistoryEntry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub date: i64,
    pub credits: i64,
    pub peer_id: u64,
}

/// Target Symbol for Module 03 Ledger: DataCredits coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataCredits {
    pub balance: i64,
    pub history: Vec<CreditsHistoryEntry>,
}

impl DataCredits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_balance(balance: i64) -> Self {
        Self {
            balance,
            history: Vec::new(),
        }
    }

    pub fn add_transaction(&mut self, entry: CreditsHistoryEntry) {
        self.balance += entry.credits;
        self.history.push(entry);
    }
}
