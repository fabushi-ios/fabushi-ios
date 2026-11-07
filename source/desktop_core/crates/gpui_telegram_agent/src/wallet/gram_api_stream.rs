//! Gram API WebSocket event streaming and endpoint parsing.
//! Upstream: Telegram/SourceFiles/gram/api/gram_api_stream.cpp, gram_api_stream.h

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEndpoint {
    pub host: String,
    pub request_target: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEventKind {
    Unknown,
    AccountState,
    Transactions,
    TraceInvalidated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamEvent {
    pub kind: StreamEventKind,
    pub accounts: Vec<String>,
}

pub const SECURE_PORT: u16 = 443;

pub fn parse_stream_endpoint(url: &str) -> Option<StreamEndpoint> {
    let trimmed = url.trim();
    let rest = if let Some(r) = trimmed.strip_prefix("wss://") {
        r
    } else if let Some(r) = trimmed.strip_prefix("https://") {
        r
    } else {
        return None;
    };

    if rest.contains('@') {
        return None;
    }

    let (host_port, path_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };

    if host_port.is_empty() {
        return None;
    }

    let (host, port) = if let Some((h, p_str)) = host_port.rsplit_once(':') {
        let p = p_str.parse::<u16>().ok()?;
        if p == 0 {
            return None;
        }
        (h.to_string(), p)
    } else {
        (host_port.to_string(), SECURE_PORT)
    };

    Some(StreamEndpoint {
        host,
        request_target: path_query.to_string(),
        port,
    })
}

pub fn stream_endpoint_label(endpoint: &StreamEndpoint) -> String {
    format!("{}:{}", endpoint.host, endpoint.port)
}

pub fn stream_subscribe_message(account: &str, id: u32) -> Vec<u8> {
    let payload = serde_json::json!({
        "type": "subscribe",
        "account": account,
        "id": id,
    });
    serde_json::to_vec(&payload).unwrap_or_default()
}

pub fn stream_ping_message(id: u32) -> Vec<u8> {
    let payload = serde_json::json!({
        "type": "ping",
        "id": id,
    });
    serde_json::to_vec(&payload).unwrap_or_default()
}

pub fn parse_stream_event(frame: &[u8]) -> StreamEvent {
    let val: serde_json::Value = match serde_json::from_slice(frame) {
        Ok(v) => v,
        Err(_) => return StreamEvent { kind: StreamEventKind::Unknown, accounts: Vec::new() },
    };
    let obj = match val.as_object() {
        Some(o) => o,
        None => return StreamEvent { kind: StreamEventKind::Unknown, accounts: Vec::new() },
    };

    let event_type = match obj.get("type").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return StreamEvent { kind: StreamEventKind::Unknown, accounts: Vec::new() },
    };

    match event_type {
        "account_state_change" => {
            let mut accounts = Vec::new();
            if let Some(acc) = obj.get("account").and_then(|v| v.as_str()) {
                if !acc.is_empty() {
                    accounts.push(acc.to_string());
                }
            }
            StreamEvent {
                kind: StreamEventKind::AccountState,
                accounts,
            }
        }
        "transactions" => {
            let mut accounts = Vec::new();
            if let Some(txs) = obj.get("transactions").and_then(|v| v.as_array()) {
                for tx in txs {
                    if let Some(acc) = tx.get("account").and_then(|v| v.as_str()) {
                        if !acc.is_empty() && !accounts.iter().any(|a| a == acc) {
                            accounts.push(acc.to_string());
                        }
                    }
                }
            }
            StreamEvent {
                kind: StreamEventKind::Transactions,
                accounts,
            }
        }
        "trace_invalidated" => StreamEvent {
            kind: StreamEventKind::TraceInvalidated,
            accounts: Vec::new(),
        },
        _ => StreamEvent {
            kind: StreamEventKind::Unknown,
            accounts: Vec::new(),
        },
    }
}

/// Upstream target symbol: GramApiStreamApi
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GramApiStreamApi {
    pub endpoint: Option<StreamEndpoint>,
    pub next_msg_id: u32,
}

impl GramApiStreamApi {
    pub fn new() -> Self {
        Self {
            endpoint: None,
            next_msg_id: 1,
        }
    }

    pub fn set_endpoint_url(&mut self, url: &str) -> bool {
        if let Some(ep) = parse_stream_endpoint(url) {
            self.endpoint = Some(ep);
            true
        } else {
            false
        }
    }

    pub fn next_ping(&mut self) -> Vec<u8> {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        stream_ping_message(id)
    }

    pub fn next_subscribe(&mut self, account: &str) -> Vec<u8> {
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.wrapping_add(1);
        stream_subscribe_message(account, id)
    }

    pub fn process_frame(&self, frame: &[u8]) -> StreamEvent {
        parse_stream_event(frame)
    }
}
