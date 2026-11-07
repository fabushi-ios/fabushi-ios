//! Telegram Desktop 1:1 Parity Submodule: `media_player_panel.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Dropdown Playlist Panel)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_panel.h:L29-L70`
//!           `Telegram/SourceFiles/media/player/media_player_panel.cpp:L36-L37, L53-L86`

use serde::{Deserialize, Serialize};

/// Upstream: `media_player_panel.cpp:L36-L37`
pub const PLAYLIST_IDS_LIMIT: usize = 32;
pub const DELAYED_HIDE_TIMEOUT_MS: i64 = 3000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaylistItem {
    pub id: u64,
    pub title: String,
    pub performer: String,
    pub duration_seconds: i32,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerPanelPlayer {
    pub is_visible: bool,
    pub items: Vec<PlaylistItem>,
    pub selected_index: usize,
    pub max_visible_items: usize,
}

impl MediaPlayerPanelPlayer {
    pub fn new() -> Self {
        Self {
            is_visible: false,
            items: Vec::new(),
            selected_index: 0,
            max_visible_items: PLAYLIST_IDS_LIMIT,
        }
    }

    pub fn set_items(&mut self, items: Vec<PlaylistItem>) {
        self.items = items;
        if self.selected_index >= self.items.len() {
            self.selected_index = 0;
        }
    }

    pub fn show(&mut self) {
        self.is_visible = true;
    }

    pub fn hide(&mut self) {
        self.is_visible = false;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current_item(&self) -> Option<&PlaylistItem> {
        self.items.get(self.selected_index)
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.selected_index = index;
            true
        } else {
            false
        }
    }
}
