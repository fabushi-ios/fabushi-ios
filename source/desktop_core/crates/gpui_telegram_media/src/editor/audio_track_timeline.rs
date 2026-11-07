//! Telegram Desktop 1:1 Parity Submodule: `AudioTrackTimeline`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/audio_track_timeline.h:L1-L43`, `Telegram/SourceFiles/editor/audio_track_timeline.cpp:L1-L81`

use super::editor_audio_timeline::EditorAudioTimeline;
use super::photo_editor_common::AudioTrack;
use serde::{Deserialize, Serialize};

/// Target symbol AudioTrackTimeline managing audio track embedding and trim sync in controls bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrackTimeline {
    pub timeline: Option<EditorAudioTimeline>,
    pub track: Option<AudioTrack>,
    pub is_active: bool,
    pub width: i32,
}

impl Default for AudioTrackTimeline {
    fn default() -> Self {
        Self {
            timeline: None,
            track: None,
            is_active: false,
            width: 422,
        }
    }
}

impl AudioTrackTimeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_track(&mut self, track: Option<AudioTrack>) {
        if let Some(t) = track {
            self.timeline = Some(EditorAudioTimeline::new(t.clone()));
            self.track = Some(t);
            self.is_active = true;
        } else {
            self.timeline = None;
            self.track = None;
            self.is_active = false;
        }
    }

    pub fn refresh_trim(&mut self) {
        if let Some(ref mut tl) = self.timeline {
            tl.refresh_trim();
        }
    }

    pub fn commit_pending_edit(&mut self) {
        if let Some(ref mut tl) = self.timeline {
            tl.commit_pending_edit();
            self.track = Some(tl.track.clone());
        }
    }

    pub fn effective_length(&self) -> i64 {
        self.track.as_ref().map(|t| t.length()).unwrap_or(0)
    }

    /// Standard timeline height in the bottom buttons strip (48px).
    /// Upstream: `audio_track_timeline.cpp:L71-L78`
    pub fn resize_get_height(&mut self, new_width: i32) -> i32 {
        self.width = new_width.max(1);
        48 // st::photoEditorButtonBarHeight
    }
}
