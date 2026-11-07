//! Telegram Desktop 1:1 Parity Submodule: `MediaViewOverlayRendererView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_overlay_renderer.h`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OverlayBackend {
    #[default]
    Raster,
    OpenGL,
    Rhi,
}

/// Abstract contract for lightbox overlay rendering
/// Upstream: `media_view_overlay_renderer.h:L18-L55`
pub trait OverlayRendererBackend {
    fn paint_background(&mut self, full_screen_video: bool);
    fn paint_content(&mut self, width: i32, height: i32, scale: f64, rotation: f64);
    fn paint_controls(&mut self, opacity: f64);
    fn paint_group_thumbs(&mut self, count: usize, opacity: f64);
    fn paint_caption(&mut self, opacity: f64);
}

/// Primary target symbol for `media_view_overlay_renderer.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewOverlayRendererView {
    pub backend: OverlayBackend,
    pub frame_index: u64,
    pub is_initialized: bool,
}

impl Default for MediaViewOverlayRendererView {
    fn default() -> Self {
        Self {
            backend: OverlayBackend::Raster,
            frame_index: 0,
            is_initialized: true,
        }
    }
}

impl MediaViewOverlayRendererView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backend(backend: OverlayBackend) -> Self {
        Self {
            backend,
            frame_index: 0,
            is_initialized: true,
        }
    }

    pub fn next_frame(&mut self) -> u64 {
        self.frame_index += 1;
        self.frame_index
    }
}
