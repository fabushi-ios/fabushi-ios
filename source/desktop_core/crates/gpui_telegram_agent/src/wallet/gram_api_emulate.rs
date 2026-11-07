//! Gram API transaction trace emulation and fee extraction.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_emulate.cpp, gram_api_emulate.h

#![allow(dead_code)]

use super::gram_api_request::HttpRequest;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatedTrace {
    pub fee_nano: i64,
    pub account: String,
}

pub fn emulate_trace_request(boc_base64: &str) -> HttpRequest {
    let payload_val = serde_json::json!({
        "boc": boc_base64,
        "ignore_chksig": true,
        "include_code_data": false,
        "include_address_book": false,
        "include_metadata": false,
        "with_actions": true,
        "mc_block_seqno": serde_json::Value::Null,
    });
    HttpRequest {
        post: true,
        endpoint: "/api/emulate/v1/emulateTrace".to_string(),
        query: String::new(),
        payload: serde_json::to_vec(&payload_val).unwrap_or_default(),
    }
}

fn phase_succeeded(
    phases: &serde_json::Map<String, serde_json::Value>,
    phase_key: &str,
    code_key: &str,
    code_allowed: impl Fn(i64) -> bool,
) -> bool {
    let phase = match phases.get(phase_key).and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return false,
    };
    let success = phase.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    if !success {
        return false;
    }
    let code = match phase.get(code_key).and_then(|v| v.as_i64()) {
        Some(c) => c,
        None => return false,
    };
    code_allowed(code)
}

pub fn parse_emulated_trace(json_bytes: &[u8]) -> Option<EmulatedTrace> {
    let document: serde_json::Value = serde_json::from_slice(json_bytes).ok()?;
    let object = document.as_object()?;
    let trace = object.get("trace")?.as_object()?;
    let tx_hash = trace.get("tx_hash")?.as_str()?;
    if tx_hash.is_empty() {
        return None;
    }
    let transactions = object.get("transactions")?.as_object()?;
    let root = transactions.get(tx_hash)?.as_object()?;

    let account = root.get("account")?.as_str()?;
    if account.is_empty() {
        return None;
    }
    let fees_str = root.get("total_fees")?.as_str()?;
    let fee_nano = fees_str.parse::<i64>().ok()?;
    if fee_nano < 0 {
        return None;
    }

    let description = root.get("description")?.as_object()?;
    let aborted = description.get("aborted").and_then(|v| v.as_bool()).unwrap_or(false);
    if aborted {
        return None;
    }

    let compute_ok = phase_succeeded(description, "compute_ph", "exit_code", |code| {
        code == 0 || code == 1
    });
    let action_ok = phase_succeeded(description, "action", "result_code", |code| code == 0);

    if !compute_ok || !action_ok {
        return None;
    }

    Some(EmulatedTrace {
        fee_nano,
        account: account.to_string(),
    })
}

/// Upstream target symbol: GramApiEmulateApi
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramApiEmulateApi {
    pub last_trace: Option<EmulatedTrace>,
}

impl GramApiEmulateApi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build_request(&self, boc_base64: &str) -> HttpRequest {
        emulate_trace_request(boc_base64)
    }

    pub fn emulate(&mut self, json_response: &[u8]) -> Option<EmulatedTrace> {
        let trace = parse_emulated_trace(json_response);
        if trace.is_some() {
            self.last_trace = trace.clone();
        }
        trace
    }

    pub fn estimated_fee_nano(&self) -> i64 {
        self.last_trace.as_ref().map(|t| t.fee_nano).unwrap_or(0)
    }
}
