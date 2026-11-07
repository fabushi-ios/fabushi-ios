//! Telegram Desktop 1:1 Parity Submodule: `WalletCardView`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_view.cpp`, `Telegram/SourceFiles/wallet/wallet_card_view.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const BUTTON_FILL_LEVEL: f64 = 0.12;
pub const BUTTON_RIPPLE_LEVEL: f64 = 0.25;
pub const BUTTON_RISE: f64 = 4.0;
pub const BUTTON_SCALE: f64 = 1.04;
pub const INK_STEPS: usize = 8;
pub const PREFERRED_CONTRAST: f64 = 4.5;
pub const REQUIRED_CONTRAST: f64 = 3.0;
pub const PLATE_LEVELS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletCardAction {
    Receive,
    Send,
    Earn,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardView {
    pub user_id: u64,
    pub address: String,
    pub balance_nano: i64,
    pub is_presented: bool,
    pub active_action: Option<WalletCardAction>,
    pub button_scale: f64,
}

impl Default for WalletCardView {
    fn default() -> Self {
        Self::new(0, "")
    }
}

impl WalletCardView {
    pub fn new(user_id: u64, address: &str) -> Self {
        Self {
            user_id,
            address: address.to_string(),
            balance_nano: 0,
            is_presented: false,
            active_action: None,
            button_scale: 1.0,
        }
    }

    pub fn show(&mut self) {
        self.is_presented = true;
    }

    pub fn hide(&mut self) {
        self.is_presented = false;
        self.active_action = None;
    }

    pub fn set_balance(&mut self, balance_nano: i64) {
        self.balance_nano = balance_nano;
    }

    pub fn trigger_action(&mut self, action: WalletCardAction) {
        self.active_action = Some(action);
        self.button_scale = BUTTON_SCALE;
    }

    pub fn reset_button_scale(&mut self) {
        self.button_scale = 1.0;
    }
}
