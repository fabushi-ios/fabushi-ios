//! Telegram Desktop 1:1 Parity Submodule: `WalletCardEditor`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_editor.cpp`, `Telegram/SourceFiles/wallet/wallet_card_editor.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const SNAP_ANGLE_DEG: f64 = 15.0;
pub const SAME_POSE_EPSILON: f64 = 1e-4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardEditorSticker {
    pub id: u64,
    pub document_id: u64,
    pub x: f64,
    pub y: f64,
    pub rotation_deg: f64,
    pub scale: f64,
}

pub fn snap_angle(angle_deg: f64) -> f64 {
    let rem = angle_deg.rem_euclid(360.0);
    let nearest = (rem / SNAP_ANGLE_DEG).round() * SNAP_ANGLE_DEG;
    if (rem - nearest).abs() < 3.5 {
        nearest
    } else {
        angle_deg
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardEditor {
    pub card_id: String,
    pub stickers: Vec<CardEditorSticker>,
    pub selected_index: Option<usize>,
    pub active_color_slug: String,
    pub undo_stack: Vec<Vec<CardEditorSticker>>,
}

impl Default for WalletCardEditor {
    fn default() -> Self {
        Self::new("my_ton_card")
    }
}

impl WalletCardEditor {
    pub fn new(card_id: &str) -> Self {
        Self {
            card_id: card_id.to_string(),
            stickers: Vec::new(),
            selected_index: None,
            active_color_slug: "ton_classic".to_string(),
            undo_stack: Vec::new(),
        }
    }

    pub fn save_state(&mut self) {
        self.undo_stack.push(self.stickers.clone());
        if self.undo_stack.len() > 30 {
            self.undo_stack.remove(0);
        }
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            self.stickers = prev;
            self.selected_index = None;
            true
        } else {
            false
        }
    }

    pub fn add_sticker(&mut self, document_id: u64, x: f64, y: f64) {
        self.save_state();
        let new_id = self.stickers.len() as u64 + 1;
        self.stickers.push(CardEditorSticker {
            id: new_id,
            document_id,
            x,
            y,
            rotation_deg: 0.0,
            scale: 1.0,
        });
        self.selected_index = Some(self.stickers.len() - 1);
    }

    pub fn select_sticker(&mut self, index: usize) -> bool {
        if index < self.stickers.len() {
            self.selected_index = Some(index);
            true
        } else {
            self.selected_index = None;
            false
        }
    }

    pub fn move_selected(&mut self, dx: f64, dy: f64) {
        if let Some(idx) = self.selected_index {
            if let Some(sticker) = self.stickers.get_mut(idx) {
                sticker.x += dx;
                sticker.y += dy;
            }
        }
    }

    pub fn rotate_selected(&mut self, delta_deg: f64, snap: bool) {
        if let Some(idx) = self.selected_index {
            if let Some(sticker) = self.stickers.get_mut(idx) {
                let new_angle = sticker.rotation_deg + delta_deg;
                sticker.rotation_deg = if snap { snap_angle(new_angle) } else { new_angle };
            }
        }
    }

    pub fn remove_selected(&mut self) -> bool {
        if let Some(idx) = self.selected_index {
            self.save_state();
            self.stickers.remove(idx);
            self.selected_index = None;
            true
        } else {
            false
        }
    }
}
