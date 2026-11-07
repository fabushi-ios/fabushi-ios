//! Telegram Desktop 1:1 Parity Submodule: `WalletCardStickers`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_stickers.cpp`, `Telegram/SourceFiles/wallet/wallet_card_stickers.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const OUTLINE_SAMPLES: usize = 16;
pub const SOLID_SPAN: f64 = 0.2;
pub const BLEND_SPAN: f64 = 0.8;
pub const SETTLE_DURATION_MS: f64 = 120.0;
pub const RING_DELAY_MS: f64 = 80.0;
pub const RING_DURATION_MS: f64 = 350.0;
pub const LANDING_DURATION_MS: f64 = 220.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedSticker {
    pub id: u64,
    pub document_id: u64,
    pub center_x: f64,
    pub center_y: f64,
    pub scale: f64,
    pub rotation: f64,
    pub ring_elapsed_ms: f64,
    pub is_settled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardStickers {
    pub stickers: Vec<PlacedSticker>,
    pub selected_sticker_id: Option<u64>,
    pub next_id: u64,
}

impl Default for WalletCardStickers {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardStickers {
    pub fn new() -> Self {
        Self {
            stickers: Vec::new(),
            selected_sticker_id: None,
            next_id: 1,
        }
    }

    pub fn place_sticker(
        &mut self,
        document_id: u64,
        center_x: f64,
        center_y: f64,
        scale: f64,
        rotation: f64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.stickers.push(PlacedSticker {
            id,
            document_id,
            center_x,
            center_y,
            scale,
            rotation,
            ring_elapsed_ms: 0.0,
            is_settled: false,
        });
        self.selected_sticker_id = Some(id);
        id
    }

    pub fn step(&mut self, dt_ms: f64) {
        for sticker in &mut self.stickers {
            if !sticker.is_settled {
                sticker.ring_elapsed_ms += dt_ms;
                if sticker.ring_elapsed_ms >= (RING_DELAY_MS + RING_DURATION_MS) {
                    sticker.is_settled = true;
                }
            }
        }
    }

    pub fn hit_test(&self, px: f64, py: f64) -> Option<u64> {
        for sticker in self.stickers.iter().rev() {
            let hit_radius = 24.0 * sticker.scale;
            let dist = (px - sticker.center_x).hypot(py - sticker.center_y);
            if dist <= hit_radius {
                return Some(sticker.id);
            }
        }
        None
    }

    pub fn remove_sticker(&mut self, id: u64) -> bool {
        let initial_len = self.stickers.len();
        self.stickers.retain(|s| s.id != id);
        if self.selected_sticker_id == Some(id) {
            self.selected_sticker_id = None;
        }
        self.stickers.len() < initial_len
    }

    pub fn ring_opacity(&self, sticker_id: u64) -> f32 {
        if let Some(s) = self.stickers.iter().find(|s| s.id == sticker_id) {
            if s.ring_elapsed_ms < RING_DELAY_MS {
                0.0
            } else {
                let active = s.ring_elapsed_ms - RING_DELAY_MS;
                let t = (active / RING_DURATION_MS).min(1.0) as f32;
                1.0 - t
            }
        } else {
            0.0
        }
    }
}
