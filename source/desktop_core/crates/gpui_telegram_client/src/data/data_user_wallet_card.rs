//! Target Symbol: DataUserWalletCard
//! Telegram Desktop 1:1 Submodule: `DataUserWalletCard`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub const WALLET_CARD_MAX_STICKERS: usize = 10;

/// Sticker placed on a user's Telegram Wallet Card profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WalletCardSticker {
    pub address: String,
    pub gift_id: u64,
}

/// User TON wallet card profile details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WalletCard {
    pub address: String,
    pub balance: i64,
    pub stickers: Vec<WalletCardSticker>,
    pub background_address: String,
    pub stickers_complete: bool,
}

impl WalletCard {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            balance: 0,
            stickers: Vec::new(),
            background_address: String::new(),
            stickers_complete: true,
        }
    }

    pub fn with_balance(mut self, balance: i64) -> Self {
        self.balance = balance;
        self
    }

    pub fn add_sticker(&mut self, sticker: WalletCardSticker) {
        if self.stickers.len() < WALLET_CARD_MAX_STICKERS {
            self.stickers.push(sticker);
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataUserWalletCard coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataUserWalletCard {
    pub card: Option<WalletCard>,
}

impl DataUserWalletCard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_card(card: WalletCard) -> Self {
        Self { card: Some(card) }
    }

    pub fn has_wallet(&self) -> bool {
        self.card.is_some()
    }
}
