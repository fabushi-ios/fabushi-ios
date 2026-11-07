//! Telegram Desktop 1:1 Parity Submodule: `WalletChatShow`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_chat_show.cpp`, `Telegram/SourceFiles/wallet/wallet_chat_show.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletChatShow {
    pub session_id: u64,
    pub resolve_windows: bool,
    pub active_box_count: usize,
    pub active_toast: Option<String>,
    pub resolved_urls: Vec<String>,
}

impl Default for WalletChatShow {
    fn default() -> Self {
        Self::new(0, false)
    }
}

impl WalletChatShow {
    pub fn new(session_id: u64, resolve_windows: bool) -> Self {
        Self {
            session_id,
            resolve_windows,
            active_box_count: 0,
            active_toast: None,
            resolved_urls: Vec::new(),
        }
    }

    pub fn show_box(&mut self, _box_identifier: &str) -> bool {
        self.active_box_count += 1;
        true
    }

    pub fn hide_box(&mut self) -> bool {
        if self.active_box_count > 0 {
            self.active_box_count -= 1;
            true
        } else {
            false
        }
    }

    pub fn show_toast(&mut self, text: &str) {
        self.active_toast = Some(text.to_string());
    }

    pub fn clear_toast(&mut self) {
        self.active_toast = None;
    }

    pub fn resolve_window(&mut self, url: &str) -> bool {
        if self.resolve_windows {
            self.resolved_urls.push(url.to_string());
            true
        } else {
            false
        }
    }
}
