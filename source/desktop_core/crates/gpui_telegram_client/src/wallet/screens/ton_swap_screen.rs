//! Authentic TON DEX Swap Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwapToken {
    pub symbol: String,
    pub address: String,
    pub decimals: u8,
    pub balance_nanos: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSwapScreen {
    pub from_token: SwapToken,
    pub to_token: SwapToken,
    pub from_amount_text: String,
    pub to_amount_text: String,
    pub slippage_tolerance_pct: f64,
    pub price_impact_pct: f64,
    pub dex_provider: String,
    pub is_fetching_quote: bool,
    pub is_swap_ready: bool,
}

impl TonSwapScreen {
    pub fn new() -> Self {
        Self {
            from_token: SwapToken {
                symbol: "TON".into(),
                address: "native".into(),
                decimals: 9,
                balance_nanos: 10_000_000_000,
            },
            to_token: SwapToken {
                symbol: "USDT".into(),
                address: "EQCxE6mUtQJKFnGfaROTKOt1lZbDiiX1kCixRv7Nw2Id_sDs".into(),
                decimals: 6,
                balance_nanos: 0,
            },
            from_amount_text: "1".into(),
            to_amount_text: "5.50".into(),
            slippage_tolerance_pct: 1.0,
            price_impact_pct: 0.05,
            dex_provider: "STON.fi".into(),
            is_fetching_quote: false,
            is_swap_ready: true,
        }
    }

    pub fn flip_tokens(&mut self) {
        std::mem::swap(&mut self.from_token, &mut self.to_token);
        std::mem::swap(&mut self.from_amount_text, &mut self.to_amount_text);
    }

    pub fn set_slippage(&mut self, pct: f64) {
        self.slippage_tolerance_pct = pct.clamp(0.1, 10.0);
    }

    pub fn update_quote(&mut self, output_amount: impl Into<String>, impact: f64) {
        self.to_amount_text = output_amount.into();
        self.price_impact_pct = impact;
        self.is_fetching_quote = false;
        self.is_swap_ready = true;
    }
}

impl Default for TonSwapScreen {
    fn default() -> Self {
        Self::new()
    }
}
