//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPipView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_pip.h`
//! - `Telegram/SourceFiles/media/view/media_view_pip.cpp:L51-L52, L69-L95, L97-L120`

use serde::{Deserialize, Serialize};

pub const K_PIP_LOADER_PRIORITY: i32 = 2;
pub const PIP_BORDER_SKIP: i32 = 12;
pub const PIP_BORDER_SNAP_AREA: i32 = 20;

/// Rectangular part flags for drag resizing and magnetic edge snapping
/// Upstream: `media_view_pip.h:L63-L68`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RectPart {
    #[default]
    None,
    Center,
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

/// Snaps PiP floating window edges to screen bounds
/// Upstream: `media_view_pip.cpp:L69-L95`
pub fn clamp_to_edges(
    screen_x: i32,
    screen_y: i32,
    screen_w: i32,
    screen_h: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> (i32, i32) {
    let sleft = screen_x + PIP_BORDER_SKIP;
    let stop = screen_y + PIP_BORDER_SKIP;
    let sright = screen_x + screen_w - PIP_BORDER_SKIP;
    let sbottom = screen_y + screen_h - PIP_BORDER_SKIP;

    let ileft = x;
    let itop = y;
    let iright = x + w;
    let ibottom = y + h;

    let mut shift_x = 0;
    let mut shift_y = 0;

    if (iright + shift_x) >= (sright - PIP_BORDER_SNAP_AREA) && (iright + shift_x) < (sright + PIP_BORDER_SNAP_AREA) {
        shift_x += sright - iright;
    }
    if (ileft + shift_x) >= (sleft - PIP_BORDER_SNAP_AREA) && (ileft + shift_x) < (sleft + PIP_BORDER_SNAP_AREA) {
        shift_x += sleft - ileft;
    }
    if (ibottom + shift_y) >= (sbottom - PIP_BORDER_SNAP_AREA) && (ibottom + shift_y) < (sbottom + PIP_BORDER_SNAP_AREA) {
        shift_y += sbottom - ibottom;
    }
    if (itop + shift_y) >= (stop - PIP_BORDER_SNAP_AREA) && (itop + shift_y) < (stop + PIP_BORDER_SNAP_AREA) {
        shift_y += stop - itop;
    }

    (x + shift_x, y + shift_y)
}

/// Resizes geometry bounded by minimum and maximum dimensions
/// Upstream: `media_view_pip.cpp:L97-L120`
pub fn transform_geometry(
    mut x: i32,
    mut y: i32,
    mut w: i32,
    mut h: i32,
    min_w: i32,
    min_h: i32,
    max_w: i32,
    max_h: i32,
    delta_x: i32,
    delta_y: i32,
    by: RectPart,
) -> (i32, i32, i32, i32) {
    let x1 = w - min_w;
    let x2 = max_w - w;
    let y1 = h - min_h;
    let y2 = max_h - h;

    match by {
        RectPart::Center => (x + delta_x, y + delta_y, w, h),
        RectPart::TopLeft => {
            y += delta_y.clamp(-y2, y1);
            x += delta_x.clamp(-x2, x1);
            w -= delta_x.clamp(-x2, x1);
            h -= delta_y.clamp(-y2, y1);
            (x, y, w, h)
        }
        RectPart::TopRight => {
            y += delta_y.clamp(-y2, y1);
            w += delta_x.clamp(-x1, x2);
            h -= delta_y.clamp(-y2, y1);
            (x, y, w, h)
        }
        RectPart::BottomRight => {
            w += delta_x.clamp(-x1, x2);
            h += delta_y.clamp(-y1, y2);
            (x, y, w, h)
        }
        RectPart::BottomLeft => {
            x += delta_x.clamp(-x2, x1);
            w -= delta_x.clamp(-x2, x1);
            h += delta_y.clamp(-y1, y2);
            (x, y, w, h)
        }
        _ => (x, y, w, h),
    }
}

/// Interactive control regions on PiP overlay
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PipOverState {
    #[default]
    None,
    Close,
    Enlarge,
    Playback,
    VolumeToggle,
    VolumeController,
}

/// Primary target symbol for `media_view_pip.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPipView {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub min_width: i32,
    pub min_height: i32,
    pub max_width: i32,
    pub max_height: i32,
    pub is_playing: bool,
    pub volume: f64,
    pub over: PipOverState,
    pub is_active: bool,
}

impl Default for MediaViewPipView {
    fn default() -> Self {
        Self {
            x: 100,
            y: 100,
            width: 320,
            height: 180,
            min_width: 160,
            min_height: 90,
            max_width: 640,
            max_height: 360,
            is_playing: true,
            volume: 1.0,
            over: PipOverState::None,
            is_active: false,
        }
    }
}

impl MediaViewPipView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, width: i32, height: i32) {
        self.width = width.clamp(self.min_width, self.max_width);
        self.height = height.clamp(self.min_height, self.max_height);
        self.is_active = true;
    }

    pub fn close(&mut self) {
        self.is_active = false;
    }

    pub fn snap_to_screen(&mut self, screen_w: i32, screen_h: i32) {
        let (nx, ny) = clamp_to_edges(0, 0, screen_w, screen_h, self.x, self.y, self.width, self.height);
        self.x = nx;
        self.y = ny;
    }

    pub fn drag_resize(&mut self, delta_x: i32, delta_y: i32, corner: RectPart) {
        let (nx, ny, nw, nh) = transform_geometry(
            self.x,
            self.y,
            self.width,
            self.height,
            self.min_width,
            self.min_height,
            self.max_width,
            self.max_height,
            delta_x,
            delta_y,
            corner,
        );
        self.x = nx;
        self.y = ny;
        self.width = nw;
        self.height = nh;
    }
}
