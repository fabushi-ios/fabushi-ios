//! Telegram Desktop 1:1 Parity Submodule: `WalletCardGradient`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_gradient.cpp`, `Telegram/SourceFiles/wallet/wallet_card_gradient.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const SWEEP_STEP: f64 = 15.0;
pub const QR_PLATE_ANGLE: f64 = 119.72;
pub const QR_PLATE_FROM: f64 = 0.0799;
pub const QR_PLATE_TILL: f64 = 0.9201;
pub const QR_PLATE_ALPHA: f64 = 0.08;
pub const TINT_RAMP_STEPS: usize = 16;
pub const SOFTNESS: f64 = 0.5;

pub const SWEEP_PALETTE: [u32; 24] = [
    0x0773fe, 0x046bfe, 0x0368fe, 0x046cff, 0x0872ff, 0x0b7aff,
    0x1084ff, 0x1594ff, 0x1ea3fe, 0x1da4fe, 0x169cfe, 0x0e94ff,
    0x098cff, 0x0483ff, 0x027eff, 0x057efe, 0x0980ff, 0x0c82ff,
    0x1083fe, 0x178efe, 0x1b9afe, 0x1a96fe, 0x1488ff, 0x0d7dff,
];

pub fn sample_conical_gradient(angle_deg: f64) -> u32 {
    let normalized = angle_deg.rem_euclid(360.0);
    let index = ((normalized / SWEEP_STEP).floor() as usize) % 24;
    let next_index = (index + 1) % 24;
    let frac = (normalized % SWEEP_STEP) / SWEEP_STEP;

    let c1 = SWEEP_PALETTE[index];
    let c2 = SWEEP_PALETTE[next_index];

    let r1 = ((c1 >> 16) & 0xff) as f64;
    let g1 = ((c1 >> 8) & 0xff) as f64;
    let b1 = (c1 & 0xff) as f64;

    let r2 = ((c2 >> 16) & 0xff) as f64;
    let g2 = ((c2 >> 8) & 0xff) as f64;
    let b2 = (c2 & 0xff) as f64;

    let r = (r1 + (r2 - r1) * frac).round() as u32;
    let g = (g1 + (g2 - g1) * frac).round() as u32;
    let b = (b1 + (b2 - b1) * frac).round() as u32;

    (r << 16) | (g << 8) | b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardGradient {
    pub current_angle: f64,
    pub custom_tint: Option<(u32, u32)>,
    pub qr_plate_alpha: f64,
}

impl Default for WalletCardGradient {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardGradient {
    pub fn new() -> Self {
        Self {
            current_angle: 0.0,
            custom_tint: None,
            qr_plate_alpha: QR_PLATE_ALPHA,
        }
    }

    pub fn set_angle(&mut self, angle_deg: f64) {
        self.current_angle = angle_deg.rem_euclid(360.0);
    }

    pub fn set_custom_tint(&mut self, light: u32, dark: u32) {
        self.custom_tint = Some((light, dark));
    }

    pub fn clear_custom_tint(&mut self) {
        self.custom_tint = None;
    }

    pub fn sample_current_color(&self) -> u32 {
        if let Some((light, dark)) = self.custom_tint {
            let frac = (self.current_angle / 360.0).clamp(0.0, 1.0);
            let r1 = ((light >> 16) & 0xff) as f64;
            let g1 = ((light >> 8) & 0xff) as f64;
            let b1 = (light & 0xff) as f64;
            let r2 = ((dark >> 16) & 0xff) as f64;
            let g2 = ((dark >> 8) & 0xff) as f64;
            let b2 = (dark & 0xff) as f64;
            let r = (r1 + (r2 - r1) * frac).round() as u32;
            let g = (g1 + (g2 - g1) * frac).round() as u32;
            let b = (b1 + (b2 - b1) * frac).round() as u32;
            (r << 16) | (g << 8) | b
        } else {
            sample_conical_gradient(self.current_angle)
        }
    }

    pub fn qr_plate_stops(&self) -> Vec<(f64, f64)> {
        vec![
            (0.0, 0.0),
            (QR_PLATE_FROM, self.qr_plate_alpha),
            (QR_PLATE_TILL, self.qr_plate_alpha),
            (1.0, 0.0),
        ]
    }
}
