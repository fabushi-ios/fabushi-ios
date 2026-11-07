//! Authentic TON Price Feed Service Implementation
//! Part of Module 13 Wallet Part 4 Client Architecture

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PriceQuote {
    pub price_usd: f64,
    pub change_24h_pct: f64,
    pub last_updated_secs: u64,
}

#[derive(Debug, Clone)]
pub struct TonPriceService {
    quotes: Arc<RwLock<HashMap<String, PriceQuote>>>,
    fiat_rates: Arc<RwLock<HashMap<String, f64>>>,
}

impl TonPriceService {
    pub fn new() -> Self {
        let mut initial_quotes = HashMap::new();
        initial_quotes.insert(
            "TON".into(),
            PriceQuote {
                price_usd: 5.50,
                change_24h_pct: 2.4,
                last_updated_secs: 0,
            },
        );
        initial_quotes.insert(
            "USDT".into(),
            PriceQuote {
                price_usd: 1.00,
                change_24h_pct: 0.0,
                last_updated_secs: 0,
            },
        );
        initial_quotes.insert(
            "NOT".into(),
            PriceQuote {
                price_usd: 0.0075,
                change_24h_pct: -1.2,
                last_updated_secs: 0,
            },
        );

        let mut initial_fiat = HashMap::new();
        initial_fiat.insert("USD".into(), 1.0);
        initial_fiat.insert("EUR".into(), 0.92);
        initial_fiat.insert("GBP".into(), 0.78);
        initial_fiat.insert("RUB".into(), 96.5);

        Self {
            quotes: Arc::new(RwLock::new(initial_quotes)),
            fiat_rates: Arc::new(RwLock::new(initial_fiat)),
        }
    }

    pub fn get_token_price(&self, symbol: &str) -> Option<f64> {
        self.quotes.read().get(symbol).map(|q| q.price_usd)
    }

    pub fn convert_to_fiat(&self, usd_amount: f64, target_fiat: &str) -> f64 {
        let rate = *self.fiat_rates.read().get(target_fiat).unwrap_or(&1.0);
        usd_amount * rate
    }

    pub fn update_quote(&self, symbol: impl Into<String>, price_usd: f64, change: f64, now: u64) {
        self.quotes.write().insert(
            symbol.into(),
            PriceQuote {
                price_usd,
                change_24h_pct: change,
                last_updated_secs: now,
            },
        );
    }
}

impl Default for TonPriceService {
    fn default() -> Self {
        Self::new()
    }
}
