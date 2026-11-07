//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_utility.h:L1-L100`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_utility.cpp:L1-L457`
//! Parity Module: `media_streaming_utility` | Feature: `TCG-N52` (Streaming Sizing & Frame Utilities)

use super::media_streaming_common::{K_DURATION_UNAVAILABLE, K_TIME_UNKNOWN};
use serde::{Deserialize, Serialize};

/// `media_streaming_utility.cpp:L19`
pub const K_SKIP_INVALID_DATA_PACKETS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimePoint {
    pub track_time: i64,
    pub world_time: i64,
}

impl Default for TimePoint {
    fn default() -> Self {
        Self {
            track_time: K_TIME_UNKNOWN,
            world_time: K_TIME_UNKNOWN,
        }
    }
}

impl TimePoint {
    pub fn is_valid(&self) -> bool {
        self.track_time != K_TIME_UNKNOWN && self.world_time != K_TIME_UNKNOWN
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandDecision {
    pub result: (u32, u32),
    pub expanding: bool,
}

/// `media_streaming_utility.cpp:L371-L387`
pub fn decide_frame_resize(
    outer: (u32, u32),
    original: (u32, u32),
    min_visible_num: u32,
    min_visible_den: u32,
) -> ExpandDecision {
    if outer.0 == 0 || outer.1 == 0 || original.0 == 0 || original.1 == 0 {
        return ExpandDecision {
            result: original,
            expanding: true,
        };
    }

    // Qt::KeepAspectRatioByExpanding calculation
    let scale_x = outer.0 as f64 / original.0 as f64;
    let scale_y = outer.1 as f64 / original.1 as f64;
    let scale_big = scale_x.max(scale_y);
    let big_w = (original.0 as f64 * scale_big).round() as u32;
    let big_h = (original.1 as f64 * scale_big).round() as u32;

    if big_w <= outer.0 && (big_h * min_visible_num <= outer.1 * min_visible_den) {
        return ExpandDecision {
            result: (big_w, big_h),
            expanding: true,
        };
    }

    // Qt::KeepAspectRatio fallback
    let scale_fit = scale_x.min(scale_y);
    let fit_w = (original.0 as f64 * scale_fit).round().max(1.0) as u32;
    let fit_h = (original.1 as f64 * scale_fit).round().max(1.0) as u32;

    ExpandDecision {
        result: (fit_w, fit_h),
        expanding: false,
    }
}

/// `media_streaming_utility.cpp:L413-L415`
pub fn decide_video_frame_resize(outer: (u32, u32), original: (u32, u32)) -> ExpandDecision {
    decide_frame_resize(outer, original, 1, 2)
}

/// `media_streaming_utility.cpp:L417-L419`
pub fn calculate_resize_from_outer(outer: (u32, u32), original: (u32, u32)) -> (u32, u32) {
    decide_video_frame_resize(outer, original).result
}

/// `media_streaming_utility.cpp:L23-L37`
pub fn frame_position_from_pts(
    pts: i64,
    time_base_num: i32,
    time_base_den: i32,
    duration: i64,
) -> i64 {
    if pts < 0 || time_base_den <= 0 {
        return K_TIME_UNKNOWN;
    }
    let calculated = (pts.saturating_mul(1000).saturating_mul(time_base_num as i64))
        / (time_base_den as i64);

    if duration == K_DURATION_UNAVAILABLE {
        calculated
    } else {
        calculated.min(duration)
    }
}

/// Ledger Target Symbol: `MediaStreamingUtilityStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingUtilityStreaming {
    pub max_skip_invalid_packets: usize,
    pub default_min_visible_num: u32,
    pub default_min_visible_den: u32,
}

impl Default for MediaStreamingUtilityStreaming {
    fn default() -> Self {
        Self {
            max_skip_invalid_packets: K_SKIP_INVALID_DATA_PACKETS,
            default_min_visible_num: 1,
            default_min_visible_den: 2,
        }
    }
}

impl MediaStreamingUtilityStreaming {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resize_video(&self, outer: (u32, u32), original: (u32, u32)) -> ExpandDecision {
        decide_video_frame_resize(outer, original)
    }

    pub fn compute_pts(&self, pts: i64, num: i32, den: i32, duration: i64) -> i64 {
        frame_position_from_pts(pts, num, den, duration)
    }
}
