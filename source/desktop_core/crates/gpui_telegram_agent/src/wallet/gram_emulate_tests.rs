//! Gram transaction emulation unit test suite.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_emulate_tests.cpp

#![allow(dead_code)]

use super::gram_api_emulate::{emulate_trace_request, parse_emulated_trace};
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramEmulateTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramEmulateTestsTests {
    pub pass_count: usize,
    pub fail_count: usize,
}

impl GramEmulateTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn test_request_payload(&mut self) -> bool {
        let req = emulate_trace_request("dGVzdGJvYw==");
        if !req.post || req.endpoint != "/api/emulate/v1/emulateTrace" {
            self.fail_count += 1;
            return false;
        }
        let val: serde_json::Value = match serde_json::from_slice(&req.payload) {
            Ok(v) => v,
            Err(_) => {
                self.fail_count += 1;
                return false;
            }
        };
        if val.get("boc").and_then(|v| v.as_str()) != Some("dGVzdGJvYw==") {
            self.fail_count += 1;
            return false;
        }
        if val.get("ignore_chksig").and_then(|v| v.as_bool()) != Some(true) {
            self.fail_count += 1;
            return false;
        }
        self.pass_count += 1;
        true
    }

    pub fn test_trace_parsing(&mut self) -> bool {
        let json_data = br#"{
            "trace": { "tx_hash": "tx12345" },
            "transactions": {
                "tx12345": {
                    "account": "0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                    "total_fees": "15000000",
                    "description": {
                        "aborted": false,
                        "compute_ph": { "success": true, "exit_code": 0 },
                        "action": { "success": true, "result_code": 0 }
                    }
                }
            }
        }"#;

        let trace = match parse_emulated_trace(json_data) {
            Some(t) => t,
            None => {
                self.fail_count += 1;
                return false;
            }
        };

        if trace.fee_nano != 15_000_000 {
            self.fail_count += 1;
            return false;
        }

        self.pass_count += 1;
        true
    }

    pub fn run_all(&mut self) -> bool {
        let r1 = self.test_request_payload();
        let r2 = self.test_trace_parsing();
        r1 && r2
    }
}
