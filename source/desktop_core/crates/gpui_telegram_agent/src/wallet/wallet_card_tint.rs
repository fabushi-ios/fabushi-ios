//! Telegram Desktop 1:1 Parity Submodule: `WalletCardTint`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_tint.cpp`, `Telegram/SourceFiles/wallet/wallet_card_tint.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const TEXT_CONTRAST: f64 = 3.1;
pub const SURFACE_CONTRAST: f64 = 1.55;
pub const STRIP_DARKER: f64 = 1.6;
pub const STRIP_LIGHTER: f64 = 3.0;
pub const INK_SURROUND_CONTRAST: f64 = 2.8;
pub const MAX_LUMINANCE: f64 = 1.05 / TEXT_CONTRAST - 0.05;
pub const LINEAR_KNEE: f64 = 0.03928;
pub const MARK_CAST: f64 = 99.0 / 255.0;
pub const MARK_HUE_STRENGTH: f64 = 0.3;

pub fn srgb_to_linear(c_norm: f64) -> f64 {
    if c_norm <= LINEAR_KNEE {
        c_norm / 12.92
    } else {
        ((c_norm + 0.055) / 1.055).powf(2.4)
    }
}

pub fn relative_luminance(rgb: u32) -> f64 {
    let r = srgb_to_linear(((rgb >> 16) & 0xff) as f64 / 255.0);
    let g = srgb_to_linear(((rgb >> 8) & 0xff) as f64 / 255.0);
    let b = srgb_to_linear((rgb & 0xff) as f64 / 255.0);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub fn contrast_ratio(rgb1: u32, rgb2: u32) -> f64 {
    let l1 = relative_luminance(rgb1);
    let l2 = relative_luminance(rgb2);
    let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardTint {
    pub light_rgb: u32,
    pub dark_rgb: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardMarkTint {
    pub hue_rgb: u32,
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardTint {
    pub tint: CardTint,
    pub mark: CardMarkTint,
}

impl Default for WalletCardTint {
    fn default() -> Self {
        Self::new(0x0773fe, 0x0088cc)
    }
}

impl WalletCardTint {
    pub fn new(light_rgb: u32, dark_rgb: u32) -> Self {
        Self {
            tint: CardTint {
                light_rgb,
                dark_rgb,
            },
            mark: CardMarkTint {
                hue_rgb: light_rgb,
                amount: MARK_CAST,
            },
        }
    }

    pub fn is_text_contrast_sufficient(&self, text_rgb: u32) -> bool {
        let cr_light = contrast_ratio(text_rgb, self.tint.light_rgb);
        let cr_dark = contrast_ratio(text_rgb, self.tint.dark_rgb);
        cr_light >= TEXT_CONTRAST && cr_dark >= TEXT_CONTRAST
    }

    pub fn compute_adapted_text_color(&self) -> u32 {
        let light_lum = relative_luminance(self.tint.light_rgb);
        if light_lum > MAX_LUMINANCE {
            0x111111
        } else {
            0xffffff
        }
    }
}
