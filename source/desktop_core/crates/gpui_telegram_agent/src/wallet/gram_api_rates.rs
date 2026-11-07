//! Gram API fiat conversion rates and valuation logic.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_rates.cpp, gram_api_rates.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrencyRates {
    pub values: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrencyRateEntry {
    pub code: String,
    pub value: f64,
}

pub fn usable(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

pub fn lookup_rate(rates: &CurrencyRates, code: &str) -> Option<f64> {
    rates.values.get(&code.to_ascii_uppercase()).copied()
}

pub fn compute_per_gram(rates: &CurrencyRates, code: &str, usd_per_gram: f64) -> Option<f64> {
    let rate = lookup_rate(rates, code)?;
    if !usable(usd_per_gram) {
        return None;
    }
    let result = rate * usd_per_gram;
    if !usable(result) {
        return None;
    }
    Some(result)
}

pub fn make_currency_rates(entries: &[CurrencyRateEntry]) -> Option<CurrencyRates> {
    let mut values = BTreeMap::new();
    for entry in entries {
        if usable(entry.value) {
            values.insert(entry.code.to_ascii_uppercase(), entry.value);
        }
    }
    if values.is_empty() {
        return None;
    }
    Some(CurrencyRates { values })
}

/// Upstream target symbol: GramApiRatesApi
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramApiRatesApi {
    pub rates: Option<CurrencyRates>,
    pub usd_per_ton: f64,
}

impl GramApiRatesApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_usd_price(&mut self, price: f64) {
        if usable(price) {
            self.usd_per_ton = price;
        }
    }

    pub fn update_rates(&mut self, entries: &[CurrencyRateEntry]) -> bool {
        if let Some(r) = make_currency_rates(entries) {
            self.rates = Some(r);
            true
        } else {
            false
        }
    }

    pub fn convert_ton(&self, currency_code: &str, ton_amount: f64) -> Option<f64> {
        let r = self.rates.as_ref()?;
        let per_ton = compute_per_gram(r, currency_code, self.usd_per_ton)?;
        let total = per_ton * ton_amount;
        if usable(total) {
            Some(total)
        } else {
            None
        }
    }
}
