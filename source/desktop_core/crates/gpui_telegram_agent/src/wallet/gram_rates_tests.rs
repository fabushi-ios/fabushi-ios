//! Gram currency conversion rates unit test suite.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_rates_tests.cpp

#![allow(dead_code)]

use super::gram_api_rates::{compute_per_gram, make_currency_rates, CurrencyRateEntry};
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramRatesTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramRatesTestsTests {
    pub pass_count: usize,
    pub fail_count: usize,
}

impl GramRatesTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn test_rates_math(&mut self) -> bool {
        let entries = vec![
            CurrencyRateEntry { code: "USD".into(), value: 1.0 },
            CurrencyRateEntry { code: "EUR".into(), value: 0.92 },
            CurrencyRateEntry { code: "RUB".into(), value: 95.0 },
        ];
        let rates = match make_currency_rates(&entries) {
            Some(r) => r,
            None => {
                self.fail_count += 1;
                return false;
            }
        };

        let usd_per_ton = 5.5; // $5.5 per TON
        let eur_per_ton = compute_per_gram(&rates, "EUR", usd_per_ton);
        if eur_per_ton.is_none() {
            self.fail_count += 1;
            return false;
        }

        let diff = (eur_per_ton.unwrap() - (0.92 * 5.5)).abs();
        if diff > 1e-6 {
            self.fail_count += 1;
            return false;
        }

        self.pass_count += 1;
        true
    }

    pub fn test_invalid_rate(&mut self) -> bool {
        let entries = vec![
            CurrencyRateEntry { code: "BAD".into(), value: -1.0 },
            CurrencyRateEntry { code: "NAN".into(), value: f64::NAN },
        ];
        if make_currency_rates(&entries).is_some() {
            self.fail_count += 1;
            return false;
        }
        self.pass_count += 1;
        true
    }

    pub fn run_all(&mut self) -> bool {
        let t1 = self.test_rates_math();
        let t2 = self.test_invalid_rate();
        t1 && t2
    }
}
