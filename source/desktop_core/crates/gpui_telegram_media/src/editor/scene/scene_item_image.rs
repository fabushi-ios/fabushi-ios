//! Telegram Desktop 1:1 Parity Module: `SceneItemImageScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_image.cpp` (L1-L48)
//! - `Telegram/SourceFiles/editor/scene/scene_item_image.h` (L1-L32)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Authentic representation of static raster photo scene element.
/// Upstream: `scene_item_image.h:L14-L31` & `scene_item_image.cpp:L16-L47`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemImageScene {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub aspect_ratio: f64,
    pub flipped: bool,
    pub file_path: Option<String>,
}

impl Default for SceneItemImageScene {
    fn default() -> Self {
        Self::new(1, 800, 600)
    }
}

impl SceneItemImageScene {
    pub fn new(id: u64, width: u32, height: u32) -> Self {
        let w = width.max(1);
        let h = height.max(1);
        Self {
            id,
            width: w,
            height: h,
            aspect_ratio: (h as f64) / (w as f64),
            flipped: false,
            file_path: None,
        }
    }

    pub fn perform_flip(&mut self) {
        self.flipped = !self.flipped;
    }

    /// Duplicate item with upstream standard 1/3 size offset.
    /// Upstream: `scene_item_base.cpp:L562-L563`
    pub fn duplicate_offset(&self, current_size: f64) -> (f64, f64) {
        let offset = current_size / 3.0;
        (offset, offset)
    }
}
