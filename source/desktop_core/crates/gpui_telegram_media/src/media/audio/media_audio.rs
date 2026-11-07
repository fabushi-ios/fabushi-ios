//! Telegram Desktop 1:1 Parity Submodule: `media_audio.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Audio Playback Engine)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio.h:L60-L137, L205-L280`
//!           `Telegram/SourceFiles/media/audio/media_audio.cpp:L235-L263, L440-L455, L1552-L1625`

use serde::{Deserialize, Serialize};

/// Upstream: `media_audio.h:L62-L65`
pub const DEFAULT_FREQUENCY: i32 = 48000;
pub const TOGETHER_LIMIT: usize = 4;
pub const WAVEFORM_SAMPLES_COUNT: usize = 100;

/// Upstream: `media_audio.cpp:L235-L241`
pub const VOLUME_ROUND: i32 = 10000;
pub const PRELOAD_SECONDS: i64 = 2;
pub const FADE_DURATION_MS: i64 = 500;
pub const CHECK_PLAYBACK_POSITION_TIMEOUT_MS: i64 = 100;
pub const CHECK_PLAYBACK_POSITION_DELTA_SAMPLES: i64 = 2400;
pub const CHECK_FADING_TIMEOUT_MS: i64 = 7;

/// Upstream: `media_audio.cpp:L35-L36`
pub const SUPPRESS_RATIO_ALL: f64 = 0.2;
pub const SUPPRESS_RATIO_SONG: f64 = 0.05;

/// Playback state machine
/// Upstream: `media_audio.h:L73-L86`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum PlayerState {
    #[default]
    Stopped = 0x01,
    StoppedAtEnd = 0x02,
    StoppedAtError = 0x03,
    StoppedAtStart = 0x04,
    Starting = 0x08,
    Playing = 0x10,
    Stopping = 0x18,
    Pausing = 0x20,
    Paused = 0x28,
    PausedAtEnd = 0x30,
    Resuming = 0x38,
}

impl PlayerState {
    /// Upstream: `media_audio.h:L88-L93`
    pub fn is_stopped(self) -> bool {
        matches!(
            self,
            PlayerState::Stopped
                | PlayerState::StoppedAtEnd
                | PlayerState::StoppedAtError
                | PlayerState::StoppedAtStart
        )
    }

    /// Upstream: `media_audio.h:L95-L97`
    pub fn is_stopped_or_stopping(self) -> bool {
        self.is_stopped() || self == PlayerState::Stopping
    }

    /// Upstream: `media_audio.h:L103-L106`
    pub fn is_paused(self) -> bool {
        matches!(self, PlayerState::Paused | PlayerState::PausedAtEnd)
    }

    /// Upstream: `media_audio.h:L108-L110`
    pub fn is_paused_or_pausing(self) -> bool {
        self.is_paused() || self == PlayerState::Pausing
    }

    /// Upstream: `media_audio.h:L112-L117`
    pub fn is_fading(self) -> bool {
        matches!(
            self,
            PlayerState::Starting
                | PlayerState::Stopping
                | PlayerState::Pausing
                | PlayerState::Resuming
        )
    }

    /// Upstream: `media_audio.h:L119-L121`
    pub fn is_active(self) -> bool {
        !self.is_stopped() && !self.is_paused()
    }

    /// Upstream: `media_audio.h:L123-L126`
    pub fn show_pause_icon(self) -> bool {
        !self.is_stopped_or_stopping() && !self.is_paused_or_pausing()
    }
}

/// Upstream: `media_audio.h:L128-L137`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackState {
    pub id: u64,
    pub state: PlayerState,
    pub position_samples: i64,
    pub received_till_samples: i64,
    pub length_samples: i64,
    pub frequency: i32,
    pub file_header_size: i32,
    pub waiting_for_data: bool,
}

impl Default for TrackState {
    fn default() -> Self {
        Self {
            id: 0,
            state: PlayerState::Stopped,
            position_samples: 0,
            received_till_samples: 0,
            length_samples: 0,
            frequency: DEFAULT_FREQUENCY,
            file_header_size: 0,
            waiting_for_data: false,
        }
    }
}

impl TrackState {
    pub fn duration_ms(&self) -> i64 {
        if self.frequency > 0 {
            (self.length_samples * 1000) / self.frequency as i64
        } else {
            0
        }
    }

    pub fn position_ms(&self) -> i64 {
        if self.frequency > 0 {
            (self.position_samples * 1000) / self.frequency as i64
        } else {
            0
        }
    }
}

/// Cubic volume computation
/// Upstream: `media_audio.cpp:L251-L263`
pub fn compute_volume_cubic(gain: f64) -> f64 {
    let clamped = gain.clamp(0.0, 1.0);
    clamped * clamped * clamped
}

/// Speed position conversion
/// Upstream: `media_audio.cpp:L440-L455`
pub fn speed_independent_position(position_samples: i64, speed: f64) -> i64 {
    (position_samples as f64 * speed).round() as i64
}

pub fn speed_dependent_position(position_samples: i64, speed: f64) -> i64 {
    if speed <= 0.0001 {
        position_samples
    } else {
        (position_samples as f64 / speed).round() as i64
    }
}

/// 100-bar Voice Waveform calculation
/// Upstream: `media_audio.cpp:L1552-L1625`
pub fn calculate_voice_waveform_100_bars(samples: &[i16]) -> Vec<u8> {
    if samples.is_empty() {
        return vec![0; WAVEFORM_SAMPLES_COUNT];
    }
    let total_samples = samples.len();
    let mut peaks = Vec::with_capacity(WAVEFORM_SAMPLES_COUNT);
    let mut current_peak: u16 = 0;
    let mut sum_samples: usize = 0;

    for &sample in samples {
        let abs_sample = sample.unsigned_abs();
        if abs_sample > current_peak {
            current_peak = abs_sample;
        }
        sum_samples += WAVEFORM_SAMPLES_COUNT;
        if sum_samples >= total_samples {
            sum_samples -= total_samples;
            peaks.push(current_peak);
            current_peak = 0;
        }
    }
    if peaks.is_empty() {
        return vec![0; WAVEFORM_SAMPLES_COUNT];
    }
    let sum: u64 = peaks.iter().map(|&p| p as u64).sum();
    let avg = sum as f64 / peaks.len() as f64;
    let max_peak = (avg * 1.8).max(2500.0) as u32;

    peaks
        .into_iter()
        .map(|p| {
            let clamped = (p as u32).min(max_peak);
            ((clamped * 31) / max_peak).min(31) as u8
        })
        .collect()
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaAudioAudio {
    pub current_track: TrackState,
    pub volume: f64,
    pub speed: f64,
    pub is_attached: bool,
}

impl MediaAudioAudio {
    pub fn new() -> Self {
        Self {
            current_track: TrackState::default(),
            volume: 1.0,
            speed: 1.0,
            is_attached: false,
        }
    }

    pub fn compute_effective_volume(&self) -> f64 {
        compute_volume_cubic(self.volume)
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.5);
    }
}
