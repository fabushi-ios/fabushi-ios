//! Standalone Calls Top-Level Window Host
//! Upstream: Telegram/SourceFiles/calls/calls_window.h:L45-L115, calls_window.cpp:L25-L295
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowBackend {
    Software,
    OpenGL,
    Metal,
    Vulkan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsWindow {
    pub backend: WindowBackend,
    pub is_pinned_on_top: bool,
    pub is_full_screen: bool,
    pub power_save_blocker_active: bool,
    pub controls_shown_ratio: f32,
    pub active_layer_id: Option<String>,
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
}

impl CallsWindow {
    pub const DEFAULT_WIDTH: f32 = 720.0;
    pub const DEFAULT_HEIGHT: f32 = 540.0;
    pub const MIN_WIDTH: f32 = 380.0;
    pub const MIN_HEIGHT: f32 = 520.0;

    pub fn new() -> Self {
        Self {
            backend: WindowBackend::Metal,
            is_pinned_on_top: false,
            is_full_screen: false,
            power_save_blocker_active: true,
            controls_shown_ratio: 1.0,
            active_layer_id: None,
            width: Self::DEFAULT_WIDTH,
            height: Self::DEFAULT_HEIGHT,
            min_width: Self::MIN_WIDTH,
            min_height: Self::MIN_HEIGHT,
        }
    }

    pub fn toggle_pin_on_top(&mut self) -> bool {
        self.is_pinned_on_top = !self.is_pinned_on_top;
        self.is_pinned_on_top
    }

    pub fn toggle_full_screen(&mut self) -> bool {
        self.is_full_screen = !self.is_full_screen;
        if self.is_full_screen {
            self.is_pinned_on_top = false;
        }
        self.is_full_screen
    }

    pub fn set_power_save_blocker(&mut self, enabled: bool) {
        self.power_save_blocker_active = enabled;
    }

    pub fn set_controls_shown(&mut self, ratio: f32) {
        self.controls_shown_ratio = ratio.clamp(0.0, 1.0);
    }

    pub fn show_layer(&mut self, layer_id: impl Into<String>) {
        self.active_layer_id = Some(layer_id.into());
    }

    pub fn hide_layer(&mut self) {
        self.active_layer_id = None;
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width.max(self.min_width);
        self.height = height.max(self.min_height);
    }
}
