//! TON Wallet App Bar component hosting navigation, network badge, and settings actions.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonWalletAppBar {
    pub title: String,
    pub is_testnet: bool,
    pub can_go_back: bool,
    pub show_settings: bool,
}

impl TonWalletAppBar {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            is_testnet: false,
            can_go_back: true,
            show_settings: true,
        }
    }

    pub fn set_testnet(&mut self, testnet: bool) {
        self.is_testnet = testnet;
    }

    pub fn network_label(&self) -> &'static str {
        if self.is_testnet {
            "Testnet"
        } else {
            "Mainnet"
        }
    }
}
