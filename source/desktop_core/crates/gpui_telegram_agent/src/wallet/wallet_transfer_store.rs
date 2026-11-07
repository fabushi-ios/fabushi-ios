//! Local Storage & Journal for Submitted Wallet Transfers
//! Upstream: `Telegram/SourceFiles/wallet/wallet_transfer_store.cpp`, `wallet_transfer_store.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const TRANSFERS_STORAGE_KEY: &str = "presentation/submitted-transfers";
pub const TRANSFERS_FORMAT_VERSION: u32 = 4;
pub const IDENTITY_MAX_BYTES: usize = 256;
pub const ADDRESS_MAX_BYTES: usize = 128;
pub const SEND_COMMENT_MAX_BYTES: usize = 960;
pub const SEND_COMMENT_WARN_BYTES: usize = 50;
pub const SEND_COMMENT_MAX_LENGTH: usize = 1920;
pub const SUBMITTED_TRANSFER_MAX_RECORDS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmittedTransferRecord {
    pub transfer_id: String,
    pub recipient_address: String,
    pub amount_nanos: u64,
    pub comment: String,
    pub submitted_at_ms: u64,
    pub is_confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletTransferStore {
    pub records: Vec<SubmittedTransferRecord>,
    pub version: u32,
}

impl Default for WalletTransferStore {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletTransferStore {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            version: TRANSFERS_FORMAT_VERSION,
        }
    }

    pub fn add_transfer(&mut self, record: SubmittedTransferRecord) {
        if self.records.len() >= SUBMITTED_TRANSFER_MAX_RECORDS {
            // Drop oldest confirmed or first record
            if let Some(idx) = self.records.iter().position(|r| r.is_confirmed) {
                self.records.remove(idx);
            } else {
                self.records.remove(0);
            }
        }
        self.records.push(record);
    }

    pub fn mark_confirmed(&mut self, transfer_id: &str) -> bool {
        if let Some(r) = self.records.iter_mut().find(|r| r.transfer_id == transfer_id) {
            r.is_confirmed = true;
            true
        } else {
            false
        }
    }

    pub fn pending_transfers(&self) -> Vec<&SubmittedTransferRecord> {
        self.records.iter().filter(|r| !r.is_confirmed).collect()
    }

    pub fn validate_comment(comment: &str) -> (bool, bool) {
        let bytes_len = comment.as_bytes().len();
        let is_valid = bytes_len <= SEND_COMMENT_MAX_BYTES;
        let is_near_limit = bytes_len > (SEND_COMMENT_MAX_BYTES.saturating_sub(SEND_COMMENT_WARN_BYTES));
        (is_valid, is_near_limit)
    }
}
