//! TON Amount input widget models, currency formatting, and nanoTON scaling.
//! Upstream: Telegram/SourceFiles/wallet/wallet_amount_field.cpp, wallet_amount_field.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const EDIT_DURATION_MS: u64 = 180;
pub const SWITCH_FIRST_DURATION_MS: u64 = 300;
pub const SWITCH_DURATION_MS: u64 = 450;
pub const SWITCH_FADE_DURATION_MS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmountLabel {
    pub prefix: String,
    pub amount: String,
    pub decimal: String,
    pub suffix: String,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmountFieldArgs {
    pub initial_value_nano: i64,
    pub fraction_digits: usize,
    pub separator: String,
    pub is_fiat: bool,
    pub currency: String,
}

impl Default for AmountFieldArgs {
    fn default() -> Self {
        Self {
            initial_value_nano: 0,
            fraction_digits: 9,
            separator: ".".to_string(),
            is_fiat: false,
            currency: "TON".to_string(),
        }
    }
}

pub fn format_nano_amount(value_nano: i64, fraction_digits: usize) -> AmountLabel {
    let sign = if value_nano < 0 { "-" } else { "" };
    let abs_val = value_nano.abs();
    let divisor = 10i64.pow(9);
    let whole = abs_val / divisor;
    let frac = abs_val % divisor;

    let frac_str = format!("{:09}", frac);
    let trimmed_frac = if fraction_digits == 0 {
        String::new()
    } else {
        let max_len = fraction_digits.min(9);
        frac_str[..max_len].trim_end_matches('0').to_string()
    };

    AmountLabel {
        prefix: sign.to_string(),
        amount: whole.to_string(),
        decimal: trimmed_frac,
        suffix: String::new(),
        unit: "TON".to_string(),
    }
}

pub fn parse_input_to_nano(input: &str) -> Option<i64> {
    let clean = input.trim().replace(',', ".");
    if clean.is_empty() {
        return Some(0);
    }
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() > 2 {
        return None;
    }
    let whole: i64 = parts[0].parse().ok()?;
    let frac: i64 = if parts.len() == 2 {
        let mut f_str = parts[1].to_string();
        if f_str.len() > 9 {
            f_str.truncate(9);
        } else {
            while f_str.len() < 9 {
                f_str.push('0');
            }
        }
        f_str.parse().ok()?
    } else {
        0
    };
    Some(whole * 1_000_000_000 + frac)
}

/// Upstream target symbol: WalletAmountField
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAmountField {
    pub value_nano: i64,
    pub currency: String,
    pub is_fiat: bool,
    pub label: AmountLabel,
}

impl WalletAmountField {
    pub fn new(args: AmountFieldArgs) -> Self {
        let label = format_nano_amount(args.initial_value_nano, args.fraction_digits);
        Self {
            value_nano: args.initial_value_nano,
            currency: args.currency,
            is_fiat: args.is_fiat,
            label,
        }
    }

    pub fn set_text_input(&mut self, text: &str) -> bool {
        if let Some(nano) = parse_input_to_nano(text) {
            self.value_nano = nano;
            self.label = format_nano_amount(nano, 9);
            true
        } else {
            false
        }
    }

    pub fn display_string(&self) -> String {
        if self.label.decimal.is_empty() {
            format!("{} {} {}", self.label.prefix, self.label.amount, self.currency)
        } else {
            format!(
                "{} {}.{} {}",
                self.label.prefix, self.label.amount, self.label.decimal, self.currency
            )
        }
    }
}
