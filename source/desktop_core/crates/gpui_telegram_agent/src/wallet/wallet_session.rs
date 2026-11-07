//! Telegram Desktop TON Wallet Main Session Controller
//! Upstream: `Telegram/SourceFiles/wallet/wallet_session.cpp`, `wallet_session.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::{Result, TelegramError};

pub const POLL_INTERVAL_MS: u64 = 5000;
pub const COLLECTIBLES_POLL_INTERVAL_MS: u64 = 60000;
pub const STATE_REFRESH_INTERVAL_MS: u64 = 60000;
pub const TRANSFER_MIN_NANOS_DEFAULT: u64 = 100_000_000; // 0.1 TON
pub const TRANSFER_MIN_NANOS_MAX: u64 = 1 << 53;
pub const GASLESS_MIN_NANOS_DEFAULT: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletSessionStage {
    Uninitialized,
    Locked,
    Syncing,
    Active,
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletCapabilities {
    pub can_transfer: bool,
    pub can_deposit: bool,
    pub can_stake: bool,
    pub supports_gasless: bool,
    pub supports_ton_connect: bool,
}

impl Default for WalletCapabilities {
    fn default() -> Self {
        Self {
            can_transfer: true,
            can_deposit: true,
            can_stake: true,
            supports_gasless: true,
            supports_ton_connect: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSession {
    pub account_address: String,
    pub balance_nanos: u64,
    pub locked_nanos: u64,
    pub stage: WalletSessionStage,
    pub capabilities: WalletCapabilities,
    pub collectibles_count: usize,
    pub last_synced_at_ms: u64,
}

impl Default for WalletSession {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletSession {
    pub fn new() -> Self {
        Self {
            account_address: String::new(),
            balance_nanos: 0,
            locked_nanos: 0,
            stage: WalletSessionStage::Uninitialized,
            capabilities: WalletCapabilities::default(),
            collectibles_count: 0,
            last_synced_at_ms: 0,
        }
    }

    pub fn with_address(address: impl Into<String>) -> Self {
        let mut session = Self::new();
        session.account_address = address.into();
        session.stage = WalletSessionStage::Active;
        session
    }

    pub fn balance(&self) -> u64 {
        self.balance_nanos
    }

    pub fn available_balance(&self) -> u64 {
        self.balance_nanos.saturating_sub(self.locked_nanos)
    }

    pub fn set_balance(&mut self, nanos: u64) {
        self.balance_nanos = nanos;
    }

    pub fn is_active(&self) -> bool {
        self.stage == WalletSessionStage::Active
    }

    pub fn can_transfer_amount(&self, nanos: u64) -> bool {
        if !self.capabilities.can_transfer || self.stage != WalletSessionStage::Active {
            return false;
        }
        if nanos < TRANSFER_MIN_NANOS_DEFAULT || nanos > TRANSFER_MIN_NANOS_MAX {
            return false;
        }
        nanos <= self.available_balance()
    }

    pub fn load_more_collectibles(&mut self, count: usize) -> Result<usize> {
        if self.stage == WalletSessionStage::Locked {
            return Err(TelegramError::Internal("Session is locked".into()));
        }
        self.collectibles_count = self.collectibles_count.saturating_add(count);
        Ok(self.collectibles_count)
    }

    pub fn refresh_state(&mut self, timestamp_ms: u64) -> Result<()> {
        self.last_synced_at_ms = timestamp_ms;
        Ok(())
    }
}
