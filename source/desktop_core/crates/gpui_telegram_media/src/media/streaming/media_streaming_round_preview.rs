//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_round_preview.h:L1-L36`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_round_preview.cpp:L1-L63`
//! Parity Module: `media_streaming_round_preview` | Feature: `TCG-N52` (Round Video Message Preview Renderer)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundClipNotification {
    Reinit,
    Repaint,
}

/// Ledger Target Symbol: `MediaStreamingRoundPreviewStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingRoundPreviewStreaming {
    pub raw_bytes_len: usize,
    pub target_diameter: u32,
    pub device_pixel_ratio: f64,
    pub is_started: bool,
    pub has_error: bool,
    pub current_frame_index: usize,
    pub repaint_count: usize,
}

impl Default for MediaStreamingRoundPreviewStreaming {
    fn default() -> Self {
        Self {
            raw_bytes_len: 0,
            target_diameter: 240,
            device_pixel_ratio: 2.0,
            is_started: false,
            has_error: false,
            current_frame_index: 0,
            repaint_count: 0,
        }
    }
}

impl MediaStreamingRoundPreviewStreaming {
    pub fn new(raw_bytes: &[u8], diameter: u32) -> Self {
        Self {
            raw_bytes_len: raw_bytes.len(),
            target_diameter: diameter,
            device_pixel_ratio: 2.0,
            is_started: !raw_bytes.is_empty(),
            has_error: false,
            current_frame_index: 0,
            repaint_count: 0,
        }
    }

    /// `media_streaming_round_preview.cpp:L40-L60`
    pub fn handle_notification(&mut self, notification: RoundClipNotification) {
        match notification {
            RoundClipNotification::Reinit => {
                if self.raw_bytes_len == 0 {
                    self.has_error = true;
                    self.is_started = false;
                } else {
                    self.is_started = true;
                }
            }
            RoundClipNotification::Repaint => {
                self.repaint_count += 1;
                self.current_frame_index += 1;
            }
        }
    }

    pub fn physical_size(&self) -> (u32, u32) {
        let px = (self.target_diameter as f64 * self.device_pixel_ratio).round() as u32;
        (px, px)
    }
}
