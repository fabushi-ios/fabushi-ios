//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupViewportGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport.cpp:L41-L53, L524-L613`

use serde::{Deserialize, Serialize};

pub const K_SHADOW_MAX_ALPHA: u8 = 80;
pub const K_GROUP_CALL_VIDEO_LARGE_SKIP: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn center_x(&self) -> i32 {
        self.x + (self.width as i32) / 2
    }

    pub fn center_y(&self) -> i32 {
        self.y + (self.height as i32) / 2
    }

    pub fn translated(&self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            width: self.width,
            height: self.height,
        }
    }
}

/// Upstream InterpolateRect for smooth tile transitions
/// calls_group_viewport.cpp:L41-L53
pub fn interpolate_rect(a: Rect, b: Rect, ratio: f64) -> Rect {
    let r = ratio.clamp(0.0, 1.0);
    let x = (a.x as f64) + ((b.x - a.x) as f64) * r;
    let y = (a.y as f64) + ((b.y - a.y) as f64) * r;
    let w = (a.width as f64) + ((b.width as f64) - (a.width as f64)) * r;
    let h = (a.height as f64) + ((b.height as f64) - (a.height as f64)) * r;
    Rect::new(x.round() as i32, y.round() as i32, w.round().max(1.0) as u32, h.round().max(1.0) as u32)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileGeometry {
    pub tile_id: u64,
    pub aspect_width: u32,
    pub aspect_height: u32,
    pub rect_rows: Rect,
    pub rect_columns: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportLayout {
    pub tiles: Vec<TileGeometry>,
    pub outer_width: u32,
    pub outer_height: u32,
    pub use_columns: bool,
}

/// Dynamic N-tile Grid Layout: Black-bar Area Minimization
/// calls_group_viewport.cpp:L524-L613: countWide
pub fn count_wide(tiles: &[(u64, u32, u32)], outer_w: u32, outer_h: u32) -> ViewportLayout {
    let count = tiles.len();
    if count == 0 {
        return ViewportLayout { tiles: Vec::new(), outer_width: outer_w, outer_height: outer_h, use_columns: false };
    }
    if count == 1 {
        let (id, aw, ah) = tiles[0];
        let single_rect = Rect::new(0, 0, outer_w, outer_h);
        return ViewportLayout {
            tiles: vec![TileGeometry { tile_id: id, aspect_width: aw, aspect_height: ah, rect_rows: single_rect, rect_columns: single_rect }],
            outer_width: outer_w,
            outer_height: outer_h,
            use_columns: false,
        };
    }

    let slices = ((count as f64).sqrt().ceil() as usize).max(1);
    let skip = K_GROUP_CALL_VIDEO_LARGE_SKIP;

    // Test columns layout
    let mut cols_geoms = Vec::with_capacity(count);
    let mut columns_black: u64 = 0;
    let col_count = slices;
    let col_w_f = (outer_w + skip) as f64 / col_count as f64;
    let mut idx = 0;
    for col in 0..col_count {
        let left = (col as f64 * col_w_f).round() as i32;
        let right = ((col as f64 * col_w_f + col_w_f) - skip as f64).round() as i32;
        let w = (right - left).max(1) as u32;
        let row_count = ((count - idx) as f64 / (col_count - col) as f64).round() as usize;
        let row_h_f = (outer_h + skip) as f64 / row_count.max(1) as f64;
        for row in 0..row_count {
            if idx >= count { break; }
            let top = (row as f64 * row_h_f).round() as i32;
            let bot = ((row as f64 * row_h_f + row_h_f) - skip as f64).round() as i32;
            let h = (bot - top).max(1) as u32;
            let (id, aw, ah) = tiles[idx];
            let scaled_w = ((w as f64) * (ah as f64) / (aw as f64).max(1.0)) as u32;
            let black = if scaled_w < h { (h - scaled_w) as u64 * w as u64 } else { 0 };
            columns_black += black;
            cols_geoms.push((id, aw, ah, Rect::new(left, top, w, h)));
            idx += 1;
        }
    }

    // Test rows layout
    let mut rows_geoms = Vec::with_capacity(count);
    let mut rows_black: u64 = 0;
    let row_count = slices;
    let row_h_f = (outer_h + skip) as f64 / row_count as f64;
    idx = 0;
    for row in 0..row_count {
        let top = (row as f64 * row_h_f).round() as i32;
        let bot = ((row as f64 * row_h_f + row_h_f) - skip as f64).round() as i32;
        let h = (bot - top).max(1) as u32;
        let col_c = ((count - idx) as f64 / (row_count - row) as f64).round() as usize;
        let col_w_f = (outer_w + skip) as f64 / col_c.max(1) as f64;
        for col in 0..col_c {
            if idx >= count { break; }
            let left = (col as f64 * col_w_f).round() as i32;
            let right = ((col as f64 * col_w_f + col_w_f) - skip as f64).round() as i32;
            let w = (right - left).max(1) as u32;
            let (id, aw, ah) = tiles[idx];
            let scaled_h = ((h as f64) * (aw as f64) / (ah as f64).max(1.0)) as u32;
            let black = if scaled_h < w { (w - scaled_h) as u64 * h as u64 } else { 0 };
            rows_black += black;
            rows_geoms.push((id, aw, ah, Rect::new(left, top, w, h)));
            idx += 1;
        }
    }

    let use_columns = columns_black < rows_black;
    let mut out_tiles = Vec::with_capacity(count);
    for i in 0..count {
        let (id, aw, ah, c_rect) = cols_geoms.get(i).cloned().unwrap_or((0, 1, 1, Rect::default()));
        let (_, _, _, r_rect) = rows_geoms.get(i).cloned().unwrap_or((0, 1, 1, Rect::default()));
        out_tiles.push(TileGeometry { tile_id: id, aspect_width: aw, aspect_height: ah, rect_rows: r_rect, rect_columns: c_rect });
    }

    ViewportLayout { tiles: out_tiles, outer_width: outer_w, outer_height: outer_h, use_columns }
}

/// Group Call Viewport Manager
/// Target Symbol: `CallsGroupViewportGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupViewportGroup {
    pub outer_width: u32,
    pub outer_height: u32,
    pub is_fullscreen: bool,
    pub controls_shown_ratio: f32,
    pub spotlight_tile_id: Option<u64>,
    pub active_endpoints: Vec<u64>,
}

impl Default for CallsGroupViewportGroup {
    fn default() -> Self {
        Self {
            outer_width: 1280,
            outer_height: 720,
            is_fullscreen: false,
            controls_shown_ratio: 1.0,
            spotlight_tile_id: None,
            active_endpoints: Vec::new(),
        }
    }
}

impl CallsGroupViewportGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        self.outer_width = width.max(1);
        self.outer_height = height.max(1);
    }

    pub fn add_tile(&mut self, endpoint_id: u64) {
        if !self.active_endpoints.contains(&endpoint_id) {
            self.active_endpoints.push(endpoint_id);
        }
    }

    pub fn remove_tile(&mut self, endpoint_id: u64) {
        self.active_endpoints.retain(|&id| id != endpoint_id);
        if self.spotlight_tile_id == Some(endpoint_id) {
            self.spotlight_tile_id = None;
        }
    }

    pub fn set_spotlight(&mut self, endpoint_id: Option<u64>) {
        self.spotlight_tile_id = endpoint_id;
    }

    pub fn compute_layout(&self) -> ViewportLayout {
        let tiles_with_aspect: Vec<(u64, u32, u32)> = self.active_endpoints.iter().map(|&id| (id, 16, 9)).collect();
        count_wide(&tiles_with_aspect, self.outer_width, self.outer_height)
    }
}
