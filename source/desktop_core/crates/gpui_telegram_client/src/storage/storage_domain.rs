//! Target Symbol: StorageDomain
//! Telegram Desktop 1:1 Submodule: `StorageDomain`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::HashMap;
use std::path::PathBuf;
use super::details::storage_file_utilities::{create_passcode_key, PasscodeKdf};
use super::storage_account::StorageAccount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartResult {
    Success,
    IncorrectPasscode,
    IncorrectPasscodeLegacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetPasscodeResult {
    Success,
    NeedsVerification,
    Failed,
}

pub struct PasscodeDerivation {
    passcode: Vec<u8>,
    salt: Vec<u8>,
    kdf: PasscodeKdf,
    derived_key: Option<[u8; 32]>,
}

impl PasscodeDerivation {
    pub fn new(passcode: Vec<u8>, salt: Vec<u8>, kdf: PasscodeKdf) -> Self {
        Self {
            passcode,
            salt,
            kdf,
            derived_key: None,
        }
    }

    pub fn run(&mut self) -> Result<(), String> {
        let key = create_passcode_key(&self.passcode, &self.salt, &self.kdf)?;
        self.derived_key = Some(key);
        // Cleanse plaintext passcode memory
        for b in &mut self.passcode {
            *b = 0;
        }
        self.passcode.clear();
        Ok(())
    }

    pub fn key(&self) -> Option<&[u8; 32]> {
        self.derived_key.as_ref()
    }
}

pub struct PasscodeVerification {
    nonce: u64,
}

impl Default for PasscodeVerification {
    fn default() -> Self {
        Self::new()
    }
}

impl PasscodeVerification {
    pub fn new() -> Self {
        let mut bytes = [0u8; 8];
        let _ = getrandom::getrandom(&mut bytes);
        Self {
            nonce: u64::from_le_bytes(bytes),
        }
    }

    pub fn nonce(&self) -> u64 {
        self.nonce
    }
}

pub struct StorageDomain {
    pub base_path: PathBuf,
    current_nonce: u64,
    has_passcode: bool,
    active_local_key: Option<[u8; 32]>,
    wallet_keyring: Option<Vec<u8>>,
    accounts: HashMap<String, StorageAccount>,
}

impl StorageDomain {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            current_nonce: 0,
            has_passcode: false,
            active_local_key: None,
            wallet_keyring: None,
            accounts: HashMap::new(),
        }
    }

    pub fn start(&mut self, mut derivation: PasscodeDerivation) -> StartResult {
        if derivation.run().is_err() {
            return StartResult::IncorrectPasscode;
        }
        if let Some(key) = derivation.key() {
            self.active_local_key = Some(*key);
            StartResult::Success
        } else {
            StartResult::IncorrectPasscode
        }
    }

    pub fn verify_passcode(&mut self, passcode: &[u8], salt: &[u8], kdf: &PasscodeKdf) -> Option<PasscodeVerification> {
        let mut d = PasscodeDerivation::new(passcode.to_vec(), salt.to_vec(), *kdf);
        if d.run().is_ok() && d.key() == self.active_local_key.as_ref() {
            let v = PasscodeVerification::new();
            self.current_nonce = v.nonce();
            Some(v)
        } else {
            None
        }
    }

    pub fn set_passcode(
        &mut self,
        verification: Option<&PasscodeVerification>,
        new_passcode: &[u8],
        salt: &[u8],
        kdf: &PasscodeKdf,
    ) -> SetPasscodeResult {
        if self.has_passcode {
            if let Some(v) = verification {
                if v.nonce() != self.current_nonce || self.current_nonce == 0 {
                    return SetPasscodeResult::NeedsVerification;
                }
            } else {
                return SetPasscodeResult::NeedsVerification;
            }
        }

        // Reset verification nonce on consumption
        self.current_nonce = 0;

        let mut d = PasscodeDerivation::new(new_passcode.to_vec(), salt.to_vec(), *kdf);
        if let Ok(()) = d.run() {
            if let Some(key) = d.key() {
                self.active_local_key = Some(*key);
                self.has_passcode = !new_passcode.is_empty();
                return SetPasscodeResult::Success;
            }
        }
        SetPasscodeResult::Failed
    }

    pub fn write_wallet_keyring(&mut self, bytes: Vec<u8>) -> bool {
        self.wallet_keyring = Some(bytes);
        true
    }

    pub fn read_wallet_keyring(&self) -> Option<&[u8]> {
        self.wallet_keyring.as_deref()
    }

    pub fn remove_wallet_keyring(&mut self) -> bool {
        self.wallet_keyring = None;
        true
    }

    pub fn get_account(&mut self, data_name: &str) -> Result<&mut StorageAccount, String> {
        if !self.accounts.contains_key(data_name) {
            let account = StorageAccount::open(data_name, &self.base_path)?;
            self.accounts.insert(data_name.to_string(), account);
        }
        Ok(self.accounts.get_mut(data_name).unwrap())
    }
}
