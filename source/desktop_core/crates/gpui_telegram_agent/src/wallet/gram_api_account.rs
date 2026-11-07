//! Gram API account information querying and funds parsing.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_account.cpp, gram_api_account.h

#![allow(dead_code)]

use super::gram_api_request::{api_details, HttpRequest};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressFunds {
    pub balance_nano: i64,
    pub never_used: bool,
}

pub fn parse_non_negative(value: &serde_json::Value) -> Option<i64> {
    if let Some(s) = value.as_str() {
        let parsed = s.parse::<i64>().ok()?;
        if parsed >= 0 {
            return Some(parsed);
        }
    } else if let Some(n) = value.as_i64() {
        if n >= 0 {
            return Some(n);
        }
    } else if let Some(f) = value.as_f64() {
        let max_safe = (1i64 << 53) as f64;
        if f >= 0.0 && f <= max_safe && f == f.floor() {
            return Some(f as i64);
        }
    }
    None
}

pub fn address_information_request(address: &str) -> HttpRequest {
    HttpRequest {
        post: false,
        endpoint: "/api/v2/getAddressInformation".to_string(),
        query: format!("address={}", api_details::percent_encoded(address)),
        payload: Vec::new(),
    }
}

pub fn parse_address_funds(json_bytes: &[u8]) -> Option<AddressFunds> {
    let document: serde_json::Value = serde_json::from_slice(json_bytes).ok()?;
    let object = document.as_object()?;
    if !object.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }
    let result = object.get("result")?.as_object()?;
    let balance = parse_non_negative(result.get("balance")?)?;
    let last = result.get("last_transaction_id").and_then(|v| v.as_object());
    let lt = last
        .and_then(|l| l.get("lt"))
        .and_then(parse_non_negative)
        .unwrap_or(0);
    Some(AddressFunds {
        balance_nano: balance,
        never_used: lt == 0,
    })
}

/// Upstream target symbol: GramApiAccountApi
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GramApiAccountApi {
    pub current_address: String,
    pub funds: Option<AddressFunds>,
}

impl GramApiAccountApi {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            current_address: address.into(),
            funds: None,
        }
    }

    pub fn build_request(&self) -> HttpRequest {
        address_information_request(&self.current_address)
    }

    pub fn apply_response(&mut self, json_payload: &[u8]) -> Option<AddressFunds> {
        let parsed = parse_address_funds(json_payload);
        if let Some(ref f) = parsed {
            self.funds = Some(f.clone());
        }
        parsed
    }

    pub fn balance_nano(&self) -> i64 {
        self.funds.as_ref().map(|f| f.balance_nano).unwrap_or(0)
    }

    pub fn is_never_used(&self) -> bool {
        self.funds.as_ref().map(|f| f.never_used).unwrap_or(true)
    }
}
