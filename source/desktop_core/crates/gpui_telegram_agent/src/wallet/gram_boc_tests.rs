//! Gram Bag of Cells (BOC) unit test suite.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_boc_tests.cpp

#![allow(dead_code)]

use super::gram_boc::{crc32c, parse_boc, BOC_MAGIC};
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramBocTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramBocTestsTests {
    pub cases_executed: usize,
    pub cases_passed: usize,
}

impl GramBocTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn test_crc32c(&mut self) -> bool {
        self.cases_executed += 1;
        // Known test vector for CRC32-C (Castagnoli)
        // Check empty input
        let empty_crc = crc32c(b"");
        if empty_crc != 0 {
            return false;
        }
        // Check "123456789" -> 0xe3069283
        let test_vec = b"123456789";
        let crc = crc32c(test_vec);
        if crc != 0xE3069283 {
            return false;
        }
        self.cases_passed += 1;
        true
    }

    pub fn test_magic_constant(&mut self) -> bool {
        self.cases_executed += 1;
        if BOC_MAGIC != 0xB5EE9C72 {
            return false;
        }
        // Empty bytes should fail parsing gracefully
        if parse_boc(b"").is_some() {
            return false;
        }
        self.cases_passed += 1;
        true
    }

    pub fn run_all(&mut self) -> bool {
        let ok1 = self.test_crc32c();
        let ok2 = self.test_magic_constant();
        ok1 && ok2
    }
}
