//! Gram test framework types and verification helpers.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_tests.h

#![allow(dead_code)]

use super::gram_api_request::HttpRequest;
use serde::{Deserialize, Serialize};

pub struct Check {
    pub name: String,
    pub run: Box<dyn Fn() -> Result<(), String> + Send + Sync>,
}

pub fn check_request(
    got: &HttpRequest,
    post: bool,
    endpoint: &str,
    query: &str,
    payload: &[u8],
) -> Result<(), String> {
    if got.post != post {
        return Err(format!("post: got {}, expected {}", got.post, post));
    }
    if got.endpoint != endpoint {
        return Err(format!("endpoint: got {}, expected {}", got.endpoint, endpoint));
    }
    if got.query != query {
        return Err(format!("query: got {}, expected {}", got.query, query));
    }
    if got.payload != payload {
        return Err(format!(
            "payload mismatch: got {} bytes, expected {} bytes",
            got.payload.len(),
            payload.len()
        ));
    }
    Ok(())
}

pub fn read_fixture_bytes(name: &str) -> Option<Vec<u8>> {
    let path = format!("crates/gpui_telegram_agent/assets/fixtures/{}", name);
    std::fs::read(&path).ok().or_else(|| {
        let fallback = format!("../../crates/gpui_telegram_agent/assets/fixtures/{}", name);
        std::fs::read(&fallback).ok()
    })
}

/// Upstream target symbol: GramTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramTestsTests {
    pub passed_count: usize,
    pub failed_count: usize,
}

impl GramTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_result(&mut self, ok: bool) {
        if ok {
            self.passed_count += 1;
        } else {
            self.failed_count += 1;
        }
    }

    pub fn is_all_passed(&self) -> bool {
        self.failed_count == 0 && self.passed_count > 0
    }
}
