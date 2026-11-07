//! Authentic TON Jetton Token Detail Screen ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonTokenDetailScreen {
    pub symbol: String,
    pub name: String,
    pub master_address: String,
    pub wallet_address: String,
    pub balance_nanos: u64,
    pub decimals: u8,
    pub price_usd: f64,
    pub change_24h_pct: f64,
    pub price_history_points: Vec<f64>,
}

impl TonTokenDetailScreen {
    pub fn new(
        symbol: impl Into<String>,
        name: impl Into<String>,
        master: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into(),
            name: name.into(),
            master_address: master.into(),
            wallet_address: String::new(),
            balance_nanos: 0,
            decimals: 9,
            price_usd: 1.0,
            change_24h_pct: 0.0,
            price_history_points: Vec::new(),
        }
    }

    pub fn formatted_balance(&self) -> String {
        let divisor = 10u64.pow(self.decimals as u32) as f64;
        let units = self.balance_nanos as f64 / divisor;
        format!("{:.4} {}", units, self.symbol)
    }

    pub fn fiat_value(&self) -> f64 {
        let divisor = 10u64.pow(self.decimals as u32) as f64;
        (self.balance_nanos as f64 / divisor) * self.price_usd
    }

    pub fn set_price_data(&mut self, price: f64, change_24h: f64, points: Vec<f64>) {
        self.price_usd = price;
        self.change_24h_pct = change_24h;
        self.price_history_points = points;
    }
}

impl Default for TonTokenDetailScreen {
    fn default() -> Self {
        Self::new("TON", "The Open Network", "native")
    }
}
