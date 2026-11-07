//! Telegram Desktop 1:1 Parity Submodule: `MediaViewOverlayWidgetView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_overlay_widget.h`
//! - `Telegram/SourceFiles/media/view/media_view_overlay_widget.cpp:L186-L210, L2923-L2999, L7533-L7580, L7619-L7673, L8093-L8107`

use serde::{Deserialize, Serialize};

// Anonymous namespace constants from media_view_overlay_widget.cpp:L186-L205
pub const K_PRELOAD_COUNT: usize = 3;
pub const K_MAX_ZOOM_LEVEL: i32 = 7; // up to x8 scale
pub const K_ZOOM_TO_SCREEN_LEVEL: i32 = 1024;
pub const K_PINCH_ZOOM_STEP: f64 = 0.25;
pub const K_OVERLAY_LOADER_PRIORITY: i32 = 2;
pub const K_SEEK_TIME_MS: i64 = 5000;
pub const K_SEEK_TIME_MS_LONG: i64 = 10000;
pub const K_FRAME_STEP_FALLBACK_FPS: f64 = 30.0;
pub const K_FRAME_STEP_THROTTLE_MS: i64 = 150;
pub const K_MAX_DISPLAY_IMAGE_SIZE: i32 = 4096;
pub const K_IDS_LIMIT: usize = 48;
pub const K_IDS_PRELOAD_AFTER: usize = 28;

// Style constants from media_view.style:L279-L303
pub const MEDIAVIEW_WAIT_HIDE_MS: u64 = 1100;
pub const MEDIAVIEW_HIDE_DURATION_MS: u64 = 600;
pub const MEDIAVIEW_SHOW_DURATION_MS: u64 = 200;
pub const MEDIAVIEW_FADE_DURATION_MS: u64 = 150;
pub const MEDIAVIEW_SWIPE_DISTANCE: i32 = 80;
pub const MEDIAVIEW_DEFAULT_WIDTH: i32 = 800;
pub const MEDIAVIEW_DEFAULT_HEIGHT: i32 = 600;

/// Controls visibility state machine
/// Upstream: `media_view_overlay_widget.h:L808-L814`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ControlsState {
    ControlsShowing,
    #[default]
    ControlsShown,
    ControlsHiding,
    ControlsHidden,
}

/// Interactive control regions under cursor
/// Upstream: `media_view_overlay_widget.h:L156-L175`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OverControl {
    #[default]
    None,
    Left,
    Right,
    LeftStories,
    RightStories,
    SponsoredButton,
    Header,
    Name,
    Date,
    Save,
    Share,
    Rotate,
    More,
    Draw,
    Recognize,
    Icon,
    Video,
    Caption,
}

/// Geometry and transform matrix for current media content
/// Upstream: `media_view_overlay_widget.h:L192-L203`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContentGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub rotation: f64,
    pub controls_opacity: f64,
    pub fade: f64,
    pub scale: f64,
}

impl Default for ContentGeometry {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: MEDIAVIEW_DEFAULT_WIDTH,
            height: MEDIAVIEW_DEFAULT_HEIGHT,
            rotation: 0.0,
            controls_opacity: 1.0,
            fade: 0.0,
            scale: 1.0,
        }
    }
}

/// Clamps and centers content coordinates within viewport
/// Upstream: `media_view_overlay_widget.cpp:L8093-L8107`
pub fn snap_xy(
    viewport_width: i32,
    viewport_height: i32,
    content_w: i32,
    content_h: i32,
    skip_top: i32,
    available_height: i32,
    min_used_top: i32,
    max_used_height: i32,
    full_screen_video: bool,
    mut x: i32,
    mut y: i32,
) -> (i32, i32) {
    let mut xmin = viewport_width - content_w;
    let mut xmax = 0;
    let mut ymin = viewport_height - content_h;
    let mut ymax = min_used_top;

    let center_x = (viewport_width - content_w) / 2;
    xmin = xmin.min(center_x);
    xmax = xmax.max(center_x);

    let center_y = if full_screen_video {
        min_used_top + (max_used_height - content_h) / 2
    } else {
        skip_top + (available_height - content_h) / 2
    };
    ymin = ymin.min(center_y);
    ymax = ymax.max(center_y);

    x = x.clamp(xmin, xmax);
    y = y.clamp(ymin, ymax);
    (x, y)
}

