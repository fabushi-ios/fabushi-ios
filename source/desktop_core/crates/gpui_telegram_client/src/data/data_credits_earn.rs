//! Target Symbol: DataCreditsEarn
//! Telegram Desktop 1:1 Submodule: `DataCreditsEarn`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreditsEarnStatistics {
    pub balance: u64,
    pub available_balance: u64,
    pub overall_revenue: u64,
    pub usd_rate: f64,
}

/// Target Symbol for Module 03 Ledger: DataCreditsEarn coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataCreditsEarn {
    pub stats: CreditsEarnStatistics,
}

impl DataCreditsEarn {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stats(stats: CreditsEarnStatistics) -> Self {
        Self { stats }
    }

    pub fn can_withdraw(&self) -> bool {
        self.stats.available_balance > 0
    }
}
