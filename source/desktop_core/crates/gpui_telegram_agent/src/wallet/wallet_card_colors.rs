//! Telegram Desktop 1:1 Parity Submodule: `WalletCardColors`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_colors.cpp`, `Telegram/SourceFiles/wallet/wallet_card_colors.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use gpui_telegram_core::error::Result;

pub const RESOLVE_WAIT_MS: u64 = 5000;
pub const TOAST_DURATION_MS: u64 = 8000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorPatternMeta {
    pub collection_id: String,
    pub primary_rgb: u32,
    pub secondary_rgb: u32,
    pub pattern_doc_id: u64,
}

pub fn default_color_collections() -> Vec<String> {
    vec![
        "ton_classic".to_string(),
        "stars_golden".to_string(),
        "gifts_aurora".to_string(),
        "emerald_crypto".to_string(),
        "midnight_dark".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardColors {
    pub collections: Vec<String>,
    pub patterns: HashMap<String, ColorPatternMeta>,
    pub active_collection: String,
    pub active_toast: Option<String>,
}

impl Default for WalletCardColors {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardColors {
    pub fn new() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert(
            "ton_classic".to_string(),
            ColorPatternMeta {
                collection_id: "ton_classic".to_string(),
                primary_rgb: 0x0773fe,
                secondary_rgb: 0x0088cc,
                pattern_doc_id: 1001,
            },
        );
        patterns.insert(
            "stars_golden".to_string(),
            ColorPatternMeta {
                collection_id: "stars_golden".to_string(),
                primary_rgb: 0xffa500,
                secondary_rgb: 0xffd700,
                pattern_doc_id: 1002,
            },
        );
        patterns.insert(
            "gifts_aurora".to_string(),
            ColorPatternMeta {
                collection_id: "gifts_aurora".to_string(),
                primary_rgb: 0x8a2be2,
                secondary_rgb: 0xda70d6,
                pattern_doc_id: 1003,
            },
        );

        Self {
            collections: default_color_collections(),
            patterns,
            active_collection: "ton_classic".to_string(),
            active_toast: None,
        }
    }

    pub fn resolve_collection(&self, slug: &str) -> Option<&ColorPatternMeta> {
        self.patterns.get(slug)
    }

    pub fn select_collection(&mut self, slug: &str) -> bool {
        if self.patterns.contains_key(slug) {
            self.active_collection = slug.to_string();
            self.active_toast = Some(format!("Card color set to: {}", slug));
            true
        } else {
            false
        }
    }

    pub fn card_color_toast(&self) -> Option<&str> {
        self.active_toast.as_deref()
    }

    pub fn clear_toast(&mut self) {
        self.active_toast = None;
    }

    pub fn register_pattern(&mut self, meta: ColorPatternMeta) {
        if !self.collections.contains(&meta.collection_id) {
            self.collections.push(meta.collection_id.clone());
        }
        self.patterns.insert(meta.collection_id.clone(), meta);
    }
}