/// Primary target symbol for `media_view_overlay_widget.rs`
#[derive(Debug, Clone)]
pub struct MediaViewOverlayWidgetView {
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub available_height: i32,
    pub skip_top: i32,
    pub base_w: i32,
    pub base_h: i32,
    pub content_w: i32,
    pub content_h: i32,
    pub x: i32,
    pub y: i32,
    pub zoom: i32,
    pub zoom_to_screen: f64,
    pub zoom_to_default: f64,
    pub rotation: i32,
    pub controls_state: ControlsState,
    pub controls_opacity: f64,
    pub over: OverControl,
    pub swipe_navigating: bool,
    pub current_index: usize,
    pub total_items: usize,
    pub is_full_screen: bool,
}

impl Default for MediaViewOverlayWidgetView {
    fn default() -> Self {
        Self {
            viewport_width: 1280,
            viewport_height: 800,
            available_height: 800,
            skip_top: 0,
            base_w: MEDIAVIEW_DEFAULT_WIDTH,
            base_h: MEDIAVIEW_DEFAULT_HEIGHT,
            content_w: MEDIAVIEW_DEFAULT_WIDTH,
            content_h: MEDIAVIEW_DEFAULT_HEIGHT,
            x: (1280 - MEDIAVIEW_DEFAULT_WIDTH) / 2,
            y: (800 - MEDIAVIEW_DEFAULT_HEIGHT) / 2,
            zoom: 0,
            zoom_to_screen: 1.0,
            zoom_to_default: 1.0,
            rotation: 0,
            controls_state: ControlsState::ControlsShown,
            controls_opacity: 1.0,
            over: OverControl::None,
            swipe_navigating: false,
            current_index: 0,
            total_items: 1,
            is_full_screen: false,
        }
    }
}

