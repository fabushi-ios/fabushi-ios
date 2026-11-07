//! Authentic TON Keychain Service Implementation
//! Part of Module 13 Wallet Part 4 Client Architecture

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum KeychainError {
    #[error("Secret not found for key: {0}")]
    NotFound(String),
    #[error("Keychain access denied")]
    AccessDenied,
    #[error("Internal storage error: {0}")]
    StorageError(String),
}

#[derive(Debug, Clone)]
pub struct TonKeychainService {
    service_name: String,
    vault: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl TonKeychainService {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            vault: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn store_secret(&self, key: &str, secret: &[u8]) -> Result<(), KeychainError> {
        let mut guard = self.vault.write();
        guard.insert(format!("{}:{}", self.service_name, key), secret.to_vec());
        Ok(())
    }

    pub fn retrieve_secret(&self, key: &str) -> Result<Vec<u8>, KeychainError> {
        let guard = self.vault.read();
        guard
            .get(&format!("{}:{}", self.service_name, key))
            .cloned()
            .ok_or_else(|| KeychainError::NotFound(key.to_string()))
    }

    pub fn delete_secret(&self, key: &str) -> Result<(), KeychainError> {
        let mut guard = self.vault.write();
        guard.remove(&format!("{}:{}", self.service_name, key));
        Ok(())
    }

    pub fn has_secret(&self, key: &str) -> bool {
        let guard = self.vault.read();
        guard.contains_key(&format!("{}:{}", self.service_name, key))
    }
}

impl Default for TonKeychainService {
    fn default() -> Self {
        Self::new("gpui_telegram_wallet")
    }
}
