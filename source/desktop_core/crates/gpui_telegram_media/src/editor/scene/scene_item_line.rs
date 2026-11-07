//! Telegram Desktop 1:1 Parity Module: `SceneItemLineScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_line.cpp` (L1-L93)
//! - `Telegram/SourceFiles/editor/scene/scene_item_line.h` (L1-L47)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EraserMask {
    pub center: (f64, f64),
    pub radius: f64,
}

/// Authentic representation of completed painted brush line stroke.
/// Upstream: `scene_item_line.h:L14-L46` & `scene_item_line.cpp:L16-L92`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemLineScene {
    pub line_id: u64,
    pub color_rgba: [u8; 4],
    pub stroke_width: f64,
    pub points: Vec<(f64, f64)>,
    pub bounding_box: (f64, f64, f64, f64),
    pub applied_erasers: Vec<EraserMask>,
    pub is_erased: bool,
}

impl Default for SceneItemLineScene {
    fn default() -> Self {
        Self::new(1, [255, 255, 255, 255], 4.0, vec![(0.0, 0.0), (100.0, 100.0)])
    }
}

impl SceneItemLineScene {
    pub fn new(id: u64, color_rgba: [u8; 4], stroke_width: f64, points: Vec<(f64, f64)>) -> Self {
        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;

        for &(x, y) in &points {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }

        let half_w = stroke_width / 2.0;
        let bbox = (
            min_x - half_w,
            min_y - half_w,
            (max_x - min_x) + stroke_width,
            (max_y - min_y) + stroke_width,
        );

        Self {
            line_id: id,
            color_rgba,
            stroke_width,
            points,
            bounding_box: bbox,
            applied_erasers: Vec::new(),
            is_erased: false,
        }
    }

    /// Applies an eraser mask subtraction at the given position.
    /// Upstream: `scene_item_line.cpp:L24-L35`
    pub fn apply_eraser(&mut self, mask_center: (f64, f64), radius: f64) -> bool {
        let (bx, by, bw, bh) = self.bounding_box;
        let (mx, my) = mask_center;
        if mx >= bx - radius && mx <= bx + bw + radius && my >= by - radius && my <= by + bh + radius {
            self.applied_erasers.push(EraserMask { center: mask_center, radius });
            true
        } else {
            false
        }
    }
}
