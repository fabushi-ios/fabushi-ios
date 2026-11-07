//! Target Symbol: DataChannelEarn
//! Telegram Desktop 1:1 Submodule: `DataChannelEarn`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub type EarnInt = u64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EarnHistorySlice {
    pub total: usize,
    pub all_loaded: bool,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarnStatistics {
    pub current_balance: u64,
    pub available_balance: u64,
    pub overall_revenue: u64,
    pub usd_rate: f64,
    pub switched_off: bool,
}

impl Default for EarnStatistics {
    fn default() -> Self {
        Self {
            current_balance: 0,
            available_balance: 0,
            overall_revenue: 0,
            usd_rate: 0.0,
            switched_off: false,
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataChannelEarn coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChannelEarn {
    pub stats: EarnStatistics,
}

impl DataChannelEarn {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stats(stats: EarnStatistics) -> Self {
        Self { stats }
    }

    pub fn is_monetization_active(&self) -> bool {
        !self.stats.switched_off && self.stats.overall_revenue > 0
    }
}
