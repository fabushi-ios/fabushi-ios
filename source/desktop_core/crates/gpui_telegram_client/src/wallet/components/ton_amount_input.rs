//! TON Amount Input component managing decimal entry, maximum button, and nanoTON conversion.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonAmountInput {
    pub raw_input: String,
    pub amount_nano: i64,
    pub available_balance_nano: i64,
    pub max_decimals: usize,
    pub currency_ticker: String,
}

impl TonAmountInput {
    pub fn new(available_balance_nano: i64) -> Self {
        Self {
            raw_input: "0".to_string(),
            amount_nano: 0,
            available_balance_nano,
            max_decimals: 9,
            currency_ticker: "TON".to_string(),
        }
    }

    pub fn set_input(&mut self, text: &str) -> bool {
        let clean = text.trim().replace(',', ".");
        if clean.is_empty() {
            self.raw_input = String::new();
            self.amount_nano = 0;
            return true;
        }

        let parts: Vec<&str> = clean.split('.').collect();
        if parts.len() > 2 {
            return false;
        }
        let whole = match parts[0].parse::<i64>() {
            Ok(w) if w >= 0 => w,
            _ if parts[0].is_empty() => 0,
            _ => return false,
        };
        let frac = if parts.len() == 2 {
            let mut f = parts[1].to_string();
            if f.len() > self.max_decimals {
                f.truncate(self.max_decimals);
            }
            while f.len() < 9 {
                f.push('0');
            }
            match f.parse::<i64>() {
                Ok(val) => val,
                Err(_) => return false,
            }
        } else {
            0
        };

        self.amount_nano = whole * 1_000_000_000 + frac;
        self.raw_input = clean;
        true
    }

    pub fn apply_max(&mut self) {
        self.amount_nano = self.available_balance_nano;
        let whole = self.available_balance_nano / 1_000_000_000;
        let frac = self.available_balance_nano % 1_000_000_000;
        if frac == 0 {
            self.raw_input = whole.to_string();
        } else {
            let frac_str = format!("{:09}", frac).trim_end_matches('0').to_string();
            self.raw_input = format!("{}.{}", whole, frac_str);
        }
    }

    pub fn exceeds_balance(&self) -> bool {
        self.amount_nano > self.available_balance_nano
    }
}
