//! Telegram Desktop 1:1 Parity Submodule: `MediaViewMetalTextureView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_metal_texture.h`
//! - `Telegram/SourceFiles/media/view/media_view_metal_texture.mm:L49-L53, L89-L100`

use serde::{Deserialize, Serialize};

/// Supported pixel buffer format constants from macOS CoreVideo
/// Upstream: `media_view_metal_texture.mm:L49-L53`
pub const K_CV_PIXEL_FORMAT_420_VIDEO_RANGE: u32 = 0x34323076; // '420v'
pub const K_CV_PIXEL_FORMAT_420_FULL_RANGE: u32 = 0x34323066;  // '420f'

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetalPixelFormat {
    BiPlanarVideoRange,
    BiPlanarFullRange,
    Unsupported,
}

impl MetalPixelFormat {
    pub fn from_fourcc(fourcc: u32) -> Self {
        match fourcc {
            K_CV_PIXEL_FORMAT_420_VIDEO_RANGE => Self::BiPlanarVideoRange,
            K_CV_PIXEL_FORMAT_420_FULL_RANGE => Self::BiPlanarFullRange,
            _ => Self::Unsupported,
        }
    }
}

/// Primary target symbol for `media_view_metal_texture.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewMetalTextureView {
    pub format: MetalPixelFormat,
    pub width: usize,
    pub height: usize,
    pub has_y_texture: bool,
    pub has_uv_texture: bool,
    pub is_flushed: bool,
}

impl Default for MediaViewMetalTextureView {
    fn default() -> Self {
        Self {
            format: MetalPixelFormat::BiPlanarVideoRange,
            width: 0,
            height: 0,
            has_y_texture: false,
            has_uv_texture: false,
            is_flushed: true,
        }
    }
}

impl MediaViewMetalTextureView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulates texture creation from a CVPixelBuffer
    /// Upstream: `media_view_metal_texture.mm:L37-L100`
    pub fn create_textures_from_pixel_buffer(
        &mut self,
        fourcc: u32,
        width: usize,
        height: usize,
    ) -> bool {
        let fmt = MetalPixelFormat::from_fourcc(fourcc);
        if fmt == MetalPixelFormat::Unsupported {
            return false;
        }

        self.format = fmt;
        self.width = width;
        self.height = height;
        self.has_y_texture = true;
        self.has_uv_texture = true;
        self.is_flushed = false;
        true
    }

    /// Releases texture caches
    pub fn flush(&mut self) {
        self.has_y_texture = false;
        self.has_uv_texture = false;
        self.is_flushed = true;
    }
}
