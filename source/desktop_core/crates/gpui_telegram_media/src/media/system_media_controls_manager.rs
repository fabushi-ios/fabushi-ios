//! Telegram Desktop 1:1 Parity Submodule: `SystemMediaControlsManager`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/system_media_controls_manager.h:L1-L70`
//! - `Telegram/SourceFiles/media/system_media_controls_manager.cpp:L1-L636`
//! Subsystem: `media` | Feature: `TCG-N08`

use super::media_common::RepeatMode;
use super::system_media_controls_video::VideoState;

/// Skip interval for system media transport controls (system_media_controls_manager.cpp:L35).
pub const K_SKIP_MS: i64 = 15_000;

/// SMTC loop status corresponding to `base::Platform::SystemMediaControls::LoopStatus` (L39-L44).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopStatus {
    #[default]
    None,
    Track,
    Playlist,
}

/// SMTC playback status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// Converts player repeat mode to OS media loop status (L37-L46).
pub fn repeat_mode_to_loop_status(mode: RepeatMode) -> LoopStatus {
    match mode {
        RepeatMode::None => LoopStatus::None,
        RepeatMode::One => LoopStatus::Track,
        RepeatMode::All => LoopStatus::Playlist,
    }
}

/// Metadata for active playing track.
#[derive(Debug, Clone, Default)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub duration_ms: i64,
    pub position_ms: i64,
    pub playback_rate: f64,
}

/// Parity target coordinator struct `SystemMediaControlsManager` (system_media_controls_manager.h:L34-L67).
#[derive(Debug, Clone, Default)]
pub struct SystemMediaControlsManager {
    pub is_supported: bool,
    pub is_initialized: bool,
    pub track: TrackMetadata,
    pub status: PlaybackStatus,
    pub loop_status: LoopStatus,
    pub active_video: Option<VideoState>,
}

impl SystemMediaControlsManager {
    pub fn new() -> Self {
        Self {
            is_supported: Self::supported(),
            is_initialized: false,
            track: TrackMetadata {
                playback_rate: 1.0,
                ..Default::default()
            },
            status: PlaybackStatus::Stopped,
            loop_status: LoopStatus::None,
            active_video: None,
        }
    }

    /// Whether native OS system media controls are supported (system_media_controls_manager.cpp:L63-L65).
    pub fn supported() -> bool {
        cfg!(target_os = "macos") || cfg!(target_os = "windows") || cfg!(target_os = "linux")
    }

    pub fn set_track(&mut self, title: impl Into<String>, artist: impl Into<String>, duration_ms: i64) {
        self.track.title = title.into();
        self.track.artist = artist.into();
        self.track.duration_ms = duration_ms;
        self.track.position_ms = 0;
    }

    pub fn set_playback_status(&mut self, status: PlaybackStatus) {
        self.status = status;
    }

    pub fn set_position(&mut self, position_ms: i64) {
        self.track.position_ms = position_ms.clamp(0, self.track.duration_ms);
    }

    pub fn set_repeat_mode(&mut self, mode: RepeatMode) {
        self.loop_status = repeat_mode_to_loop_status(mode);
    }

    pub fn skip_forward(&mut self) {
        let new_pos = self.track.position_ms + K_SKIP_MS;
        self.set_position(new_pos);
    }

    pub fn skip_backward(&mut self) {
        let new_pos = (self.track.position_ms - K_SKIP_MS).max(0);
        self.set_position(new_pos);
    }

    pub fn video_start(&mut self, state: VideoState) {
        self.active_video = Some(state);
    }

    pub fn video_update(&mut self, state: VideoState) {
        if let Some(ref mut current) = self.active_video {
            *current = state;
        }
    }

    pub fn video_finish(&mut self) {
        self.active_video = None;
    }
}
