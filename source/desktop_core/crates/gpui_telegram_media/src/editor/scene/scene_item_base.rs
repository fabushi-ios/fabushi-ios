//! Telegram Desktop 1:1 Parity Module: `SceneItemBaseScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_base.cpp` (L1-L872)
//! - `Telegram/SourceFiles/editor/scene/scene_item_base.h` (L1-L267)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream rotation snap angle: `scene_item_base.cpp:L27`
pub const K_SNAP_ANGLE: f64 = 45.0;

/// Upstream minimum size ratio: `scene_item_base.cpp:L33`
pub const K_MIN_SIZE_RATIO: f64 = 0.05;

/// Upstream sticky snap duration in milliseconds: `scene_item_base.cpp:L35`
pub const K_STICKY_DURATION_MS: u64 = 150;

/// Upstream number of sticky guidelines: `scene_item_base.cpp:L36`
pub const K_STICKY_LINES: usize = 5;

/// Normalizes an angle into `[-360.0, 360.0]` range.
/// Upstream: `scene_item_base.cpp:L38-L41`
#[inline]
pub fn normalize_angle(angle: f64) -> f64 {
    if angle.abs() < 360.0 {
        angle
    } else {
        angle - 360.0 * (if angle < 0.0 { -1.0 } else { 1.0 })
    }
}

/// Snaps an angle to the nearest 45-degree increment when Shift is held.
/// Upstream: `scene_item_base.cpp:L228`
#[inline]
pub fn snap_angle(angle: f64) -> f64 {
    let normalized = normalize_angle(angle);
    (normalized / K_SNAP_ANGLE).round() * K_SNAP_ANGLE
}

/// Computes the sticky coordinate for a given guideline index.
/// Upstream: `scene_item_base.cpp:L55-L61`
#[inline]
pub fn sticky_line(start: f64, length: f64, line_index: usize) -> f64 {
    let index = line_index.min(K_STICKY_LINES - 1);
    start + length * (index as f64) / ((K_STICKY_LINES - 1) as f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ItemStatus {
    #[default]
    Normal,
    Undid,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HandleType {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StickyAnchor {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPlacement {
    pub position: (f64, f64),
    pub rotation: f64,
    pub scale: f64,
    pub z_value: f64,
    pub size: f64,
    pub aspect_ratio: f64,
    pub bend: f64,
    pub font_size: f64,
    pub flipped: bool,
}

impl Default for ItemPlacement {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0),
            rotation: 0.0,
            scale: 1.0,
            z_value: 0.0,
            size: 100.0,
            aspect_ratio: 1.0,
            bend: 0.0,
            font_size: 14.0,
            flipped: false,
        }
    }
}

/// Authentic representation of scene item affine transform and interaction state.
/// Upstream: `scene_item_base.h:L67-L265` & `scene_item_base.cpp:L130-L248`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemBaseScene {
    pub id: u64,
    pub number: usize,
    pub status: ItemStatus,
    pub undoable: bool,
    pub placement: ItemPlacement,
    pub horizontal_size: f64,
    pub vertical_size: f64,
    pub min_size: f64,
    pub max_size: f64,
    pub scaled_handle_size: f64,
    pub active_handle: HandleType,
    pub is_dragging: bool,
}

impl Default for SceneItemBaseScene {
    fn default() -> Self {
        Self::new(1, (0.0, 0.0), 120.0, 1.0)
    }
}

impl SceneItemBaseScene {
    pub fn new(id: u64, pos: (f64, f64), size: f64, aspect_ratio: f64) -> Self {
        let h_size = size.max(16.0);
        let v_size = (h_size * aspect_ratio).max(16.0);
        let mut placement = ItemPlacement::default();
        placement.position = pos;
        placement.size = h_size;
        placement.aspect_ratio = aspect_ratio;

        Self {
            id,
            number: 0,
            status: ItemStatus::Normal,
            undoable: true,
            placement,
            horizontal_size: h_size,
            vertical_size: v_size,
            min_size: 16.0,
            max_size: 4096.0,
            scaled_handle_size: 12.0,
            active_handle: HandleType::None,
            is_dragging: false,
        }
    }

    /// Inner bounding rectangle centered at origin `[-w/2, -h/2, w, h]`.
    /// Upstream: `scene_item_base.cpp:L153-L157`
    pub fn inner_rect(&self) -> (f64, f64, f64, f64) {
        (
            -self.horizontal_size / 2.0,
            -self.vertical_size / 2.0,
            self.horizontal_size,
            self.vertical_size,
        )
    }

    /// Right handle bounding box.
    /// Upstream: `scene_item_base.cpp:L623-L629`
    pub fn right_handle_rect(&self) -> (f64, f64, f64, f64) {
        (
            (self.horizontal_size / 2.0) - (self.scaled_handle_size / 2.0),
            -(self.scaled_handle_size / 2.0),
            self.scaled_handle_size,
            self.scaled_handle_size,
        )
    }

    /// Left handle bounding box.
    /// Upstream: `scene_item_base.cpp:L631-L637`
    pub fn left_handle_rect(&self) -> (f64, f64, f64, f64) {
        (
            (-self.horizontal_size / 2.0) - (self.scaled_handle_size / 2.0),
            -(self.scaled_handle_size / 2.0),
            self.scaled_handle_size,
            self.scaled_handle_size,
        )
    }

    /// Applies resize dragging math from mouse coordinate.
    /// Upstream: `scene_item_base.cpp:L207-L218`
    pub fn apply_resize(&mut self, mouse_x: f64, mouse_y: f64, is_left: bool) {
        let (px, py) = if is_left { (-mouse_x, -mouse_y) } else { (mouse_x, mouse_y) };
        let dx = (2.0 * px).abs();
        let dy = (2.0 * py).abs();
        let target = if dx > dy { dx } else { dy };
        self.horizontal_size = target.clamp(self.min_size, self.max_size);
        self.vertical_size = (self.horizontal_size * self.placement.aspect_ratio).max(self.min_size);
        self.placement.size = self.horizontal_size;
    }

    /// Applies rotation calculation with optional Shift angle snapping.
    /// Upstream: `scene_item_base.cpp:L221-L229`
    pub fn apply_rotation(&mut self, origin: (f64, f64), mouse_pos: (f64, f64), is_left: bool, snap: bool) {
        let diff_x = mouse_pos.0 - origin.0;
        let diff_y = mouse_pos.1 - origin.1;
        let base_angle = diff_y.atan2(diff_x) * 180.0 / std::f64::consts::PI;
        let offset = if is_left { 180.0 } else { 0.0 };
        let angle = normalize_angle(offset + base_angle);
        self.placement.rotation = if snap { snap_angle(angle) } else { angle };
    }

    pub fn set_flipped(&mut self, flipped: bool) {
        self.placement.flipped = flipped;
    }

    pub fn is_flipped(&self) -> bool {
        self.placement.flipped
    }
}
