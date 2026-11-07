//! Telegram Desktop 1:1 Parity Submodule: `WalletApi`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_api.cpp`, `Telegram/SourceFiles/wallet/wallet_api.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use gpui_telegram_core::error::Result;

pub const KILL_SESSION_TIMEOUT_MS: u64 = 10_000;
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;
pub const TIMEOUT_ERROR_MESSAGE: &str = "TIMEOUT";
pub const INVALID_WEBFILE_DC_ERROR: &str = "WEBFILE_DC_INVALID";
pub const STREAMING_URL_ENDPOINT: &str = "getStreamingUrl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToncenterRequest {
    pub endpoint: String,
    pub query: String,
    pub payload: Vec<u8>,
    pub is_post: bool,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
}

impl ApiError {
    pub fn timeout() -> Self {
        Self {
            code: -1,
            message: TIMEOUT_ERROR_MESSAGE.to_string(),
        }
    }

    pub fn invalid_dc() -> Self {
        Self {
            code: -2,
            message: INVALID_WEBFILE_DC_ERROR.to_string(),
        }
    }
}

pub fn is_timeout_error(error: &ApiError) -> bool {
    error.message == TIMEOUT_ERROR_MESSAGE
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletApi {
    pub session_id: u64,
    pub next_request_id: u64,
    pub active_requests: HashMap<u64, ToncenterRequest>,
    pub is_session_alive: bool,
}

impl Default for WalletApi {
    fn default() -> Self {
        Self::new(1)
    }
}

impl WalletApi {
    pub fn new(session_id: u64) -> Self {
        Self {
            session_id,
            next_request_id: 1,
            active_requests: HashMap::new(),
            is_session_alive: true,
        }
    }

    pub fn request(
        &mut self,
        endpoint: &str,
        query: &str,
        payload: Vec<u8>,
        is_post: bool,
    ) -> u64 {
        let req_id = self.next_request_id;
        self.next_request_id += 1;

        let req = ToncenterRequest {
            endpoint: endpoint.to_string(),
            query: query.to_string(),
            payload,
            is_post,
            created_at_ms: chrono::Utc::now().timestamp_millis() as u64,
        };
        self.active_requests.insert(req_id, req);
        req_id
    }

    pub fn cancel_request(&mut self, request_id: u64) -> bool {
        self.active_requests.remove(&request_id).is_some()
    }

    pub fn request_streaming_url(&mut self) -> (u64, String) {
        let req_id = self.request(STREAMING_URL_ENDPOINT, "", Vec::new(), false);
        let endpoint_url = format!("https://t.me/wallet/{}", STREAMING_URL_ENDPOINT);
        (req_id, endpoint_url)
    }

    pub fn check_timeouts(&mut self, now_ms: u64) -> Vec<u64> {
        let mut timed_out = Vec::new();
        for (&id, req) in &self.active_requests {
            if now_ms.saturating_sub(req.created_at_ms) >= REQUEST_TIMEOUT_MS {
                timed_out.push(id);
            }
        }
        for id in &timed_out {
            self.active_requests.remove(id);
        }
        timed_out
    }

    pub fn kill_session(&mut self) {
        self.active_requests.clear();
        self.is_session_alive = false;
    }
}
