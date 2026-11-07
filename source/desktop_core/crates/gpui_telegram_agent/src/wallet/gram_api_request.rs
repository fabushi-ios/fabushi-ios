//! Gram API generic HTTP request contracts and URL encoding primitives.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_request.cpp, gram_api_request.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub post: bool,
    pub endpoint: String,
    pub query: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
}

pub fn parse_api_error(json_bytes: &[u8]) -> Option<ApiError> {
    let value: serde_json::Value = serde_json::from_slice(json_bytes).ok()?;
    let obj = value.as_object()?;
    let err_val = obj.get("error")?.as_str()?;
    Some(ApiError {
        code: 0,
        message: err_val.to_string(),
    })
}

pub mod api_details {
    pub fn percent_encoded(value: &str) -> String {
        let mut result = String::with_capacity(value.len() * 3);
        for byte in value.as_bytes() {
            match *byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(*byte as char);
                }
                b => {
                    result.push_str(&format!("%{:02X}", b));
                }
            }
        }
        result
    }
}

/// Upstream target symbol: GramApiRequestApi
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GramApiRequestApi {
    pub base_url: String,
    pub api_key: Option<String>,
}

impl GramApiRequestApi {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn make_get(&self, endpoint: &str, query_params: &[(&str, &str)]) -> HttpRequest {
        let query = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, api_details::percent_encoded(v)))
            .collect::<Vec<_>>()
            .join("&");
        HttpRequest {
            post: false,
            endpoint: endpoint.to_string(),
            query,
            payload: Vec::new(),
        }
    }

    pub fn make_post(&self, endpoint: &str, json_payload: Vec<u8>) -> HttpRequest {
        HttpRequest {
            post: true,
            endpoint: endpoint.to_string(),
            query: String::new(),
            payload: json_payload,
        }
    }

    pub fn parse_error(&self, payload: &[u8]) -> Option<ApiError> {
        parse_api_error(payload)
    }
}