impl MediaViewOverlayWidgetView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets zoom level around optional anchor point
    /// Upstream: `media_view_overlay_widget.cpp:L7619-L7673`
    pub fn set_zoom_level(&mut self, new_zoom: i32, anchor: Option<(i32, i32)>) {
        if self.zoom == new_zoom {
            return;
        }

        let full = if self.is_full_screen {
            self.zoom_to_screen
        } else {
            self.zoom_to_default
        };
        let z = if self.zoom == K_ZOOM_TO_SCREEN_LEVEL {
            full
        } else {
            self.zoom as f64
        };

        let (anchor_x, anchor_y) = anchor
            .map(|(ax, ay)| (ax as f64, ay as f64))
            .unwrap_or((self.viewport_width as f64 / 2.0, self.available_height as f64 / 2.0));

        let (nx, ny) = if z >= 0.0 {
            (
                (self.x as f64 - anchor_x) / (z + 1.0),
                (self.y as f64 - anchor_y) / (z + 1.0),
            )
        } else {
            (
                (self.x as f64 - anchor_x) * (-z + 1.0),
                (self.y as f64 - anchor_y) * (-z + 1.0),
            )
        };

        self.zoom = new_zoom;
        let z_new = if self.zoom == K_ZOOM_TO_SCREEN_LEVEL {
            full
        } else {
            self.zoom as f64
        };

        let base_w = self.base_w as f64;
        let base_h = self.base_h as f64;
        if z_new > 0.0 {
            self.content_w = (base_w * (z_new + 1.0)).round() as i32;
            self.content_h = (base_h * (z_new + 1.0)).round() as i32;
            self.x = (nx * (z_new + 1.0) + anchor_x).round() as i32;
            self.y = (ny * (z_new + 1.0) + anchor_y).round() as i32;
        } else {
            self.content_w = (base_w / (-z_new + 1.0)).round() as i32;
            self.content_h = (base_h / (-z_new + 1.0)).round() as i32;
            self.x = (nx / (-z_new + 1.0) + anchor_x).round() as i32;
            self.y = (ny / (-z_new + 1.0) + anchor_y).round() as i32;
        }

        self.snap();
    }

    /// Zoom in step
    /// Upstream: `media_view_overlay_widget.cpp:L2923-L2938`
    pub fn zoom_in(&mut self, anchor: Option<(i32, i32)>) {
        let mut new_zoom = self.zoom;
        let full = if self.is_full_screen {
            self.zoom_to_screen
        } else {
            self.zoom_to_default
        };

        if new_zoom == K_ZOOM_TO_SCREEN_LEVEL {
            if (full.ceil() as i32) <= K_MAX_ZOOM_LEVEL {
                new_zoom = full.ceil() as i32;
            }
        } else if (new_zoom as f64) < full && (new_zoom as f64 + 1.0 > full || (full > K_MAX_ZOOM_LEVEL as f64 && new_zoom == K_MAX_ZOOM_LEVEL)) {
            new_zoom = K_ZOOM_TO_SCREEN_LEVEL;
        } else if new_zoom < K_MAX_ZOOM_LEVEL {
            new_zoom += 1;
        }

        self.set_zoom_level(new_zoom, anchor);
    }

    /// Zoom out step
    /// Upstream: `media_view_overlay_widget.cpp:L2940-L2955`
    pub fn zoom_out(&mut self, anchor: Option<(i32, i32)>) {
        let mut new_zoom = self.zoom;
        let full = if self.is_full_screen {
            self.zoom_to_screen
        } else {
            self.zoom_to_default
        };

        if new_zoom == K_ZOOM_TO_SCREEN_LEVEL {
            if (full.floor() as i32) >= -K_MAX_ZOOM_LEVEL {
                new_zoom = full.floor() as i32;
            }
        } else if (new_zoom as f64) > full && (new_zoom as f64 - 1.0 < full || (full < -K_MAX_ZOOM_LEVEL as f64 && new_zoom == -K_MAX_ZOOM_LEVEL)) {
            new_zoom = K_ZOOM_TO_SCREEN_LEVEL;
        } else if new_zoom > -K_MAX_ZOOM_LEVEL {
            new_zoom -= 1;
        }

        self.set_zoom_level(new_zoom, anchor);
    }

    /// Snaps current (x, y) coordinates
    pub fn snap(&mut self) {
        let (sx, sy) = snap_xy(
            self.viewport_width,
            self.viewport_height,
            self.content_w,
            self.content_h,
            self.skip_top,
            self.available_height,
            0,
            self.viewport_height,
            self.is_full_screen,
            self.x,
            self.y,
        );
        self.x = sx;
        self.y = sy;
    }

    /// Handles swipe gesture navigation
    /// Upstream: `media_view_overlay_widget.cpp:L7533-L7580`
    pub fn handle_swipe(&mut self, delta_x: i32) -> Option<bool> {
        if delta_x.abs() >= MEDIAVIEW_SWIPE_DISTANCE {
            if delta_x > 0 && self.current_index + 1 < self.total_items {
                self.current_index += 1;
                Some(true) // navigated next
            } else if delta_x < 0 && self.current_index > 0 {
                self.current_index -= 1;
                Some(false) // navigated previous
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Shows overlay controls
    pub fn activate_controls(&mut self) {
        self.controls_state = ControlsState::ControlsShown;
        self.controls_opacity = 1.0;
    }

    /// Hides overlay controls
    pub fn hide_controls(&mut self) {
        self.controls_state = ControlsState::ControlsHidden;
        self.controls_opacity = 0.0;
    }
}
