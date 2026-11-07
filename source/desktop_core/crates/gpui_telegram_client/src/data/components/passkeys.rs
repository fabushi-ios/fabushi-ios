//! Target Symbol: PasskeysComponents
//! Telegram Desktop 1:1 Submodule: `PasskeysComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PasskeyEntry {
    pub id: Vec<u8>,
    pub name: String,
    pub created_at: i64,
}

/// Target Symbol for Module 03 Ledger: PasskeysComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PasskeysComponents {
    pub passkeys: Vec<PasskeyEntry>,
}

impl PasskeysComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: Vec<u8>, name: impl Into<String>) {
        self.passkeys.push(PasskeyEntry {
            id,
            name: name.into(),
            created_at: chrono::Utc::now().timestamp(),
        });
    }

    pub fn count(&self) -> usize {
        self.passkeys.len()
    }
}
