//! Target Symbol: DataPremiumLimits
//! Telegram Desktop 1:1 Submodule: `DataPremiumLimits`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PremiumLimitPair {
    pub default_limit: i32,
    pub premium_limit: i32,
}

impl Default for PremiumLimitPair {
    fn default() -> Self {
        Self {
            default_limit: 0,
            premium_limit: 0,
        }
    }
}

/// System-wide Telegram Premium feature quotas and limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PremiumLimits {
    pub channels: PremiumLimitPair,
    pub dialog_filters: PremiumLimitPair,
    pub dialog_filters_chats: PremiumLimitPair,
    pub dialogs_pinned: PremiumLimitPair,
    pub caption_length: PremiumLimitPair,
    pub message_length: PremiumLimitPair,
    pub upload_max_mb: PremiumLimitPair,
    pub about_length: PremiumLimitPair,
}

impl PremiumLimits {
    pub fn standard_limits() -> Self {
        Self {
            channels: PremiumLimitPair { default_limit: 500, premium_limit: 1000 },
            dialog_filters: PremiumLimitPair { default_limit: 10, premium_limit: 30 },
            dialog_filters_chats: PremiumLimitPair { default_limit: 100, premium_limit: 200 },
            dialogs_pinned: PremiumLimitPair { default_limit: 5, premium_limit: 10 },
            caption_length: PremiumLimitPair { default_limit: 1024, premium_limit: 4096 },
            message_length: PremiumLimitPair { default_limit: 4096, premium_limit: 4096 },
            upload_max_mb: PremiumLimitPair { default_limit: 2048, premium_limit: 4096 },
            about_length: PremiumLimitPair { default_limit: 70, premium_limit: 140 },
        }
    }

    pub fn channels_limit(&self, is_premium: bool) -> i32 {
        if is_premium { self.channels.premium_limit } else { self.channels.default_limit }
    }

    pub fn upload_max_mb(&self, is_premium: bool) -> i32 {
        if is_premium { self.upload_max_mb.premium_limit } else { self.upload_max_mb.default_limit }
    }
}

/// Target Symbol for Module 03 Ledger: DataPremiumLimits coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPremiumLimits {
    pub limits: PremiumLimits,
}

impl DataPremiumLimits {
    pub fn new() -> Self {
        Self {
            limits: PremiumLimits::standard_limits(),
        }
    }

    pub fn get_limit(&self, is_premium: bool) -> i32 {
        self.limits.channels_limit(is_premium)
    }
}
