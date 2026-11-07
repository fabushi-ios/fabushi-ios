//! Authentic TON Send Amount Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSendAmountScreen {
    pub token_symbol: String,
    pub available_balance_nanos: u64,
    pub entered_amount_text: String,
    pub fiat_equivalent: f64,
    pub token_usd_rate: f64,
    pub is_valid: bool,
    pub validation_error: Option<String>,
}

impl TonSendAmountScreen {
    pub fn new(token_symbol: impl Into<String>, balance_nanos: u64, rate: f64) -> Self {
        Self {
            token_symbol: token_symbol.into(),
            available_balance_nanos: balance_nanos,
            entered_amount_text: "0".into(),
            fiat_equivalent: 0.0,
            token_usd_rate: rate,
            is_valid: false,
            validation_error: None,
        }
    }

    pub fn set_amount_text(&mut self, text: impl Into<String>) {
        let input = text.into();
        self.entered_amount_text = input;
        self.validate();
    }

    pub fn set_percentage(&mut self, pct: u32) {
        let frac = (pct.min(100) as f64) / 100.0;
        let nanos = (self.available_balance_nanos as f64 * frac) as u64;
        let coins = nanos as f64 / 1_000_000_000.0;
        self.entered_amount_text = format!("{:.4}", coins);
        self.validate();
    }

    pub fn validate(&mut self) -> bool {
        if let Ok(amount) = self.entered_amount_text.parse::<f64>() {
            let nanos = (amount * 1_000_000_000.0) as u64;
            self.fiat_equivalent = amount * self.token_usd_rate;
            if nanos == 0 {
                self.is_valid = false;
                self.validation_error = Some("Enter amount greater than 0".into());
            } else if nanos > self.available_balance_nanos {
                self.is_valid = false;
                self.validation_error = Some("Insufficient balance".into());
            } else {
                self.is_valid = true;
                self.validation_error = None;
            }
        } else {
            self.is_valid = false;
            self.validation_error = Some("Invalid number format".into());
        }
        self.is_valid
    }
}

impl Default for TonSendAmountScreen {
    fn default() -> Self {
        Self::new("TON", 0, 5.0)
    }
}
