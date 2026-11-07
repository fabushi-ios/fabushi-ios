//! Target Symbol: CreditsComponents
//! Telegram Desktop 1:1 Submodule: `CreditsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreditsBalance {
    pub stars_balance: i64,
    pub ton_balance_nanos: u64,
    pub usd_rate: f64,
    pub withdraw_locked: bool,
}

/// Target Symbol for Module 03 Ledger: CreditsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreditsComponents {
    pub balance: CreditsBalance,
}

impl CreditsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stars(stars: i64, usd_rate: f64) -> Self {
        Self {
            balance: CreditsBalance {
                stars_balance: stars,
                ton_balance_nanos: 0,
                usd_rate,
                withdraw_locked: false,
            },
        }
    }

    pub fn stars(&self) -> i64 {
        self.balance.stars_balance
    }

    pub fn lock_withdraw(&mut self) {
        self.balance.withdraw_locked = true;
    }

    pub fn unlock_withdraw(&mut self) {
        self.balance.withdraw_locked = false;
    }

    pub fn can_withdraw(&self) -> bool {
        !self.balance.withdraw_locked && self.balance.stars_balance > 0
    }
}
