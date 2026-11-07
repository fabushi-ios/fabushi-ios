//! Authentic TON Wallet Fiat Conversion & Formatting Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_fiat.cpp, Telegram/SourceFiles/wallet/wallet_fiat.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_NANOS_IN_ONE: f64 = 1_000_000_000.0;
pub const K_TINY_FIAT_SCALE: i32 = 15;
pub const K_GRAM_DIGITS: i32 = 9;
pub const K_FEE_FIAT_DECIMALS: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FiatRate {
    pub currency: String,
    pub per_gram: f64,
}

impl FiatRate {
    pub fn new(currency: impl Into<String>, per_gram: f64) -> Self {
        Self {
            currency: currency.into(),
            per_gram,
        }
    }

    pub fn is_available(&self) -> bool {
        self.per_gram > 0.0
    }

    pub fn convert_nanos(&self, nanos: i64) -> f64 {
        (nanos as f64 / K_NANOS_IN_ONE) * self.per_gram
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletFiat {
    pub rates: Vec<FiatRate>,
    pub selected_currency: String,
}

impl WalletFiat {
    pub fn new(currency: impl Into<String>) -> Self {
        Self {
            rates: Vec::new(),
            selected_currency: currency.into(),
        }
    }

    pub fn set_rate(&mut self, currency: impl Into<String>, rate: f64) {
        let cur = currency.into();
        if let Some(pos) = self.rates.iter().position(|r| r.currency == cur) {
            self.rates[pos].per_gram = rate;
        } else {
            self.rates.push(FiatRate::new(cur, rate));
        }
    }

    pub fn get_rate(&self, currency: &str) -> Option<&FiatRate> {
        self.rates.iter().find(|r| r.currency == currency)
    }

    pub fn format_fiat(&self, nanos: i64) -> String {
        let symbol = match self.selected_currency.as_str() {
            "USD" => "$",
            "EUR" => "€",
            "GBP" => "£",
            "RUB" => "₽",
            _ => "",
        };

        if let Some(rate) = self.get_rate(&self.selected_currency) {
            let val = rate.convert_nanos(nanos);
            format!("{}{:.2}", symbol, val)
        } else {
            format!("{:.2} {}", nanos as f64 / K_NANOS_IN_ONE, self.selected_currency)
        }
    }
}

impl Default for WalletFiat {
    fn default() -> Self {
        Self::new("USD")
    }
}
