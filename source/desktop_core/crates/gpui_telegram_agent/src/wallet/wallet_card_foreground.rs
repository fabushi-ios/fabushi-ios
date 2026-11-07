//! Telegram Desktop 1:1 Parity Submodule: `WalletCardForeground`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_foreground.cpp`, `Telegram/SourceFiles/wallet/wallet_card_foreground.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const SECONDARY_LIFT: f32 = 0.7;
pub const HALO_SPREAD: u32 = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardHaloEffect {
    pub center_x: f64,
    pub center_y: f64,
    pub radius: f64,
    pub spread: u32,
    pub alpha: f32,
}

impl Default for CardHaloEffect {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_y: 0.0,
            radius: 40.0,
            spread: HALO_SPREAD,
            alpha: 0.25,
        }
    }
}

pub fn compute_specular_sheen(base_rgb: u32, lift: f32) -> u32 {
    let r = ((base_rgb >> 16) & 0xff) as f32;
    let g = ((base_rgb >> 8) & 0xff) as f32;
    let b = (base_rgb & 0xff) as f32;

    let nr = (r + (255.0 - r) * lift).clamp(0.0, 255.0) as u32;
    let ng = (g + (255.0 - g) * lift).clamp(0.0, 255.0) as u32;
    let nb = (b + (255.0 - b) * lift).clamp(0.0, 255.0) as u32;

    (nr << 16) | (ng << 8) | nb
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardForeground {
    pub halo: CardHaloEffect,
    pub facet_progress: f32,
    pub is_highlighted: bool,
    pub secondary_lift: f32,
}

impl Default for WalletCardForeground {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardForeground {
    pub fn new() -> Self {
        Self {
            halo: CardHaloEffect::default(),
            facet_progress: 0.0,
            is_highlighted: false,
            secondary_lift: SECONDARY_LIFT,
        }
    }

    pub fn set_halo(&mut self, cx: f64, cy: f64, radius: f64, alpha: f32) {
        self.halo = CardHaloEffect {
            center_x: cx,
            center_y: cy,
            radius,
            spread: HALO_SPREAD,
            alpha: alpha.clamp(0.0, 1.0),
        };
        self.is_highlighted = true;
    }

    pub fn clear_halo(&mut self) {
        self.is_highlighted = false;
        self.halo.alpha = 0.0;
    }

    pub fn update_facet_shine(&mut self, dt_progress: f32) {
        self.facet_progress = (self.facet_progress + dt_progress).rem_euclid(1.0);
    }

    pub fn current_sheen_color(&self, base_rgb: u32) -> u32 {
        let shine_factor = (self.facet_progress * std::f32::consts::PI * 2.0).sin().max(0.0);
        compute_specular_sheen(base_rgb, self.secondary_lift * shine_factor)
    }
}
