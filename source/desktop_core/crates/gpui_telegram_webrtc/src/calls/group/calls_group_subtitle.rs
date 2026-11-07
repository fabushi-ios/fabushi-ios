//! Group Call Subtitle & Captions: `CallsGroupSubtitle`
//! Formats and lays out real-time speech captions in group calls.

use serde::{Deserialize, Serialize};

/// Caption segment from speech recognition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleSegment {
    pub speaker_id: u64,
    pub speaker_name: String,
    pub text: String,
    pub timestamp_ms: i64,
    pub duration_ms: u64,
}

/// Parity component for `CallsGroupSubtitle`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupSubtitle {
    pub active_captions: Vec<SubtitleSegment>,
    pub max_captions: usize,
    pub is_enabled: bool,
}

impl CallsGroupSubtitle {
    pub fn new() -> Self {
        Self {
            active_captions: Vec::new(),
            max_captions: 3,
            is_enabled: true,
        }
    }

    pub fn push_caption(&mut self, speaker_id: u64, speaker_name: &str, text: &str, duration_ms: u64) {
        if !self.is_enabled {
            return;
        }
        if self.active_captions.len() >= self.max_captions {
            self.active_captions.remove(0);
        }
        self.active_captions.push(SubtitleSegment {
            speaker_id,
            speaker_name: speaker_name.to_string(),
            text: text.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            duration_ms,
        });
    }

    pub fn purge_expired(&mut self, now_ms: i64) {
        self.active_captions.retain(|c| now_ms - c.timestamp_ms < c.duration_ms as i64);
    }

    pub fn current_display_text(&self) -> Option<String> {
        self.active_captions.last().map(|c| format!("{}: {}", c.speaker_name, c.text))
    }
}
