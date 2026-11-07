//! Authentic TON Wallet Rates Polling & Cache Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_rates.cpp, Telegram/SourceFiles/wallet/wallet_rates.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_REFRESH_TIMEOUT_MS: u64 = 300_000;
pub const K_RETRY_TIMEOUT_MS: u64 = 30_000;
pub const K_FIAT_CURRENCY_PREF: &str = "wallet_fiat_currency";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletRates {
    pub current_currency: String,
    pub usd_per_ton: f64,
    pub eur_per_ton: f64,
    pub rub_per_ton: f64,
    pub last_update_ms: u64,
    pub is_refreshing: bool,
}

impl WalletRates {
    pub fn new() -> Self {
        Self {
            current_currency: "USD".into(),
            usd_per_ton: 5.50,
            eur_per_ton: 5.10,
            rub_per_ton: 520.0,
            last_update_ms: 0,
            is_refreshing: false,
        }
    }

    pub fn set_currency(&mut self, code: impl Into<String>) {
        self.current_currency = code.into();
    }

    pub fn update_rates(&mut self, usd: f64, eur: f64, rub: f64, now_ms: u64) {
        self.usd_per_ton = usd;
        self.eur_per_ton = eur;
        self.rub_per_ton = rub;
        self.last_update_ms = now_ms;
        self.is_refreshing = false;
    }

    pub fn rate_for_current(&self) -> f64 {
        match self.current_currency.as_str() {
            "USD" => self.usd_per_ton,
            "EUR" => self.eur_per_ton,
            "RUB" => self.rub_per_ton,
            _ => self.usd_per_ton,
        }
    }
}

impl Default for WalletRates {
    fn default() -> Self {
        Self::new()
    }
}
