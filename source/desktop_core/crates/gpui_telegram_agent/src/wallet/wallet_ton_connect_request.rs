//! TON Connect RPC Request Parsing & Execution Handler
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_request.cpp`, `wallet_ton_connect_request.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::{Result, TelegramError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonConnectMessage {
    pub address: String,
    pub amount_nanos_str: String,
    pub payload_b64: Option<String>,
    pub state_init_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTransactionRpcParams {
    pub valid_until: u64,
    pub from: Option<String>,
    pub messages: Vec<TonConnectMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TonConnectRpcRequest {
    SendTransaction { id: String, params: SendTransactionRpcParams },
    SignData { id: String, schema_b64: String, cell_b64: String },
    Disconnect { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectRequest {
    pub pending_requests: Vec<TonConnectRpcRequest>,
}

impl Default for WalletTonConnectRequest {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectRequest {
    pub fn new() -> Self {
        Self {
            pending_requests: Vec::new(),
        }
    }

    pub fn enqueue_send_transaction(&mut self, id: impl Into<String>, params: SendTransactionRpcParams, current_time: u64) -> Result<()> {
        if params.valid_until != 0 && params.valid_until < current_time {
            return Err(TelegramError::Internal("Transaction request has already expired".into()));
        }
        if params.messages.is_empty() {
            return Err(TelegramError::Internal("Transaction messages list cannot be empty".into()));
        }
        for msg in &params.messages {
            if msg.address.is_empty() {
                return Err(TelegramError::Internal("Destination address cannot be empty".into()));
            }
        }
        self.pending_requests.push(TonConnectRpcRequest::SendTransaction {
            id: id.into(),
            params,
        });
        Ok(())
    }

    pub fn total_pending_amount_nanos(&self) -> u64 {
        let mut total = 0u64;
        for req in &self.pending_requests {
            if let TonConnectRpcRequest::SendTransaction { params, .. } = req {
                for msg in &params.messages {
                    if let Ok(amount) = msg.amount_nanos_str.parse::<u64>() {
                        total = total.saturating_add(amount);
                    }
                }
            }
        }
        total
    }

    pub fn pop_next(&mut self) -> Option<TonConnectRpcRequest> {
        if !self.pending_requests.is_empty() {
            Some(self.pending_requests.remove(0))
        } else {
            None
        }
    }
}
