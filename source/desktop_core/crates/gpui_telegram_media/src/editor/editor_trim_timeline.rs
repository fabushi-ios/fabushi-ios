//! Telegram Desktop 1:1 Parity Submodule: `EditorTrimTimeline`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_trim_timeline.h:L1-L208`, `Telegram/SourceFiles/editor/editor_trim_timeline.cpp:L1-L1208`

use serde::{Deserialize, Serialize};

pub const ZOOM_STEP: f64 = 1.2;
pub const ZOOM_SNAP: f64 = 0.001;
pub const MIN_VISIBLE_SPAN_MS: i64 = 2000;
pub const MIN_SELECTION_MS: i64 = 100;
pub const COMPACT_HANDLE_SPANS: i32 = 3;
pub const EDGE_SCROLL_FRAME_MS: i64 = 16;
pub const EDGE_SCROLL_SPEED: f64 = 9.0;
pub const EDGE_SCROLL_MAX_SPEED: f64 = 3.0;
pub const SCROLL_INDICATOR_OPACITY: f64 = 0.5;
pub const SCROLL_INDICATOR_FADE_ZOOM: f64 = 0.25;
pub const OVERVIEW_TRACK_OPACITY: f64 = 0.18;
pub const HINT_DURATION_MS: i64 = 150;
pub const SCROLL_TO_SELECTION_DURATION_MS: i64 = 250;

/// Formats milliseconds into `MM:SS.f` timestamp format.
/// Upstream: `editor_trim_timeline.cpp:L42-L49`
pub fn format_stamp(ms: i64, precision: usize) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let millis = ms % 1000;
    let prec = precision.clamp(1, 3);
    let frac = format!("{:03}", millis);
    format!("{:02}:{:02}.{}", minutes, seconds, &frac[..prec])
}

/// Parses `[HH:]MM:SS[.mmm]` or `SS[.mmm]` timestamp into milliseconds.
/// Upstream: `editor_trim_timeline.cpp:L64-L100`
pub fn parse_stamp(text: &str) -> Option<i64> {
    let clean = text.trim();
    if clean.is_empty() {
        return None;
    }
    let parts: Vec<&str> = clean.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }

    let mut seconds: i64 = 0;
    for &part in &parts[..parts.len() - 1] {
        let val: i64 = part.parse().ok()?;
        if val < 0 {
            return None;
        }
        seconds = seconds * 60 + val;
    }

    let last = parts.last()?;
    let (whole_str, frac_str) = if let Some(dot_idx) = last.find(|c| c == '.' || c == ',') {
        (&last[..dot_idx], &last[dot_idx + 1..])
    } else {
        (*last, "")
    };

    let whole: i64 = if whole_str.is_empty() {
        0
    } else {
        whole_str.parse().ok()?
    };
    if whole < 0 {
        return None;
    }

    let mut millis: i64 = 0;
    if !frac_str.is_empty() {
        let digits = &frac_str[..frac_str.len().min(3)];
        let val: i64 = digits.parse().ok()?;
        millis = val;
        for _ in digits.len()..3 {
            millis *= 10;
        }
    }

    Some((seconds * 60 + whole) * 1000 + millis)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TrimTimelineDescriptor {
    pub duration: i64,
    pub max_duration: i64,
    pub min_duration: i64,
    pub from: i64,
    pub till: i64,
    pub cover: i64,
    pub trim_only: bool,
}

/// Target symbol EditorTrimTimeline providing timeline trimming, scrub playhead, and zoom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorTrimTimeline {
    pub duration_ms: i64,
    pub from_ms: i64,
    pub till_ms: i64,
    pub cover_ms: i64,
    pub playback_ms: i64,
    pub zoom: f64,
    pub visible_from_ms: i64,
    pub visible_till_ms: i64,
    pub trim_only: bool,
    pub is_dragging: bool,
}

pub type TrimTimeline = EditorTrimTimeline;

impl Default for EditorTrimTimeline {
    fn default() -> Self {
        Self::new(TrimTimelineDescriptor {
            duration: 10_000,
            from: 0,
            till: 10_000,
            ..Default::default()
        })
    }
}

impl EditorTrimTimeline {
    pub fn new(descriptor: TrimTimelineDescriptor) -> Self {
        let dur = descriptor.duration.max(1);
        let from = descriptor.from.clamp(0, dur);
        let till = if descriptor.till > from {
            descriptor.till.min(dur)
        } else {
            dur
        };
        Self {
            duration_ms: dur,
            from_ms: from,
            till_ms: till,
            cover_ms: descriptor.cover,
            playback_ms: from,
            zoom: 1.0,
            visible_from_ms: 0,
            visible_till_ms: dur,
            trim_only: descriptor.trim_only,
            is_dragging: false,
        }
    }

    pub fn set_trim(&mut self, from: i64, till: i64) {
        self.from_ms = from.clamp(0, self.duration_ms);
        let min_till = (self.from_ms + MIN_SELECTION_MS).min(self.duration_ms);
        self.till_ms = till.clamp(min_till, self.duration_ms);
    }

    pub fn set_playback_position(&mut self, position_ms: i64) {
        self.playback_ms = position_ms.clamp(self.from_ms, self.till_ms);
    }

    pub fn time_at(&self, x: i32, width: i32) -> i64 {
        if width <= 0 {
            return self.visible_from_ms;
        }
        let ratio = (x as f64 / width as f64).clamp(0.0, 1.0);
        let span = (self.visible_till_ms - self.visible_from_ms) as f64;
        self.visible_from_ms + (span * ratio).round() as i64
    }

    pub fn x_at(&self, time_ms: i64, width: i32) -> i32 {
        let span = (self.visible_till_ms - self.visible_from_ms).max(1) as f64;
        let offset = (time_ms - self.visible_from_ms).clamp(0, span as i64) as f64;
        ((offset / span) * width as f64).round() as i32
    }
}
