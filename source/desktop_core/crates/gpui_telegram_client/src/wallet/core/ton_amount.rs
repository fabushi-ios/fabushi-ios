//! TON amount abstraction handling nanoTON arithmetic and decimal formatting.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub const NANOS_PER_TON: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct TonAmount(pub i64);

impl TonAmount {
    pub const ZERO: Self = Self(0);

    pub fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    pub fn from_ton_f64(ton: f64) -> Option<Self> {
        if !ton.is_finite() || ton < 0.0 {
            return None;
        }
        let nanos = (ton * (NANOS_PER_TON as f64)).round() as i64;
        Some(Self(nanos))
    }

    pub fn as_nanos(&self) -> i64 {
        self.0
    }

    pub fn as_ton_f64(&self) -> f64 {
        (self.0 as f64) / (NANOS_PER_TON as f64)
    }

    pub fn format_ton(&self, max_decimals: usize) -> String {
        let whole = self.0 / NANOS_PER_TON;
        let frac = (self.0 % NANOS_PER_TON).abs();
        if max_decimals == 0 || frac == 0 {
            return whole.to_string();
        }
        let frac_str = format!("{:09}", frac);
        let len = max_decimals.min(9);
        let trimmed = frac_str[..len].trim_end_matches('0');
        if trimmed.is_empty() {
            whole.to_string()
        } else {
            format!("{}.{}", whole, trimmed)
        }
    }
}
