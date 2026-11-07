//! Authentic TON Wallet Collectibles Hint Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_collectibles_hint.cpp, Telegram/SourceFiles/wallet/wallet_collectibles_hint.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCollectiblesHint {
    pub is_shown: bool,
    pub is_dismissed: bool,
    pub target_bounds: Option<(i32, i32, i32, i32)>, // x, y, width, height
    pub hint_text: String,
}

impl WalletCollectiblesHint {
    pub fn new() -> Self {
        Self {
            is_shown: false,
            is_dismissed: false,
            target_bounds: None,
            hint_text: "Tap here to explore your TON collectibles".into(),
        }
    }

    pub fn set_target(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.target_bounds = Some((x, y, w, h));
    }

    pub fn show(&mut self) -> bool {
        if self.is_dismissed {
            return false;
        }
        self.is_shown = true;
        true
    }

    pub fn dismiss(&mut self) {
        self.is_shown = false;
        self.is_dismissed = true;
    }

    pub fn refresh(&mut self) {
        if self.is_dismissed {
            self.is_shown = false;
        }
    }
}

impl Default for WalletCollectiblesHint {
    fn default() -> Self {
        Self::new()
    }
}
