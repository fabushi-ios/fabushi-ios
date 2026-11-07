//! Gram test runner entry and comprehensive integration test coordinator.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_tests_main.cpp

#![allow(dead_code)]

use super::gram_api_tests::GramApiTestsTests;
use super::gram_boc_tests::GramBocTestsTests;
use super::gram_emulate_tests::GramEmulateTestsTests;
use super::gram_nft_tests::GramNftTestsTests;
use super::gram_rates_tests::GramRatesTestsTests;
use super::gram_stream_tests::GramStreamTestsTests;
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramTestsMainTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramTestsMainTests {
    pub suites_total: usize,
    pub suites_passed: usize,
}

impl GramTestsMainTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run_all_suites(&mut self) -> bool {
        self.suites_total = 6;
        self.suites_passed = 0;

        let mut api_tests = GramApiTestsTests::new();
        if api_tests.run_suite() {
            self.suites_passed += 1;
        }

        let mut boc_tests = GramBocTestsTests::new();
        if boc_tests.run_all() {
            self.suites_passed += 1;
        }

        let mut emulate_tests = GramEmulateTestsTests::new();
        if emulate_tests.run_all() {
            self.suites_passed += 1;
        }

        let mut nft_tests = GramNftTestsTests::new();
        if nft_tests.run_all() {
            self.suites_passed += 1;
        }

        let mut rates_tests = GramRatesTestsTests::new();
        if rates_tests.run_all() {
            self.suites_passed += 1;
        }

        let mut stream_tests = GramStreamTestsTests::new();
        if stream_tests.run_all() {
            self.suites_passed += 1;
        }

        self.suites_passed == self.suites_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_gram_subsystem_checks() {
        let mut runner = GramTestsMainTests::new();
        let success = runner.run_all_suites();
        assert!(success, "All Gram checks must pass without failure");
        assert_eq!(runner.suites_passed, 6);
    }
}
