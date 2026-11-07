//! Authentic TON Wallet Panel Window Coordinator Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_panel.cpp, Telegram/SourceFiles/wallet/wallet_panel.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelWindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletPanel {
    pub is_open: bool,
    pub is_minimized: bool,
    pub geometry: PanelWindowGeometry,
    pub active_account_id: Option<u64>,
}

impl WalletPanel {
    pub fn new() -> Self {
        Self {
            is_open: false,
            is_minimized: false,
            geometry: PanelWindowGeometry {
                x: 100,
                y: 100,
                width: 390,
                height: 640,
            },
            active_account_id: None,
        }
    }

    pub fn show(&mut self, account_id: u64) {
        self.active_account_id = Some(account_id);
        self.is_open = true;
        self.is_minimized = false;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    pub fn minimize(&mut self) -> bool {
        if self.is_open {
            self.is_minimized = true;
            true
        } else {
            false
        }
    }

    pub fn set_geometry(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.geometry = PanelWindowGeometry {
            x,
            y,
            width: w,
            height: h,
        };
    }
}

impl Default for WalletPanel {
    fn default() -> Self {
        Self::new()
    }
}
