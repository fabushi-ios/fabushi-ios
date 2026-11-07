//! TON Connect 2.0 Deep Link & Universal URL Parser
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_link.cpp`, `wallet_ton_connect_link.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::{Result, TelegramError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedTonConnectLink {
    pub protocol_version: u32,
    pub client_session_id: String,
    pub manifest_url: String,
    pub return_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectLink {
    pub last_parsed: Option<ParsedTonConnectLink>,
}

impl Default for WalletTonConnectLink {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectLink {
    pub fn new() -> Self {
        Self { last_parsed: None }
    }

    pub fn is_valid_scheme(url: &str) -> bool {
        url.starts_with("tc://") || url.starts_with("https://app.tonkeeper.com/ton-connect") || url.starts_with("ton://connect")
    }

    pub fn parse(&mut self, url: &str) -> Result<ParsedTonConnectLink> {
        if !Self::is_valid_scheme(url) {
            return Err(TelegramError::Internal("Invalid TON Connect link scheme".into()));
        }

        let query_str = if let Some(idx) = url.find('?') {
            &url[idx + 1..]
        } else {
            return Err(TelegramError::Internal("Missing query parameters in TON Connect link".into()));
        };

        let mut version = 2u32;
        let mut client_session_id = String::new();
        let mut manifest_url = String::new();
        let mut return_url = None;

        for pair in query_str.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let val = parts.next().unwrap_or("");
            match key {
                "v" => {
                    if let Ok(v) = val.parse::<u32>() {
                        version = v;
                    }
                }
                "id" => client_session_id = val.to_string(),
                "r" => manifest_url = val.to_string(),
                "ret" => return_url = Some(val.to_string()),
                _ => {}
            }
        }

        if client_session_id.is_empty() {
            return Err(TelegramError::Internal("Missing 'id' parameter in TON Connect link".into()));
        }

        let parsed = ParsedTonConnectLink {
            protocol_version: version,
            client_session_id,
            manifest_url,
            return_url,
        };

        self.last_parsed = Some(parsed.clone());
        Ok(parsed)
    }

    pub fn build_return_url(&self, success: bool) -> Option<String> {
        self.last_parsed.as_ref()?.return_url.as_ref().map(|ret| {
            let status = if success { "success" } else { "canceled" };
            format!("{}?status={}", ret, status)
        })
    }
}
