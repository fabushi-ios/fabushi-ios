//! Telegram Desktop 1:1 Parity Submodule: `WalletCardDrag`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_drag.cpp`, `Telegram/SourceFiles/wallet/wallet_card_drag.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const SQUARE_LIFT_SCALE: f64 = 1.05;
pub const SQUARE_LIFT_DURATION_MS: f64 = 50.0;
pub const SQUARE_RETURN_DURATION_MS: f64 = 185.0;
pub const CARD_LIFT_SCALE: f64 = 1.28;
pub const CARD_LIFT_DURATION_MS: f64 = 100.0;
pub const CARD_RETURN_DURATION_MS: f64 = 140.0;
pub const SETTLE_DURATION_MS: f64 = 110.0;
pub const RETURN_FADE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DraggedGiftSource {
    Square,
    Card,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragPhase {
    Idle,
    Lifting,
    Dragging,
    Settling,
    Returned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardDrag {
    pub source: DraggedGiftSource,
    pub phase: DragPhase,
    pub current_scale: f64,
    pub cursor_x: f64,
    pub cursor_y: f64,
    pub start_x: f64,
    pub start_y: f64,
    pub elapsed_ms: f64,
    pub opacity: f64,
}

impl Default for WalletCardDrag {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardDrag {
    pub fn new() -> Self {
        Self {
            source: DraggedGiftSource::Square,
            phase: DragPhase::Idle,
            current_scale: 1.0,
            cursor_x: 0.0,
            cursor_y: 0.0,
            start_x: 0.0,
            start_y: 0.0,
            elapsed_ms: 0.0,
            opacity: 1.0,
        }
    }

    pub fn start_drag(&mut self, source: DraggedGiftSource, x: f64, y: f64) {
        self.source = source;
        self.phase = DragPhase::Lifting;
        self.current_scale = 1.0;
        self.cursor_x = x;
        self.cursor_y = y;
        self.start_x = x;
        self.start_y = y;
        self.elapsed_ms = 0.0;
        self.opacity = 1.0;
    }

    pub fn update_cursor(&mut self, x: f64, y: f64) {
        if self.phase == DragPhase::Dragging || self.phase == DragPhase::Lifting {
            self.cursor_x = x;
            self.cursor_y = y;
        }
    }

    pub fn release_drag(&mut self, accepted_on_card: bool) {
        if accepted_on_card {
            self.phase = DragPhase::Settling;
        } else {
            self.phase = DragPhase::Returned;
        }
        self.elapsed_ms = 0.0;
    }

    pub fn step(&mut self, dt_ms: f64) {
        self.elapsed_ms += dt_ms;
        match self.phase {
            DragPhase::Idle => {}
            DragPhase::Lifting => {
                let max_duration = match self.source {
                    DraggedGiftSource::Square => SQUARE_LIFT_DURATION_MS,
                    DraggedGiftSource::Card => CARD_LIFT_DURATION_MS,
                };
                let target_scale = match self.source {
                    DraggedGiftSource::Square => SQUARE_LIFT_SCALE,
                    DraggedGiftSource::Card => CARD_LIFT_SCALE,
                };
                let t = (self.elapsed_ms / max_duration).min(1.0);
                self.current_scale = 1.0 + (target_scale - 1.0) * t;
                if t >= 1.0 {
                    self.phase = DragPhase::Dragging;
                }
            }
            DragPhase::Dragging => {
                let target_scale = match self.source {
                    DraggedGiftSource::Square => SQUARE_LIFT_SCALE,
                    DraggedGiftSource::Card => CARD_LIFT_SCALE,
                };
                self.current_scale = target_scale;
            }
            DragPhase::Settling => {
                let t = (self.elapsed_ms / SETTLE_DURATION_MS).min(1.0);
                self.current_scale = 1.0 + (1.28 - 1.0) * (1.0 - t);
                if t >= 1.0 {
                    self.phase = DragPhase::Idle;
                    self.current_scale = 1.0;
                }
            }
            DragPhase::Returned => {
                let max_duration = match self.source {
                    DraggedGiftSource::Square => SQUARE_RETURN_DURATION_MS,
                    DraggedGiftSource::Card => CARD_RETURN_DURATION_MS,
                };
                let t = (self.elapsed_ms / max_duration).min(1.0);
                self.cursor_x = self.cursor_x + (self.start_x - self.cursor_x) * t;
                self.cursor_y = self.cursor_y + (self.start_y - self.cursor_y) * t;
                self.opacity = 1.0 - (1.0 - RETURN_FADE) * t;
                if t >= 1.0 {
                    self.phase = DragPhase::Idle;
                    self.opacity = 1.0;
                    self.current_scale = 1.0;
                }
            }
        }
    }
}
