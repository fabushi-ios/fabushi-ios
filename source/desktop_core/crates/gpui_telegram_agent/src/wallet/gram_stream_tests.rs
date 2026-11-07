//! Gram streaming endpoint and message format unit test suite.
//! Upstream: Telegram/SourceFiles/gram/tests/gram_stream_tests.cpp

#![allow(dead_code)]

use super::gram_api_stream::{
    parse_stream_endpoint, parse_stream_event, stream_ping_message, stream_subscribe_message,
    StreamEndpoint, StreamEventKind, SECURE_PORT,
};
use serde::{Deserialize, Serialize};

/// Upstream target symbol: GramStreamTestsTests
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramStreamTestsTests {
    pub pass_count: usize,
    pub fail_count: usize,
}

impl GramStreamTestsTests {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn test_parse_endpoint(&mut self) -> bool {
        let ep = parse_stream_endpoint("wss://toncenter.com/api/v2/stream");
        if ep != Some(StreamEndpoint {
            host: "toncenter.com".into(),
            request_target: "/api/v2/stream".into(),
            port: SECURE_PORT,
        }) {
            self.fail_count += 1;
            return false;
        }

        let ep_custom_port = parse_stream_endpoint("wss://stream.ton.org:8443/ws?token=xyz");
        if ep_custom_port != Some(StreamEndpoint {
            host: "stream.ton.org".into(),
            request_target: "/ws?token=xyz".into(),
            port: 8443,
        }) {
            self.fail_count += 1;
            return false;
        }

        self.pass_count += 1;
        true
    }

    pub fn test_event_parsing(&mut self) -> bool {
        let frame = br#"{"type":"account_state_change","account":"0:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#;
        let event = parse_stream_event(frame);
        if event.kind != StreamEventKind::AccountState || event.accounts.len() != 1 {
            self.fail_count += 1;
            return false;
        }

        let ping = stream_ping_message(42);
        let sub = stream_subscribe_message("0:12345", 43);
        if sub.is_empty() {
            self.fail_count += 1;
            return false;
        }
        let ping_val: serde_json::Value = match serde_json::from_slice(&ping) {
            Ok(v) => v,
            Err(_) => {
                self.fail_count += 1;
                return false;
            }
        };
        if ping_val.get("type").and_then(|v| v.as_str()) != Some("ping")
            || ping_val.get("id").and_then(|v| v.as_u64()) != Some(42)
        {
            self.fail_count += 1;
            return false;
        }

        self.pass_count += 1;
        true
    }

    pub fn run_all(&mut self) -> bool {
        let t1 = self.test_parse_endpoint();
        let t2 = self.test_event_parsing();
        t1 && t2
    }
}
