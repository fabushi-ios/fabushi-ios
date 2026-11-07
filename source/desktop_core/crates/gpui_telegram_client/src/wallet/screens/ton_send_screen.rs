//! Authentic TON Send Screen Confirmation Coordinator ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SendStatus {
    Review,
    Signing,
    Broadcasting,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonSendScreen {
    pub recipient_address: String,
    pub amount_nanotons: u64,
    pub token_symbol: String,
    pub comment: Option<String>,
    pub encrypt_comment: bool,
    pub estimated_fee_nanotons: u64,
    pub status: SendStatus,
    pub transaction_hash: Option<String>,
    pub error_message: Option<String>,
}

impl TonSendScreen {
    pub fn new(recipient: impl Into<String>, amount_nanos: u64) -> Self {
        Self {
            recipient_address: recipient.into(),
            amount_nanotons: amount_nanos,
            token_symbol: "TON".into(),
            comment: None,
            encrypt_comment: false,
            estimated_fee_nanotons: 8_000_000, // 0.008 TON typical transfer fee
            status: SendStatus::Review,
            transaction_hash: None,
            error_message: None,
        }
    }

    pub fn total_nanotons(&self) -> u64 {
        self.amount_nanotons + self.estimated_fee_nanotons
    }

    pub fn set_comment(&mut self, text: impl Into<String>, encrypt: bool) {
        self.comment = Some(text.into());
        self.encrypt_comment = encrypt;
    }

    pub fn confirm_send(&mut self) {
        self.status = SendStatus::Signing;
    }

    pub fn mark_broadcasting(&mut self) {
        self.status = SendStatus::Broadcasting;
    }

    pub fn on_success(&mut self, tx_hash: impl Into<String>) {
        self.status = SendStatus::Success;
        self.transaction_hash = Some(tx_hash.into());
    }

    pub fn on_failure(&mut self, err: impl Into<String>) {
        self.status = SendStatus::Failed;
        self.error_message = Some(err.into());
    }
}

impl Default for TonSendScreen {
    fn default() -> Self {
        Self::new("", 0)
    }
}
