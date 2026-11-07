//! Telegram Desktop 1:1 Parity Submodule: `EditorMessageVideo`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_message_video.h:L1-L58`, `Telegram/SourceFiles/editor/editor_message_video.cpp:L1-L186`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoClipSource {
    pub path: String,
    pub content: Vec<u8>,
    pub duration_ms: i64,
    pub width: u32,
    pub height: u32,
}

/// Target symbol EditorMessageVideo handling video download and clip preparation for messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorMessageVideo {
    pub document_id: i64,
    pub message_id: i64,
    pub is_loading: bool,
    pub is_ready: bool,
    pub is_failed: bool,
    pub progress: f64,
    pub source: Option<VideoClipSource>,
}

pub type MessageVideo = EditorMessageVideo;

impl Default for EditorMessageVideo {
    fn default() -> Self {
        Self {
            document_id: 0,
            message_id: 0,
            is_loading: false,
            is_ready: false,
            is_failed: false,
            progress: 0.0,
            source: None,
        }
    }
}

impl EditorMessageVideo {
    pub fn new(document_id: i64, message_id: i64) -> Self {
        Self {
            document_id,
            message_id,
            ..Default::default()
        }
    }

    pub fn start_loading(&mut self) {
        self.is_loading = true;
        self.is_failed = false;
        self.progress = 0.0;
    }

    pub fn set_progress(&mut self, progress: f64) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    pub fn finish_loading(&mut self, source: VideoClipSource) {
        self.source = Some(source);
        self.is_loading = false;
        self.is_ready = true;
        self.progress = 1.0;
    }

    pub fn fail(&mut self) {
        self.is_loading = false;
        self.is_failed = true;
    }
}
