//! Wallet Passcode Gate & Biometric Unlock Pipeline
//! Upstream: `Telegram/SourceFiles/wallet/wallet_unlock.cpp`, `wallet_unlock.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const RETAINED_TOAST_DURATION_MS: u64 = 4000;
pub const PROBLEM_TOAST_DURATION_MS: u64 = 8000;
pub const MAX_FAILED_ATTEMPTS: u32 = 5;
pub const LOCKOUT_DURATION_MS: u64 = 30000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockMethod {
    Passcode,
    Biometrics,
    HardwareKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletUnlock {
    pub is_locked: bool,
    pub failed_attempts: u32,
    pub locked_until_ms: u64,
    pub biometric_available: bool,
}

impl Default for WalletUnlock {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletUnlock {
    pub fn new() -> Self {
        Self {
            is_locked: true,
            failed_attempts: 0,
            locked_until_ms: 0,
            biometric_available: false,
        }
    }

    pub fn is_lockout_active(&self, now_ms: u64) -> bool {
        now_ms < self.locked_until_ms
    }

    pub fn verify_passcode(&mut self, entered: &str, expected_hash: &str, now_ms: u64) -> bool {
        if self.is_lockout_active(now_ms) {
            return false;
        }

        // Constant-time length and equality check simulation
        let matches = entered == expected_hash;
        if matches {
            self.is_locked = false;
            self.failed_attempts = 0;
            self.locked_until_ms = 0;
            true
        } else {
            self.failed_attempts = self.failed_attempts.saturating_add(1);
            if self.failed_attempts >= MAX_FAILED_ATTEMPTS {
                self.locked_until_ms = now_ms.saturating_add(LOCKOUT_DURATION_MS);
            }
            false
        }
    }

    pub fn unlock_with_biometrics(&mut self) -> bool {
        if self.biometric_available {
            self.is_locked = false;
            self.failed_attempts = 0;
            true
        } else {
            false
        }
    }

    pub fn lock(&mut self) {
        self.is_locked = true;
    }
}
