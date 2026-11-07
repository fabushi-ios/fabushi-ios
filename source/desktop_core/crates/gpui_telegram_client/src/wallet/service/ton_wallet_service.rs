//! Authentic TON Wallet Service Facade Implementation
//! Part of Module 13 Wallet Part 4 Client Architecture

use crate::wallet::service::ton_biometrics_service::TonBiometricsService;
use crate::wallet::service::ton_keychain_service::TonKeychainService;
use crate::wallet::service::ton_price_service::TonPriceService;
use crate::wallet::service::ton_secure_storage::{StoredWalletAccount, TonSecureStorage};
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum WalletServiceError {
    #[error("No active wallet account found")]
    NoActiveAccount,
    #[error("Invalid mnemonic words: {0}")]
    InvalidMnemonic(String),
    #[error("Biometric authentication failed")]
    BiometricFailed,
    #[error("Insufficient funds for transfer: required {required}, available {available}")]
    InsufficientFunds { required: u64, available: u64 },
    #[error("Network broadcast error: {0}")]
    BroadcastError(String),
}

#[derive(Debug, Clone)]
pub struct TonWalletService {
    pub biometrics: TonBiometricsService,
    pub keychain: TonKeychainService,
    pub price: TonPriceService,
    pub storage: TonSecureStorage,
    balance_cache: Arc<RwLock<u64>>,
}

impl TonWalletService {
    pub fn new() -> Self {
        Self {
            biometrics: TonBiometricsService::new(),
            keychain: TonKeychainService::new("gpui_telegram_wallet"),
            price: TonPriceService::new(),
            storage: TonSecureStorage::new(),
            balance_cache: Arc::new(RwLock::new(0)),
        }
    }

    pub fn get_active_account(&self) -> Option<StoredWalletAccount> {
        self.storage.active_account()
    }

    pub fn get_cached_balance_nanotons(&self) -> u64 {
        *self.balance_cache.read()
    }

    pub fn set_balance_nanotons(&self, balance: u64) {
        *self.balance_cache.write() = balance;
    }

    pub async fn create_new_wallet(
        &self,
        account_id: &str,
        address: &str,
        label: &str,
        now: u64,
    ) -> Result<StoredWalletAccount, WalletServiceError> {
        let account = StoredWalletAccount {
            account_id: account_id.to_string(),
            address: address.to_string(),
            label: label.to_string(),
            is_active: true,
            created_at_secs: now,
        };
        self.storage.save_account(account.clone());
        self.storage.set_active_account(account_id);
        Ok(account)
    }

    pub async fn send_transfer(
        &self,
        _recipient: &str,
        amount_nanos: u64,
        fee_nanos: u64,
    ) -> Result<String, WalletServiceError> {
        let current_balance = self.get_cached_balance_nanotons();
        let total_needed = amount_nanos + fee_nanos;
        if current_balance < total_needed {
            return Err(WalletServiceError::InsufficientFunds {
                required: total_needed,
                available: current_balance,
            });
        }

        // Deduct balance and return simulated external message hash
        *self.balance_cache.write() = current_balance - total_needed;
        Ok(format!("tx_hash_{}", chrono::Utc::now().timestamp_micros()))
    }
}

impl Default for TonWalletService {
    fn default() -> Self {
        Self::new()
    }
}
