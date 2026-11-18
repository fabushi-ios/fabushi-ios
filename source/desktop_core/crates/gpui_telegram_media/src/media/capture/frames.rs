//! Frame Data Models for Media Audio, Video, and Screen Capture Streams

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: i64,
    pub sequence: u64,
}

impl AudioFrame {
    pub fn new(samples: Vec<i16>, sample_rate: u32, channels: u16, sequence: u64) -> Self {
        Self {
            samples,
            sample_rate,
            channels,
            timestamp_us: chrono::Utc::now().timestamp_micros(),
            sequence,
        }
    }

    pub fn duration_ms(&self) -> i64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0;
        }
        (self.samples.len() as i64 * 1000) / (self.sample_rate as i64 * self.channels as i64)
    }

    pub fn rms_level(&self) -> u16 {
        crate::media::audio::wav::compute_pcm_rms_level(&self.samples)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoPixelFormat {
    Rgba8,
    Bgra8,
    I420,
    Nv12,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub format: VideoPixelFormat,
    pub data: Vec<u8>,
    pub stride_y: usize,
    pub stride_uv: usize,
    pub timestamp_us: i64,
    pub sequence: u64,
}

impl VideoFrame {
    pub fn new_rgba8(width: u32, height: u32, data: Vec<u8>, sequence: u64) -> Self {
        let stride = (width as usize).saturating_mul(4);
        Self {
            width,
            height,
            format: VideoPixelFormat::Rgba8,
            data,
            stride_y: stride,
            stride_uv: 0,
            timestamp_us: chrono::Utc::now().timestamp_micros(),
            sequence,
        }
    }

    /// Converts frame to RGBA8 format. Returns an empty Vec if the input buffer is truncated or overflows.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let Some(expected_rgba) = w.checked_mul(h).and_then(|n| n.checked_mul(4)) else {
            return Vec::new();
        };
        if expected_rgba == 0 {
            return Vec::new();
        }

        match self.format {
            VideoPixelFormat::Rgba8 => {
                if self.data.len() < expected_rgba {
                    return Vec::new();
                }
                self.data[..expected_rgba].to_vec()
            }
            VideoPixelFormat::Bgra8 => {
                if self.data.len() < expected_rgba {
                    return Vec::new();
                }
                let mut rgba = self.data[..expected_rgba].to_vec();
                for chunk in rgba.chunks_exact_mut(4) {
                    chunk.swap(0, 2); // B <-> R
                }
                rgba
            }
            VideoPixelFormat::I420 => {
                let uv_w = w.div_ceil(2);
                let uv_h = h.div_ceil(2);
                let Some(y_size) = w.checked_mul(h) else {
                    return Vec::new();
                };
                let Some(u_size) = uv_w.checked_mul(uv_h) else {
                    return Vec::new();
                };
                let Some(total_i420) = u_size.checked_mul(2).and_then(|uv| y_size.checked_add(uv))
                else {
                    return Vec::new();
                };
                if self.data.len() < total_i420 {
                    return Vec::new();
                }
                let mut rgba = vec![0u8; expected_rgba];
                let y_plane = &self.data[0..y_size];
                let u_plane = &self.data[y_size..y_size + u_size];
                let v_plane = &self.data[y_size + u_size..y_size + 2 * u_size];
                for row in 0..h {
                    for col in 0..w {
                        let y = y_plane[row * w + col] as f32;
                        let uv_idx = (row / 2) * uv_w + (col / 2);
                        let u = u_plane[uv_idx] as f32 - 128.0;
                        let v = v_plane[uv_idx] as f32 - 128.0;
                        let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
                        let g = (y - 0.344136 * u - 0.714136 * v).clamp(0.0, 255.0) as u8;
                        let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
                        let idx = (row * w + col) * 4;
                        rgba[idx] = r;
                        rgba[idx + 1] = g;
                        rgba[idx + 2] = b;
                        rgba[idx + 3] = 255;
                    }
                }
                rgba
            }
            VideoPixelFormat::Nv12 => {
                let uv_w = w.div_ceil(2);
                let uv_h = h.div_ceil(2);
                let Some(uv_stride) = uv_w.checked_mul(2) else {
                    return Vec::new();
                };
                let Some(y_size) = w.checked_mul(h) else {
                    return Vec::new();
                };
                let Some(uv_size) = uv_stride.checked_mul(uv_h) else {
                    return Vec::new();
                };
                let Some(total_nv12) = y_size.checked_add(uv_size) else {
                    return Vec::new();
                };
                if self.data.len() < total_nv12 {
                    return Vec::new();
                }
                let mut rgba = vec![0u8; expected_rgba];
                let y_plane = &self.data[0..y_size];
                let uv_plane = &self.data[y_size..y_size + uv_size];
                for row in 0..h {
                    for col in 0..w {
                        let y = y_plane[row * w + col] as f32;
                        let uv_idx = (row / 2) * uv_stride + (col / 2) * 2;
                        let u = uv_plane[uv_idx] as f32 - 128.0;
                        let v = uv_plane[uv_idx + 1] as f32 - 128.0;
                        let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
                        let g = (y - 0.344136 * u - 0.714136 * v).clamp(0.0, 255.0) as u8;
                        let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;
                        let idx = (row * w + col) * 4;
                        rgba[idx] = r;
                        rgba[idx + 1] = g;
                        rgba[idx + 2] = b;
                        rgba[idx + 3] = 255;
                    }
                }
                rgba
            }
        }
    }

    /// Converts RGBA8 to I420 YUV420p planar buffer for WebRTC RTP video encoding.
    /// Supports odd dimensions via ceiling division and returns an empty Vec on invalid/truncated buffers.
    pub fn to_i420(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let Some(expected_rgba) = w.checked_mul(h).and_then(|n| n.checked_mul(4)) else {
            return Vec::new();
        };
        if expected_rgba == 0 {
            return Vec::new();
        }
        let rgba = self.to_rgba8();
        if rgba.len() < expected_rgba {
            return Vec::new();
        }
        let uv_w = w.div_ceil(2);
        let uv_h = h.div_ceil(2);
        let Some(y_len) = w.checked_mul(h) else {
            return Vec::new();
        };
        let Some(uv_len) = uv_w.checked_mul(uv_h) else {
            return Vec::new();
        };
        let Some(total_len) = uv_len.checked_mul(2).and_then(|uv| y_len.checked_add(uv)) else {
            return Vec::new();
        };
        let mut out = vec![0u8; total_len];
        for row in 0..h {
            for col in 0..w {
                let idx = (row * w + col) * 4;
                let r = rgba[idx] as f32;
                let g = rgba[idx + 1] as f32;
                let b = rgba[idx + 2] as f32;
                let y = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
                out[row * w + col] = y;
                if row % 2 == 0 && col % 2 == 0 {
                    let u = (-0.169 * r - 0.331 * g + 0.500 * b + 128.0).clamp(0.0, 255.0) as u8;
                    let v = (0.500 * r - 0.419 * g - 0.081 * b + 128.0).clamp(0.0, 255.0) as u8;
                    let uv_idx = (row / 2) * uv_w + (col / 2);
                    out[y_len + uv_idx] = u;
                    out[y_len + uv_len + uv_idx] = v;
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrame {
    pub video_frame: VideoFrame,
    pub display_id: u32,
    pub cursor_pos: Option<(u32, u32)>,
}
