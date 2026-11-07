//! Authentic TON Wallet Key Custody Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_custody.cpp, Telegram/SourceFiles/wallet/wallet_custody.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_CUSTODY_PUBLIC_KEY_SIZE: usize = 32;
pub const K_CUSTODY_FORMAT_VERSION: u32 = 4;
pub const K_ACTIVE_FLAG: u32 = 1 << 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyRecord {
    pub record_id: String,
    pub address: String,
    pub public_key: Vec<u8>,
    pub signing_key: Vec<u8>,
    pub network_id: i32,
    pub secret_ref: String,
    pub is_active: bool,
    pub rotated_since_backup: bool,
    pub awaiting_server_key: bool,
}

impl CustodyRecord {
    pub fn signs_with(&self, served_key: &[u8]) -> bool {
        if self.signing_key.is_empty() {
            self.public_key == served_key
        } else {
            self.signing_key == served_key
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCustody {
    pub records: Vec<CustodyRecord>,
    pub pending_rotations: Vec<String>,
    pub storage_key: String,
    pub format_version: u32,
}

impl WalletCustody {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            pending_rotations: Vec::new(),
            storage_key: "custody/records".into(),
            format_version: K_CUSTODY_FORMAT_VERSION,
        }
    }

    pub fn add_record(&mut self, record: CustodyRecord) {
        if let Some(pos) = self.records.iter().position(|r| r.record_id == record.record_id) {
            self.records[pos] = record;
        } else {
            self.records.push(record);
        }
    }

    pub fn active_record(&self) -> Option<&CustodyRecord> {
        self.records.iter().find(|r| r.is_active)
    }

    pub fn schedule_rotation(&mut self, record_id: impl Into<String>) {
        let id = record_id.into();
        if !self.pending_rotations.contains(&id) {
            self.pending_rotations.push(id);
        }
    }
}

impl Default for WalletCustody {
    fn default() -> Self {
        Self::new()
    }
}
