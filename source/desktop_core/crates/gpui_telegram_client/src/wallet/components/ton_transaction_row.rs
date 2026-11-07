//! TON Transaction Row component displaying transfer history and transaction status.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonTransactionRow {
    pub tx_hash: String,
    pub direction: TransactionDirection,
    pub peer_address: String,
    pub amount_nano: i64,
    pub fee_nano: i64,
    pub comment: Option<String>,
    pub timestamp: i64,
    pub is_confirmed: bool,
}

impl TonTransactionRow {
    pub fn new(
        tx_hash: impl Into<String>,
        direction: TransactionDirection,
        peer_address: impl Into<String>,
        amount_nano: i64,
    ) -> Self {
        Self {
            tx_hash: tx_hash.into(),
            direction,
            peer_address: peer_address.into(),
            amount_nano,
            fee_nano: 0,
            comment: None,
            timestamp: chrono::Utc::now().timestamp(),
            is_confirmed: true,
        }
    }

    pub fn formatted_amount(&self) -> String {
        let ton = (self.amount_nano as f64) / 1_000_000_000.0;
        match self.direction {
            TransactionDirection::Inbound => format!("+{:.4} TON", ton),
            TransactionDirection::Outbound => format!("-{:.4} TON", ton),
        }
    }
}
