//! Authentic TON Wallet Security & Recovery Settings ViewModel
//! Part of Module 13 Wallet Part 4 Client Architecture

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonWalletSettingsScreen {
    pub wallet_name: String,
    pub address: String,
    pub is_backup_confirmed: bool,
    pub has_passcode: bool,
    pub biometrics_enabled: bool,
    pub auto_lock_timeout_minutes: u32,
    pub is_delete_dialog_open: bool,
}

impl TonWalletSettingsScreen {
    pub fn new(name: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            wallet_name: name.into(),
            address: address.into(),
            is_backup_confirmed: true,
            has_passcode: true,
            biometrics_enabled: true,
            auto_lock_timeout_minutes: 5,
            is_delete_dialog_open: false,
        }
    }

    pub fn toggle_biometrics(&mut self) -> bool {
        self.biometrics_enabled = !self.biometrics_enabled;
        self.biometrics_enabled
    }

    pub fn set_auto_lock(&mut self, minutes: u32) {
        self.auto_lock_timeout_minutes = minutes;
    }

    pub fn prompt_delete(&mut self) {
        self.is_delete_dialog_open = true;
    }

    pub fn cancel_delete(&mut self) {
        self.is_delete_dialog_open = false;
    }
}

impl Default for TonWalletSettingsScreen {
    fn default() -> Self {
        Self::new("Default Wallet", "")
    }
}
