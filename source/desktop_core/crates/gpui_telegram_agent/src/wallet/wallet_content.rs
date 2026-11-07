//! Authentic TON Wallet Content Coordinator Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_content.cpp, Telegram/SourceFiles/wallet/wallet_content.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_ADDRESS_LENGTH: usize = 48;
pub const K_ADDRESS_GROUP: usize = 4;
pub const K_ADDRESS_GROUPS_PER_LINE: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletTab {
    Assets,
    Activity,
    Collectibles,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletContent {
    pub current_tab: WalletTab,
    pub address: String,
    pub is_panel_open: bool,
    pub has_collectibles: bool,
    pub pending_transfer_count: usize,
}

impl WalletContent {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            current_tab: WalletTab::Assets,
            address: address.into(),
            is_panel_open: false,
            has_collectibles: false,
            pending_transfer_count: 0,
        }
    }

    pub fn set_tab(&mut self, tab: WalletTab) {
        self.current_tab = tab;
    }

    /// Formats 48-char address with space groups of 4 (kAddressGroup)
    pub fn format_grouped_address(&self) -> String {
        let clean: String = self.address.chars().filter(|c| !c.is_whitespace()).collect();
        let chunks: Vec<&str> = clean
            .as_bytes()
            .chunks(K_ADDRESS_GROUP)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
            .collect();
        chunks.join(" ")
    }

    pub fn open_panel(&mut self) {
        self.is_panel_open = true;
    }

    pub fn close_panel(&mut self) {
        self.is_panel_open = false;
    }
}

impl Default for WalletContent {
    fn default() -> Self {
        Self::new("")
    }
}
