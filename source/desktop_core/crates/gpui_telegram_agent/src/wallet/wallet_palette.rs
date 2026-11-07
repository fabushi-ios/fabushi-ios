//! Authentic TON Wallet Dynamic Palette Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_palette.cpp, Telegram/SourceFiles/wallet/wallet_palette.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_DERIVED_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Converts sRGB channel [0..255] to linear luminance component
    pub fn to_linear(channel: u8) -> f64 {
        let val = (channel as f64) / 255.0;
        if val <= 0.04045 {
            val / 12.92
        } else {
            ((val + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Computes relative luminance (Rec. 709)
    pub fn luminance(&self) -> f64 {
        0.2126 * Self::to_linear(self.r)
            + 0.7152 * Self::to_linear(self.g)
            + 0.0722 * Self::to_linear(self.b)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletPalette {
    pub primary_accent: RgbColor,
    pub background: RgbColor,
    pub surface: RgbColor,
    pub text_primary: RgbColor,
    pub is_dark: bool,
}

impl WalletPalette {
    pub fn from_card_color(card_color: RgbColor) -> Self {
        let lum = card_color.luminance();
        let is_dark = lum < 0.5;

        let (background, surface, text_primary) = if is_dark {
            (
                RgbColor::new(18, 24, 32),
                RgbColor::new(28, 37, 50),
                RgbColor::new(255, 255, 255),
            )
        } else {
            (
                RgbColor::new(245, 247, 250),
                RgbColor::new(255, 255, 255),
                RgbColor::new(16, 22, 28),
            )
        };

        Self {
            primary_accent: card_color,
            background,
            surface,
            text_primary,
            is_dark,
        }
    }
}

impl Default for WalletPalette {
    fn default() -> Self {
        Self::from_card_color(RgbColor::new(0, 136, 204)) // Telegram Blue
    }
}
