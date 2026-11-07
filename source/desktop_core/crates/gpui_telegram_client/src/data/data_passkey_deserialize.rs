//! Target Symbol: DataPasskeyDeserialize
//! Telegram Desktop 1:1 Submodule: `DataPasskeyDeserialize`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RelyingParty {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PasskeyUser {
    pub id: Vec<u8>,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CredentialParameter {
    pub cred_type: String,
    pub alg: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RegisterData {
    pub rp: RelyingParty,
    pub user: PasskeyUser,
    pub challenge: Vec<u8>,
    pub pub_key_cred_params: Vec<CredentialParameter>,
    pub timeout: i32,
}

/// Target Symbol for Module 03 Ledger: DataPasskeyDeserialize coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPasskeyDeserialize {
    pub last_register: Option<RegisterData>,
}

impl DataPasskeyDeserialize {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn deserialize_register_data(json_str: &str) -> Option<RegisterData> {
        serde_json::from_str(json_str).ok()
    }
}
