//! Gram API test suite validating error schemas and request formatting.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_api_tests.cpp

#![allow(dead_code)]

use super::gram_api_request::parse_api_error;
use serde::{Deserialize, Serialize};

pub fn check_api_error_case(json_raw: &[u8], expected_msg: &str) -> Result<(), String> {
    let err = parse_api_error(json_raw)
        .ok_or_else(|| "Failed to parse API error from json".to_string())?;
    if !err.message.contains(expected_msg) {
        return Err(format!(
            "Error message mismatch: got '{}', expected substring '{}'",
            err.message, expected_msg
        ));
    }
    Ok(())
}

/// Upstream target symbol: GramApiTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramApiTestsTests {
    pub total_tests: usize,
    pub passed_tests: usize,
}

impl GramApiTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run_suite(&mut self) -> bool {
        let test_cases: [(&[u8], &str); 3] = [
            (
                br#"{"error":"timeout: context deadline exceeded"}"#,
                "timeout: context deadline exceeded",
            ),
            (
                br#"{"error":"failed to decode: schema: error converting value for \"address\""}"#,
                "failed to decode",
            ),
            (
                br#"{"error":"internal server error: Method Not Allowed"}"#,
                "Method Not Allowed",
            ),
        ];

        self.total_tests = test_cases.len();
        self.passed_tests = 0;

        for (json_bytes, expected) in test_cases {
            if check_api_error_case(json_bytes, expected).is_ok() {
                self.passed_tests += 1;
            }
        }

        self.passed_tests == self.total_tests
    }
}
