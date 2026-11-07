//! Target Symbol: DataSavedMusic
//! Telegram Desktop 1:1 Submodule: `data/data_saved_music`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMusicTrack {
    pub document_id: u64,
    pub title: String,
    pub performer: String,
    pub duration_seconds: u32,
    pub message_id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSavedMusic {
    pub tracks: Vec<SavedMusicTrack>,
    pub current_track_index: Option<usize>,
    pub is_shuffled: bool,
    pub is_repeating: bool,
}

impl DataSavedMusic {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_track(&mut self, track: SavedMusicTrack) {
        if !self.tracks.iter().any(|t| t.document_id == track.document_id) {
            self.tracks.push(track);
        }
    }

    pub fn remove_track(&mut self, document_id: u64) {
        self.tracks.retain(|t| t.document_id != document_id);
        if let Some(cur) = self.current_track_index {
            if cur >= self.tracks.len() {
                self.current_track_index = if self.tracks.is_empty() { None } else { Some(0) };
            }
        }
    }

    pub fn current_track(&self) -> Option<&SavedMusicTrack> {
        self.current_track_index.and_then(|idx| self.tracks.get(idx))
    }

    pub fn next_track(&mut self) -> Option<&SavedMusicTrack> {
        if self.tracks.is_empty() {
            return None;
        }
        let next_idx = match self.current_track_index {
            Some(idx) => {
                if idx + 1 < self.tracks.len() {
                    idx + 1
                } else if self.is_repeating {
                    0
                } else {
                    return None;
                }
            }
            None => 0,
        };
        self.current_track_index = Some(next_idx);
        self.tracks.get(next_idx)
    }

    pub fn previous_track(&mut self) -> Option<&SavedMusicTrack> {
        if self.tracks.is_empty() {
            return None;
        }
        let prev_idx = match self.current_track_index {
            Some(idx) => {
                if idx > 0 {
                    idx - 1
                } else if self.is_repeating {
                    self.tracks.len() - 1
                } else {
                    0
                }
            }
            None => 0,
        };
        self.current_track_index = Some(prev_idx);
        self.tracks.get(prev_idx)
    }
}
