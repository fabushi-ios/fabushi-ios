//! Target Symbol: DataPremiumSubscriptionOption
//! Telegram Desktop 1:1 Submodule: `DataPremiumSubscriptionOption`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PremiumSubscriptionOption {
    pub months: i32,
    pub currency: String,
    pub amount: i64,
    pub bot_url: String,
}

/// Target Symbol for Module 03 Ledger: DataPremiumSubscriptionOption coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPremiumSubscriptionOption {
    pub options: Vec<PremiumSubscriptionOption>,
}

impl DataPremiumSubscriptionOption {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, opt: PremiumSubscriptionOption) {
        self.options.push(opt);
    }

    pub fn find_option_for_months(&self, months: i32) -> Option<&PremiumSubscriptionOption> {
        self.options.iter().find(|o| o.months == months)
    }
}
