//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_native_frame_mac.h:L1-L23`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_native_frame_mac.mm:L1-L130`
//! Parity Module: `media_streaming_native_frame_mac` | Feature: `TCG-N52` (macOS Native NV12 CVPixelBuffer Decoder)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CvPixelFormat {
    BiPlanarVideoRange, // kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
    BiPlanarFullRange,  // kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    Unsupported,
}

/// Ledger Target Symbol: `MediaStreamingNativeFrameMacStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingNativeFrameMacStreaming {
    pub format: CvPixelFormat,
    pub width: u32,
    pub height: u32,
    pub y_stride: usize,
    pub uv_stride: usize,
    pub is_locked: bool,
    pub converted_frames_count: usize,
}

impl Default for MediaStreamingNativeFrameMacStreaming {
    fn default() -> Self {
        Self {
            format: CvPixelFormat::BiPlanarVideoRange,
            width: 0,
            height: 0,
            y_stride: 0,
            uv_stride: 0,
            is_locked: false,
            converted_frames_count: 0,
        }
    }
}

impl MediaStreamingNativeFrameMacStreaming {
    pub fn new(width: u32, height: u32, format: CvPixelFormat) -> Self {
        Self {
            format,
            width,
            height,
            y_stride: width as usize,
            uv_stride: width as usize,
            is_locked: false,
            converted_frames_count: 0,
        }
    }

    /// `media_streaming_native_frame_mac.mm:L52-L85`
    pub fn convert_nv12_to_argb(
        &mut self,
        y_plane: &[u8],
        uv_plane: &[u8],
    ) -> Option<Vec<u8>> {
        if self.width == 0 || self.height == 0 || self.format == CvPixelFormat::Unsupported {
            return None;
        }

        let pixel_count = (self.width * self.height) as usize;
        let mut argb = vec![0u8; pixel_count * 4];

        for row in 0..(self.height as usize) {
            for col in 0..(self.width as usize) {
                let y_idx = row * self.y_stride + col;
                let uv_idx = (row / 2) * self.uv_stride + (col / 2) * 2;

                if y_idx < y_plane.len() && uv_idx + 1 < uv_plane.len() {
                    let y = y_plane[y_idx] as f32;
                    let u = uv_plane[uv_idx] as f32 - 128.0;
                    let v = uv_plane[uv_idx + 1] as f32 - 128.0;

                    // Standard Rec.601 / Rec.709 conversion approximation
                    let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
                    let g = (y - 0.344136 * u - 0.714136 * v).clamp(0.0, 255.0) as u8;
                    let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;

                    let out_idx = (row * self.width as usize + col) * 4;
                    argb[out_idx] = b;
                    argb[out_idx + 1] = g;
                    argb[out_idx + 2] = r;
                    argb[out_idx + 3] = 255;
                }
            }
        }

        self.converted_frames_count += 1;
        Some(argb)
    }
}
