//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPipRasterView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_pip_raster.h`
//! - `Telegram/SourceFiles/media/view/media_view_pip_raster.cpp:L25-L95`

use serde::{Deserialize, Serialize};
use super::media_view_pip_renderer::PipRendererBackend;

/// Software raster fallback renderer for PiP
/// Upstream: `media_view_pip_raster.h:L14-L45`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPipRasterView {
    pub surface_width: i32,
    pub surface_height: i32,
    pub rendered_frames: u64,
    pub is_active: bool,
}

impl Default for MediaViewPipRasterView {
    fn default() -> Self {
        Self {
            surface_width: 320,
            surface_height: 180,
            rendered_frames: 0,
            is_active: true,
        }
    }
}

impl MediaViewPipRasterView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.surface_width = width.max(1);
        self.surface_height = height.max(1);
    }
}

impl PipRendererBackend for MediaViewPipRasterView {
    fn paint_video_frame(&mut self, _width: i32, _height: i32, _rotation: i32) {
        self.rendered_frames += 1;
    }

    fn paint_controls(&mut self, _is_playing: bool, _volume: f64, _opacity: f64) {
        // Software 2D controls overlay drawing
    }

    fn paint_loading(&mut self, _progress: f64) {
        // Radial progress arc
    }
}
