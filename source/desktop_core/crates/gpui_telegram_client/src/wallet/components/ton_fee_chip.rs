//! TON Fee Chip component displaying estimated blockchain transaction fee.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonFeeChip {
    pub fee_nano: i64,
    pub is_estimating: bool,
    pub has_error: bool,
}

impl TonFeeChip {
    pub fn new() -> Self {
        Self {
            fee_nano: 0,
            is_estimating: true,
            has_error: false,
        }
    }

    pub fn set_fee(&mut self, fee_nano: i64) {
        self.fee_nano = fee_nano;
        self.is_estimating = false;
        self.has_error = false;
    }

    pub fn set_error(&mut self) {
        self.has_error = true;
        self.is_estimating = false;
    }

    pub fn display_text(&self) -> String {
        if self.is_estimating {
            "Estimating fee...".to_string()
        } else if self.has_error {
            "Fee estimation failed".to_string()
        } else {
            let ton = (self.fee_nano as f64) / 1_000_000_000.0;
            format!("Fee: ≈ {:.4} TON", ton)
        }
    }
}
