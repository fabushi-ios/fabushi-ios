//! Target Symbol: DataAuthorization
//! Telegram Desktop 1:1 Submodule: `DataAuthorization`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Unreviewed session login attempt requiring user confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UnreviewedAuth {
    pub hash: u64,
    pub unconfirmed: bool,
    pub date: i64,
    pub device: String,
    pub location: String,
}

impl UnreviewedAuth {
    pub fn new(hash: u64, device: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            hash,
            unconfirmed: true,
            date: chrono::Utc::now().timestamp(),
            device: device.into(),
            location: location.into(),
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataAuthorization coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataAuthorization {
    pub unreviewed: Vec<UnreviewedAuth>,
}

impl DataAuthorization {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_unreviewed(&mut self, auth: UnreviewedAuth) {
        if !self.unreviewed.iter().any(|a| a.hash == auth.hash) {
            self.unreviewed.push(auth);
        }
    }

    pub fn confirm(&mut self, hash: u64) -> bool {
        if let Some(pos) = self.unreviewed.iter().position(|a| a.hash == hash) {
            self.unreviewed.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn has_unreviewed(&self) -> bool {
        !self.unreviewed.is_empty()
    }
}
