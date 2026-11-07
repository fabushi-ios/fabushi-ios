//! Telegram Desktop 1:1 Parity Submodule: `EditorLayerWidget`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_layer_widget.h:L1-L51`, `Telegram/SourceFiles/editor/editor_layer_widget.cpp:L1-L93`

use serde::{Deserialize, Serialize};

/// Computes the alpha channel byte for the layer blur backdrop.
/// Upstream: `editor_layer_widget.cpp:L17-L19`
#[inline]
pub fn blur_overlay_alpha(night: bool) -> u8 {
    if night {
        128
    } else {
        192
    }
}

/// Target symbol EditorLayerWidget providing modal backdrop presentation and event routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorLayerWidget {
    pub is_night_theme: bool,
    pub is_started: bool,
    pub width: i32,
    pub height: i32,
    pub forwarding_key_event: bool,
}

pub type LayerWidget = EditorLayerWidget;

impl Default for EditorLayerWidget {
    fn default() -> Self {
        Self {
            is_night_theme: false,
            is_started: false,
            width: 1280,
            height: 720,
            forwarding_key_event: false,
        }
    }
}

impl EditorLayerWidget {
    pub fn new(night: bool) -> Self {
        Self {
            is_night_theme: night,
            ..Default::default()
        }
    }

    pub fn backdrop_alpha(&self) -> u8 {
        blur_overlay_alpha(self.is_night_theme)
    }

    pub fn start(&mut self) {
        self.is_started = true;
    }

    pub fn parent_resized(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        if !self.is_started {
            self.start();
        }
    }

    /// Upstream: `editor_layer_widget.cpp:L88-L90`
    pub fn close_by_outside_click(&self) -> bool {
        false
    }
}
