//! Hardware Keyring & AES-256-GCM Encrypted Wallet Vault
//! Upstream: `Telegram/SourceFiles/wallet/wallet_vault.cpp`, `wallet_vault.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const DEVICE_KEYRING_MAGIC: u32 = 0x574B5247; // 'WKRG'
pub const DEVICE_KEYRING_FORMAT_VERSION: u32 = 1;
pub const VAULT_RECORD_MAGIC: u32 = 0x574B5243;   // 'WKRC'
pub const VAULT_RECORD_FORMAT_VERSION: u32 = 1;
pub const VAULT_KEY_SIZE_BYTES: usize = 32;
pub const VAULT_KEY_ID_SIZE_BYTES: usize = 16;
pub const VAULT_NONCE_SIZE_BYTES: usize = 12;
pub const VAULT_TAG_SIZE_BYTES: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedVaultRecord {
    pub magic: u32,
    pub version: u32,
    pub key_id: [u8; VAULT_KEY_ID_SIZE_BYTES],
    pub nonce: [u8; VAULT_NONCE_SIZE_BYTES],
    pub tag: [u8; VAULT_TAG_SIZE_BYTES],
    pub ciphertext: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletVault {
    pub records: Vec<EncryptedVaultRecord>,
    pub is_unlocked: bool,
    pub active_key_id: Option<[u8; VAULT_KEY_ID_SIZE_BYTES]>,
}

impl Default for WalletVault {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletVault {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            is_unlocked: false,
            active_key_id: None,
        }
    }

    pub fn store_record(&mut self, key_id: [u8; VAULT_KEY_ID_SIZE_BYTES], nonce: [u8; VAULT_NONCE_SIZE_BYTES], tag: [u8; VAULT_TAG_SIZE_BYTES], ciphertext: Vec<u8>) {
        let record = EncryptedVaultRecord {
            magic: VAULT_RECORD_MAGIC,
            version: VAULT_RECORD_FORMAT_VERSION,
            key_id,
            nonce,
            tag,
            ciphertext,
        };
        self.records.push(record);
    }

    pub fn find_record(&self, key_id: &[u8; VAULT_KEY_ID_SIZE_BYTES]) -> Option<&EncryptedVaultRecord> {
        self.records.iter().find(|r| &r.key_id == key_id)
    }

    pub fn set_unlocked(&mut self, unlocked: bool, key_id: Option<[u8; VAULT_KEY_ID_SIZE_BYTES]>) {
        self.is_unlocked = unlocked;
        self.active_key_id = if unlocked { key_id } else { None };
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}
