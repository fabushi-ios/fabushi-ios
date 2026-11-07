//! Telegram Desktop 1:1 Parity Submodule: `WalletCardSpace`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_space.cpp`, `Telegram/SourceFiles/wallet/wallet_card_space.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const CARD_FOLD_MIN_HEIGHT: f64 = 1.0;
pub const OUTLINE_RATIO: f64 = 0.03;
pub const ANCHOR_X: f64 = 0.55;
pub const ANCHOR_Y: f64 = 0.70;
pub const PREFERRED_SIDE: f64 = 0.21;
pub const MIN_DROP_SIDE: f64 = 0.12;
pub const PLACE_TOLERANCE: f64 = 1e-6;
pub const PARALLEL_SINE: f64 = 1e-12;
pub const ROOT_BISECTIONS: usize = 40;
pub const GRID_DIVISIONS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardFold {
    pub rest_x: f64,
    pub rest_y: f64,
    pub rest_w: f64,
    pub rest_h: f64,
    pub quad: [Point2D; 4],
    pub top_y: f64,
    pub bottom_y: f64,
    pub opacity: f64,
}

impl Default for CardFold {
    fn default() -> Self {
        Self {
            rest_x: 0.0,
            rest_y: 0.0,
            rest_w: 320.0,
            rest_h: 200.0,
            quad: [
                Point2D { x: 0.0, y: 0.0 },
                Point2D { x: 320.0, y: 0.0 },
                Point2D { x: 320.0, y: 200.0 },
                Point2D { x: 0.0, y: 200.0 },
            ],
            top_y: 0.0,
            bottom_y: 200.0,
            opacity: 1.0,
        }
    }
}

pub fn compute_quad_corners(w: f64, h: f64, angle_x: f64, angle_y: f64) -> [Point2D; 4] {
    let rad_x = angle_x.to_radians();
    let rad_y = angle_y.to_radians();
    let cos_x = rad_x.cos();
    let sin_x = rad_x.sin();
    let cos_y = rad_y.cos();
    let sin_y = rad_y.sin();

    let half_w = w * 0.5;
    let half_h = h * 0.5;

    let project = |lx: f64, ly: f64| -> Point2D {
        let x_rot = lx * cos_y;
        let y_rot = ly * cos_x - lx * sin_y * sin_x;
        Point2D {
            x: half_w + x_rot,
            y: half_h + y_rot,
        }
    };

    [
        project(-half_w, -half_h),
        project(half_w, -half_h),
        project(half_w, half_h),
        project(-half_w, half_h),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardSpace {
    pub fold: CardFold,
    pub anchor_x: f64,
    pub anchor_y: f64,
}

impl Default for WalletCardSpace {
    fn default() -> Self {
        Self::new(320.0, 200.0)
    }
}

impl WalletCardSpace {
    pub fn new(width: f64, height: f64) -> Self {
        let mut s = Self {
            fold: CardFold::default(),
            anchor_x: ANCHOR_X,
            anchor_y: ANCHOR_Y,
        };
        s.update_projection(width, height, 0.0, 0.0);
        s
    }

    pub fn update_projection(&mut self, w: f64, h: f64, angle_x: f64, angle_y: f64) {
        let quad = compute_quad_corners(w, h, angle_x, angle_y);
        let min_y = quad.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_y = quad.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);

        self.fold = CardFold {
            rest_x: 0.0,
            rest_y: 0.0,
            rest_w: w,
            rest_h: h.max(CARD_FOLD_MIN_HEIGHT),
            quad,
            top_y: min_y,
            bottom_y: max_y,
            opacity: 1.0,
        };
    }

    pub fn contains_point(&self, px: f64, py: f64) -> bool {
        let q = &self.fold.quad;
        let mut inside = false;
        let mut j = 3;
        for i in 0..4 {
            if ((q[i].y > py) != (q[j].y > py))
                && (px < (q[j].x - q[i].x) * (py - q[i].y) / (q[j].y - q[i].y) + q[i].x)
            {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}
