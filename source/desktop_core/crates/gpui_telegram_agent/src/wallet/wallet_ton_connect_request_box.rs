//! TON Connect Transaction Confirmation Dialog View Model
//! Upstream: `Telegram/SourceFiles/wallet/wallet_ton_connect_request_box.cpp`, `wallet_ton_connect_request_box.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const PAYLOAD_SHOWN_MAX_BYTES: usize = 1024;
pub const FIELD_DEPTH_SHOWN_MAX: usize = 8;
pub const MINUS_SIGN_CHAR: char = '\u{2212}'; // QChar(0x2212)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TonConnectRequestPhase {
    Review,
    Signing,
    Broadcasting,
    Success,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationDisplay {
    pub recipient: String,
    pub amount_nanos: u64,
    pub payload_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTonConnectRequestBox {
    pub request_id: String,
    pub phase: TonConnectRequestPhase,
    pub destinations: Vec<DestinationDisplay>,
    pub estimated_fee_nanos: u64,
    pub error_text: Option<String>,
}

impl Default for WalletTonConnectRequestBox {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTonConnectRequestBox {
    pub fn new() -> Self {
        Self {
            request_id: String::new(),
            phase: TonConnectRequestPhase::Review,
            destinations: Vec::new(),
            estimated_fee_nanos: 0,
            error_text: None,
        }
    }

    pub fn with_request(request_id: impl Into<String>, destinations: Vec<DestinationDisplay>, estimated_fee_nanos: u64) -> Self {
        Self {
            request_id: request_id.into(),
            phase: TonConnectRequestPhase::Review,
            destinations,
            estimated_fee_nanos,
            error_text: None,
        }
    }

    pub fn format_total_cost(&self) -> String {
        let total_amount: u64 = self.destinations.iter().map(|d| d.amount_nanos).sum();
        let total = total_amount.saturating_add(self.estimated_fee_nanos);
        let tons = total as f64 / 1_000_000_000.0;
        format!("{}{:.4} TON", MINUS_SIGN_CHAR, tons)
    }

    pub fn start_signing(&mut self) {
        self.phase = TonConnectRequestPhase::Signing;
    }

    pub fn mark_success(&mut self) {
        self.phase = TonConnectRequestPhase::Success;
    }

    pub fn mark_rejected(&mut self) {
        self.phase = TonConnectRequestPhase::Rejected;
    }

    pub fn mark_failed(&mut self, err: impl Into<String>) {
        self.phase = TonConnectRequestPhase::Failed;
        self.error_text = Some(err.into());
    }
}
