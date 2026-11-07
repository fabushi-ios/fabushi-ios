//! Telegram Desktop 1:1 Parity Submodule: `VideoItemTimeline`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/video_item_timeline.h:L1-L51`, `Telegram/SourceFiles/editor/video_item_timeline.cpp:L1-L112`

use super::editor_trim_timeline::{EditorTrimTimeline, TrimTimelineDescriptor};
use serde::{Deserialize, Serialize};

/// Target symbol VideoItemTimeline managing video trim strip and volume menu.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoItemTimeline {
    pub timeline: Option<EditorTrimTimeline>,
    pub duration_ms: i64,
    pub from_ms: i64,
    pub till_ms: i64,
    pub volume: f64,
    pub has_audio: bool,
    pub width: i32,
}

impl Default for VideoItemTimeline {
    fn default() -> Self {
        Self {
            timeline: None,
            duration_ms: 0,
            from_ms: 0,
            till_ms: 0,
            volume: 1.0,
            has_audio: false,
            width: 422,
        }
    }
}

impl VideoItemTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_clip(
        &mut self,
        duration_ms: i64,
        from_ms: i64,
        till_ms: i64,
        volume: f64,
        has_audio: bool,
    ) {
        let dur = duration_ms.max(1);
        let from = from_ms.clamp(0, dur);
        let till = if till_ms > from { till_ms.min(dur) } else { dur };

        self.duration_ms = dur;
        self.from_ms = from;
        self.till_ms = till;
        self.volume = volume;
        self.has_audio = has_audio;

        self.timeline = Some(EditorTrimTimeline::new(TrimTimelineDescriptor {
            duration: dur,
            from,
            till,
            trim_only: true,
            ..Default::default()
        }));
    }

    pub fn refresh_trim(&mut self) {
        if let Some(ref mut tl) = self.timeline {
            tl.set_trim(self.from_ms, self.till_ms);
        }
    }

    pub fn commit_pending_edit(&mut self) {
        if let Some(ref mut tl) = self.timeline {
            self.from_ms = tl.from_ms;
            self.till_ms = tl.till_ms;
        }
    }

    pub fn loop_duration(&self) -> i64 {
        (self.till_ms - self.from_ms).max(0)
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Standard height in controls bar (48px).
    /// Upstream: `video_item_timeline.cpp:L102-L109`
    pub fn resize_get_height(&mut self, new_width: i32) -> i32 {
        self.width = new_width.max(1);
        48 // st::photoEditorButtonBarHeight
    }
}
