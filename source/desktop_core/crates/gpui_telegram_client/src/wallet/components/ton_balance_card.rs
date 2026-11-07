//! TON Balance Card component with fiat conversion and action buttons.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonBalanceCard {
    pub balance_nano: i64,
    pub fiat_currency: String,
    pub fiat_amount: f64,
    pub address_short: String,
    pub is_loading: bool,
}

impl TonBalanceCard {
    pub fn new(balance_nano: i64, address: &str) -> Self {
        let addr_short = if address.len() > 8 {
            format!("{}...{}", &address[..4], &address[address.len() - 4..])
        } else {
            address.to_string()
        };
        Self {
            balance_nano,
            fiat_currency: "USD".to_string(),
            fiat_amount: 0.0,
            address_short: addr_short,
            is_loading: false,
        }
    }

    pub fn set_fiat_rate(&mut self, usd_per_ton: f64) {
        let ton = (self.balance_nano as f64) / 1_000_000_000.0;
        self.fiat_amount = ton * usd_per_ton;
    }

    pub fn formatted_ton(&self) -> String {
        let whole = self.balance_nano / 1_000_000_000;
        let frac = (self.balance_nano % 1_000_000_000).abs();
        if frac == 0 {
            format!("{} TON", whole)
        } else {
            let frac_str = format!("{:09}", frac).trim_end_matches('0').to_string();
            format!("{}.{} TON", whole, frac_str)
        }
    }

    pub fn formatted_fiat(&self) -> String {
        format!("≈ ${:.2}", self.fiat_amount)
    }
}
