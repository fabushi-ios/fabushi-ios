//! Telegram Desktop 1:1 Parity Submodule: `WalletCardLook`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_look.cpp`, `Telegram/SourceFiles/wallet/wallet_card_look.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const PATTERN_TOP: f64 = 0.125;
pub const PATTERN_HEIGHT: f64 = 0.79;
pub const PATTERN_INK_ALPHA: u8 = 153;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEmojiPattern {
    pub document_id: u64,
    pub source_slug: String,
    pub size_tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardLook {
    pub source_slug: String,
    pub center_color_rgb: u32,
    pub pattern: Option<CustomEmojiPattern>,
    pub gradient_id: u32,
    pub stickers_count: usize,
    pub pattern_alpha: u8,
}

impl Default for WalletCardLook {
    fn default() -> Self {
        Self::new("ton_classic", 0x0773fe)
    }
}

impl WalletCardLook {
    pub fn new(source_slug: &str, center_color_rgb: u32) -> Self {
        Self {
            source_slug: source_slug.to_string(),
            center_color_rgb,
            pattern: None,
            gradient_id: 1,
            stickers_count: 0,
            pattern_alpha: PATTERN_INK_ALPHA,
        }
    }

    pub fn set_pattern(&mut self, document_id: u64, source_slug: &str) {
        self.pattern = Some(CustomEmojiPattern {
            document_id,
            source_slug: source_slug.to_string(),
            size_tag: "Large".to_string(),
        });
    }

    pub fn clear_pattern(&mut self) {
        self.pattern = None;
    }

    pub fn pattern_vertical_bounds(&self, card_height: f64) -> (f64, f64) {
        let top = card_height * PATTERN_TOP;
        let height = card_height * PATTERN_HEIGHT;
        (top, top + height)
    }

    pub fn set_stickers_count(&mut self, count: usize) {
        self.stickers_count = count;
    }
}
