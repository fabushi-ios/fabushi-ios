//! Authentic TON Wallet Own Card Customization Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_own_card.cpp, Telegram/SourceFiles/wallet/wallet_own_card.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_PRESERVE_APPEARANCE_DURATION_MS: u64 = 60_000;
pub const K_CARD_DECORATION_HINT_DONE_PREF: &str = "wallet_card_dr";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnCardChange {
    Applied,
    Unchanged,
    NotReady,
    LimitExceeded,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnCardState {
    pub background: String,
    pub stickers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletOwnCard {
    pub current_state: OwnCardState,
    pub is_ready: bool,
    pub last_save_timestamp: u64,
}

impl WalletOwnCard {
    pub fn new() -> Self {
        Self {
            current_state: OwnCardState {
                background: "default_ton_gradient".into(),
                stickers: Vec::new(),
            },
            is_ready: true,
            last_save_timestamp: 0,
        }
    }

    pub fn save(&mut self, desired: OwnCardState, now: u64) -> OwnCardChange {
        if !self.is_ready {
            return OwnCardChange::NotReady;
        }
        if desired.stickers.len() > 5 {
            return OwnCardChange::LimitExceeded;
        }
        if self.current_state == desired {
            return OwnCardChange::Unchanged;
        }
        self.current_state = desired;
        self.last_save_timestamp = now;
        OwnCardChange::Applied
    }

    pub fn shown(&self) -> &OwnCardState {
        &self.current_state
    }
}

impl Default for WalletOwnCard {
    fn default() -> Self {
        Self::new()
    }
}
