//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPipRendererView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_pip_renderer.h`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PipBackendType {
    #[default]
    Raster,
    OpenGL,
    Rhi,
}

/// Abstract PiP renderer contract
/// Upstream: `media_view_pip_renderer.h:L15-L35`
pub trait PipRendererBackend {
    fn paint_video_frame(&mut self, width: i32, height: i32, rotation: i32);
    fn paint_controls(&mut self, is_playing: bool, volume: f64, opacity: f64);
    fn paint_loading(&mut self, progress: f64);
}

/// Primary target symbol for `media_view_pip_renderer.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPipRendererView {
    pub backend_type: PipBackendType,
    pub frame_counter: u64,
    pub is_initialized: bool,
}

impl Default for MediaViewPipRendererView {
    fn default() -> Self {
        Self {
            backend_type: PipBackendType::Raster,
            frame_counter: 0,
            is_initialized: true,
        }
    }
}

impl MediaViewPipRendererView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_backend(backend_type: PipBackendType) -> Self {
        Self {
            backend_type,
            frame_counter: 0,
            is_initialized: true,
        }
    }

    pub fn render_frame(&mut self) -> u64 {
        self.frame_counter += 1;
        self.frame_counter
    }
}
