//! Telegram Desktop 1:1 Parity Module: `VideoTimelineSeekerVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_timeline_seeker.cpp` (L1-L86)
//! - `Telegram/SourceFiles/editor/video/video_timeline_seeker.h` (L1-L39)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream seek throttle debounce in milliseconds: `video_timeline_seeker.cpp:L16`
pub const K_SEEK_THROTTLE_MS: u64 = 120;

/// Authentic representation of timeline scrubbing throttler state machine.
/// Upstream: `video_timeline_seeker.h:L17-L38` & `video_timeline_seeker.cpp:L20-L85`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTimelineSeekerVideo {
    pub current_position_ms: u64,
    pub pending_seek_ms: Option<u64>,
    pub is_dragging: bool,
    pub last_seek_time_ms: u64,
}

impl Default for VideoTimelineSeekerVideo {
    fn default() -> Self {
        Self {
            current_position_ms: 0,
            pending_seek_ms: None,
            is_dragging: false,
            last_seek_time_ms: 0,
        }
    }
}

impl VideoTimelineSeekerVideo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues or immediately executes a seek based on 120ms throttle threshold.
    /// Upstream: `video_timeline_seeker.cpp:L68-L85`
    pub fn seek(&mut self, target_ms: u64, now_ms: u64) -> Option<u64> {
        self.pending_seek_ms = Some(target_ms);
        if now_ms.saturating_sub(self.last_seek_time_ms) >= K_SEEK_THROTTLE_MS {
            self.last_seek_time_ms = now_ms;
            let executed = self.pending_seek_ms.take();
            if let Some(pos) = executed {
                self.current_position_ms = pos;
            }
            executed
        } else {
            None
        }
    }

    pub fn set_dragging(&mut self, dragging: bool) {
        self.is_dragging = dragging;
    }

    pub fn flush_pending_seek(&mut self) -> Option<u64> {
        let executed = self.pending_seek_ms.take();
        if let Some(pos) = executed {
            self.current_position_ms = pos;
        }
        executed
    }
}
