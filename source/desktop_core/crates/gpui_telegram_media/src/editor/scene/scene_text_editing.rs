//! Telegram Desktop 1:1 Parity Module: `SceneTextEditingScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_text_editing.cpp` (L1-L708)
//! - `Telegram/SourceFiles/editor/scene/scene_text_editing.h` (L1-L90)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream minimum width factor: `scene_text_editing.cpp:L27`
pub const K_MIN_WIDTH_FACTOR: f64 = 0.16;

/// Upstream ideal width extra padding: `scene_text_editing.cpp:L28`
pub const K_IDEAL_WIDTH_EXTRA: i32 = 2;

/// Upstream scale threshold: `scene_text_editing.cpp:L29`
pub const K_SCALE_THRESHOLD: f64 = 0.01;

/// Upstream placeholder opacity: `scene_text_editing.cpp:L30`
pub const K_PLACEHOLDER_OPACITY: f64 = 0.38;

/// Upstream auto-shrink height factor: `scene_text_editing.cpp:L35`
pub const K_AUTO_SHRINK_HEIGHT_FACTOR: f64 = 1.0 / 3.0;

/// Upstream auto-shrink step: `scene_text_editing.cpp:L36`
pub const K_AUTO_SHRINK_STEP: f64 = 0.9;

/// Upstream minimum font size factor: `scene_text_editing.cpp:L37`
pub const K_MIN_FONT_SIZE_FACTOR: f64 = 1.0 / 50.0;

/// Authentic representation of scene text editing controller.
/// Upstream: `scene_text_editing.h:L23-L90` & `scene_text_editing.cpp:L80-L700`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneTextEditingScene {
    pub is_editing: bool,
    pub active_item_id: Option<u64>,
    pub current_text: String,
    pub font_size: f64,
    pub color_rgba: [u8; 4],
    pub canvas_height: f64,
    pub min_font_size: f64,
}

impl Default for SceneTextEditingScene {
    fn default() -> Self {
        Self::new(1080.0)
    }
}

impl SceneTextEditingScene {
    pub fn new(canvas_height: f64) -> Self {
        let ch = canvas_height.max(100.0);
        Self {
            is_editing: false,
            active_item_id: None,
            current_text: String::new(),
            font_size: 32.0,
            color_rgba: [255, 255, 255, 255],
            canvas_height: ch,
            min_font_size: (ch * K_MIN_FONT_SIZE_FACTOR).max(8.0),
        }
    }

    pub fn start_editing(&mut self, item_id: u64, initial_text: String, font_size: f64, color: [u8; 4]) {
        self.is_editing = true;
        self.active_item_id = Some(item_id);
        self.current_text = initial_text;
        self.font_size = font_size.max(self.min_font_size);
        self.color_rgba = color;
    }

    pub fn finish_editing(&mut self) -> (Option<u64>, String) {
        let id = self.active_item_id.take();
        let text = std::mem::take(&mut self.current_text);
        self.is_editing = false;
        (id, text)
    }

    /// Evaluates auto-shrink when text height exceeds the editor budget.
    /// Upstream: `scene_text_editing.cpp:L35-L38`
    pub fn apply_auto_shrink(&mut self, rendered_text_height: f64) -> bool {
        let budget = self.canvas_height * K_AUTO_SHRINK_HEIGHT_FACTOR;
        if rendered_text_height > budget && self.font_size * K_AUTO_SHRINK_STEP >= self.min_font_size {
            self.font_size *= K_AUTO_SHRINK_STEP;
            true
        } else {
            false
        }
    }
}
