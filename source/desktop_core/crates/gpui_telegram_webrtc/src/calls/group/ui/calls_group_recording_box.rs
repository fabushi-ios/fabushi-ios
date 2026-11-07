//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupRecordingBoxGroupUi`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_recording_box.h`
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_recording_box.cpp:L26-L100`

use serde::{Deserialize, Serialize};

pub const K_ROUND_RADIUS: u32 = 9;
pub const K_MAX_GROUP_CALL_LENGTH: usize = 40;
pub const K_SWITCH_DURATION_MS: u64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingType {
    AudioOnly,
    VideoLandscape,
    VideoPortrait,
}

impl Default for RecordingType {
    fn default() -> Self {
        Self::AudioOnly
    }
}

/// Group Call Recording Dialog Controller
/// Target Symbol: `CallsGroupRecordingBoxGroupUi`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupRecordingBoxGroupUi {
    pub recording_type: RecordingType,
    pub call_title: String,
    pub is_recording: bool,
    pub recording_duration_sec: u64,
    pub is_livestream: bool,
}

impl Default for CallsGroupRecordingBoxGroupUi {
    fn default() -> Self {
        Self {
            recording_type: RecordingType::AudioOnly,
            call_title: String::new(),
            is_recording: false,
            recording_duration_sec: 0,
            is_livestream: false,
        }
    }
}

impl CallsGroupRecordingBoxGroupUi {
    pub fn new(initial_title: &str, is_livestream: bool) -> Self {
        let mut title = initial_title.to_string();
        if title.len() > K_MAX_GROUP_CALL_LENGTH {
            title.truncate(K_MAX_GROUP_CALL_LENGTH);
        }
        Self {
            recording_type: RecordingType::AudioOnly,
            call_title: title,
            is_recording: false,
            recording_duration_sec: 0,
            is_livestream,
        }
    }

    pub fn set_recording_type(&mut self, rec_type: RecordingType) {
        self.recording_type = rec_type;
    }

    pub fn set_title(&mut self, title: &str) {
        let mut trimmed = title.trim().to_string();
        if trimmed.len() > K_MAX_GROUP_CALL_LENGTH {
            trimmed.truncate(K_MAX_GROUP_CALL_LENGTH);
        }
        self.call_title = trimmed;
    }

    pub fn start_recording(&mut self) -> bool {
        if !self.is_recording {
            self.is_recording = true;
            self.recording_duration_sec = 0;
            true
        } else {
            false
        }
    }

    pub fn stop_recording(&mut self) -> u64 {
        self.is_recording = false;
        self.recording_duration_sec
    }

    pub fn tick_duration(&mut self) {
        if self.is_recording {
            self.recording_duration_sec = self.recording_duration_sec.saturating_add(1);
        }
    }
}
