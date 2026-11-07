//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPlaybackControlsView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_playback_controls.h`
//! - `Telegram/SourceFiles/media/view/media_view_playback_controls.cpp:L29, L142-L171, L258-L272, L274-L284`

use serde::{Deserialize, Serialize};

pub const K_EPS: f64 = 0.005;

/// Video chapter or timestamp marker on the playback slider
/// Upstream: `media_view_playback_controls.h:L60-L63`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimestampData {
    pub position: f64, // normalized 0.0 .. 1.0
    pub label: String,
}

/// Video quality stream options
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoQuality {
    Auto,
    Quality360p,
    Quality480p,
    Quality720p,
    Quality1080p,
}

impl VideoQuality {
    pub fn height(&self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Quality360p => 360,
            Self::Quality480p => 480,
            Self::Quality720p => 720,
            Self::Quality1080p => 1080,
        }
    }
}

/// Volume indicator state
/// Upstream: `media_view_playback_controls.cpp:L258-L272`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeLevel {
    Muted,
    Low,  // < 0.5
    High, // >= 0.5
}

/// Primary target symbol for `media_view_playback_controls.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPlaybackControlsView {
    pub is_playing: bool,
    pub in_full_screen: bool,
    pub volume: f64,
    pub speed: f64,
    pub quality: VideoQuality,
    pub position_ms: i64,
    pub duration_ms: i64,
    pub seek_position_ms: Option<i64>,
    pub ready_bytes: i64,
    pub total_bytes: i64,
    pub file_header_size: i64,
    pub timestamps: Vec<TimestampData>,
    pub fade_opacity: f64,
}

impl Default for MediaViewPlaybackControlsView {
    fn default() -> Self {
        Self {
            is_playing: false,
            in_full_screen: false,
            volume: 1.0,
            speed: 1.0,
            quality: VideoQuality::Auto,
            position_ms: 0,
            duration_ms: 0,
            seek_position_ms: None,
            ready_bytes: 0,
            total_bytes: 0,
            file_header_size: 0,
            timestamps: Vec::new(),
            fade_opacity: 1.0,
        }
    }
}

impl MediaViewPlaybackControlsView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Handles progress during a seek gesture
    /// Upstream: `media_view_playback_controls.cpp:L142-L157`
    pub fn handle_seek_progress(&mut self, progress: f64) -> i64 {
        if self.duration_ms <= 0 {
            return 0;
        }
        let pos = (progress * self.duration_ms as f64).clamp(0.0, self.duration_ms as f64) as i64;
        self.seek_position_ms = Some(pos);
        pos
    }

    /// Handles seek completion
    /// Upstream: `media_view_playback_controls.cpp:L159-L171`
    pub fn handle_seek_finished(&mut self, progress: f64) -> i64 {
        if self.duration_ms <= 0 {
            return 0;
        }
        let pos = (progress * self.duration_ms as f64).clamp(0.0, self.duration_ms as f64) as i64;
        self.seek_position_ms = None;
        self.position_ms = pos;
        pos
    }

    /// Computes volume icon state
    /// Upstream: `media_view_playback_controls.cpp:L258-L272`
    pub fn volume_level(&self) -> VolumeLevel {
        if self.volume <= 0.0 {
            VolumeLevel::Muted
        } else if self.volume < 0.5 {
            VolumeLevel::Low
        } else {
            VolumeLevel::High
        }
    }

    /// Computes downloaded buffer ratio
    /// Upstream: `media_view_playback_controls.cpp:L274-L284`
    pub fn count_downloaded_till_percent(&self) -> f64 {
        if self.ready_bytes > 0 && self.ready_bytes == self.total_bytes {
            return 1.0;
        }
        let header = self.file_header_size;
        if header <= 0 || self.ready_bytes <= header || self.total_bytes <= header {
            return 0.0;
        }
        (self.ready_bytes - header) as f64 / (self.total_bytes - header) as f64
    }

    /// Adds timestamp / chapter divider
    pub fn add_timestamp(&mut self, position: f64, label: impl Into<String>) {
        if (0.0..=1.0).contains(&position) {
            self.timestamps.push(TimestampData {
                position,
                label: label.into(),
            });
            self.timestamps.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap());
        }
    }

    /// Toggles play/pause
    pub fn toggle_play_pause(&mut self) {
        self.is_playing = !self.is_playing;
    }

    /// Toggles fullscreen
    pub fn toggle_fullscreen(&mut self) {
        self.in_full_screen = !self.in_full_screen;
    }

    /// Adjusts volume
    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }
}
