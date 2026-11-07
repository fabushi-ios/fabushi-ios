//! Authentic TON Secure Storage Implementation
//! Part of Module 13 Wallet Part 4 Client Architecture

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredWalletAccount {
    pub account_id: String,
    pub address: String,
    pub label: String,
    pub is_active: bool,
    pub created_at_secs: u64,
}

#[derive(Debug, Clone)]
pub struct TonSecureStorage {
    storage: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    accounts: Arc<RwLock<Vec<StoredWalletAccount>>>,
}

impl TonSecureStorage {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            accounts: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn save_account(&self, account: StoredWalletAccount) {
        let mut guard = self.accounts.write();
        if let Some(pos) = guard.iter().position(|a| a.account_id == account.account_id) {
            guard[pos] = account;
        } else {
            guard.push(account);
        }
    }

    pub fn list_accounts(&self) -> Vec<StoredWalletAccount> {
        self.accounts.read().clone()
    }

    pub fn active_account(&self) -> Option<StoredWalletAccount> {
        self.accounts.read().iter().find(|a| a.is_active).cloned()
    }

    pub fn set_active_account(&self, account_id: &str) -> bool {
        let mut guard = self.accounts.write();
        let mut found = false;
        for acc in guard.iter_mut() {
            if acc.account_id == account_id {
                acc.is_active = true;
                found = true;
            } else {
                acc.is_active = false;
            }
        }
        found
    }

    pub fn store_kv(&self, key: &str, data: &[u8]) {
        self.storage.write().insert(key.to_string(), data.to_vec());
    }

    pub fn get_kv(&self, key: &str) -> Option<Vec<u8>> {
        self.storage.read().get(key).cloned()
    }
}

impl Default for TonSecureStorage {
    fn default() -> Self {
        Self::new()
    }
}
