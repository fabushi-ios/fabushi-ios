//! Target Symbol: DataPeerShareTheme
//! Telegram Desktop 1:1 Submodule: `DataPeerShareTheme`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShareThemeDefault;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShareThemeEmojiBackdrop {
    pub backdrop_id: Option<i32>,
    pub emojis: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShareThemeGift {
    pub gift_id: u64,
    pub saved_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareTheme {
    Default(ShareThemeDefault),
    EmojiBackdrop(ShareThemeEmojiBackdrop),
    Gift(ShareThemeGift),
}

impl Default for ShareTheme {
    fn default() -> Self {
        Self::Default(ShareThemeDefault)
    }
}

/// Target Symbol for Module 03 Ledger: DataPeerShareTheme coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerShareTheme {
    pub theme: ShareTheme,
}

impl DataPeerShareTheme {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backdrop(backdrop_id: Option<i32>, emojis: Vec<String>) -> Self {
        Self {
            theme: ShareTheme::EmojiBackdrop(ShareThemeEmojiBackdrop { backdrop_id, emojis }),
        }
    }
}
