//! Telegram Desktop 1:1 Parity Submodule: `media_player_instance.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Global Media Player Coordinator)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_instance.h:L74-L140`
//!           `Telegram/SourceFiles/media/player/media_player_instance.cpp:L47-L76`

use serde::{Deserialize, Serialize};

/// Upstream: `media_player_instance.cpp:L48-L57`
pub const IDS_LIMIT: usize = 32;
pub const IDS_PRELOAD_AFTER: usize = 28;
pub const SHUFFLE_PLAYLIST_LIMIT: usize = 10000;
pub const REMEMBER_SHUFFLED_ORDER_ITEMS: usize = 16;
pub const MIN_LENGTH_FOR_SAVE_POSITION_VIDEO_SECS: i64 = 60;
pub const MIN_LENGTH_FOR_SAVE_POSITION_MUSIC_SECS: i64 = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerRepeatMode {
    #[default]
    None,
    One,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerOrderMode {
    #[default]
    Default,
    Reverse,
    Shuffle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaType {
    #[default]
    Voice,
    Song,
    Video,
}

/// Upstream: `media_player_instance.h:L58-L61`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlaylistContext {
    pub topic_root_id: i64,
    pub peer_id: u64,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerInstancePlayer {
    pub active_type: MediaType,
    pub current_track_id: u64,
    pub is_playing: bool,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub speed: f64,
    pub volume: f64,
    pub repeat_mode: PlayerRepeatMode,
    pub order_mode: PlayerOrderMode,
    pub playlist: Vec<u64>,
    pub playlist_index: usize,
    pub context: PlaylistContext,
}

impl MediaPlayerInstancePlayer {
    pub fn new() -> Self {
        Self {
            active_type: MediaType::Song,
            current_track_id: 0,
            is_playing: false,
            position_ms: 0,
            duration_ms: 0,
            speed: 1.0,
            volume: 1.0,
            repeat_mode: PlayerRepeatMode::None,
            order_mode: PlayerOrderMode::Default,
            playlist: Vec::new(),
            playlist_index: 0,
            context: PlaylistContext::default(),
        }
    }

    pub fn play(&mut self, track_id: u64, duration_ms: i64) {
        self.current_track_id = track_id;
        self.duration_ms = duration_ms;
        self.is_playing = true;
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn toggle_play_pause(&mut self) {
        self.is_playing = !self.is_playing;
    }

    pub fn stop(&mut self) {
        self.is_playing = false;
        self.position_ms = 0;
    }

    pub fn next(&mut self) -> bool {
        if self.playlist.is_empty() {
            return false;
        }
        match self.repeat_mode {
            PlayerRepeatMode::One => true,
            PlayerRepeatMode::All => {
                self.playlist_index = (self.playlist_index + 1) % self.playlist.len();
                self.current_track_id = self.playlist[self.playlist_index];
                true
            }
            PlayerRepeatMode::None => {
                if self.playlist_index + 1 < self.playlist.len() {
                    self.playlist_index += 1;
                    self.current_track_id = self.playlist[self.playlist_index];
                    true
                } else {
                    self.stop();
                    false
                }
            }
        }
    }

    pub fn previous(&mut self) -> bool {
        if self.playlist.is_empty() {
            return false;
        }
        if self.playlist_index > 0 {
            self.playlist_index -= 1;
            self.current_track_id = self.playlist[self.playlist_index];
            true
        } else if self.repeat_mode == PlayerRepeatMode::All {
            self.playlist_index = self.playlist.len().saturating_sub(1);
            self.current_track_id = self.playlist[self.playlist_index];
            true
        } else {
            false
        }
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.5);
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }
}
