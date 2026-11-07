//! Upstream Source: `Telegram/SourceFiles/media/streaming/media_streaming_common.h:L1-L214`
//! Parity Module: `media_streaming_common` | Feature: `TCG-N52` (Media Streaming Pipeline)

use serde::{Deserialize, Serialize};

pub const K_TIME_UNKNOWN: i64 = i64::MIN;
pub const K_DURATION_MAX: i64 = i32::MAX as i64;
pub const K_DURATION_UNAVAILABLE: i64 = i64::MAX;
pub const K_MAX_FRAME_AREA: u32 = 3840 * 2160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Both,
    Audio,
    Video,
    Inspection,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Both
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackOptions {
    pub mode: Mode,
    pub position: i64,
    pub duration_override: i64,
    pub speed: f64,
    pub volume: f64,
    pub audio_id: u64,
    pub sync_video_by_audio: bool,
    pub wait_for_mark_as_shown: bool,
    pub hw_allowed: bool,
    pub seekable: bool,
    pub loop_playback: bool,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Both,
            position: 0,
            duration_override: 0,
            speed: 1.0,
            volume: 1.0,
            audio_id: 0,
            sync_video_by_audio: true,
            wait_for_mark_as_shown: false,
            hw_allowed: false,
            seekable: true,
            loop_playback: false,
        }
    }
}

impl PlaybackOptions {
    pub fn sanitized(mut self) -> Self {
        self.speed = self.speed.clamp(0.5, 2.0);
        self.volume = self.volume.clamp(0.0, 1.0);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackState {
    pub position: i64,
    pub received_till: i64,
    pub duration: i64,
}

impl Default for TrackState {
    fn default() -> Self {
        Self {
            position: K_TIME_UNKNOWN,
            received_till: K_TIME_UNKNOWN,
            duration: K_TIME_UNKNOWN,
        }
    }
}

impl TrackState {
    pub fn is_fully_received(&self) -> bool {
        self.duration != K_TIME_UNKNOWN && self.received_till >= self.duration
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoInformation {
    pub state: TrackState,
    pub size: (u32, u32),
    pub real_size: (u32, u32),
    pub cover: Vec<u8>,
    pub rotation: i32,
    pub fps: f64,
    pub alpha: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioInformation {
    pub state: TrackState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Information {
    pub video: VideoInformation,
    pub audio: AudioInformation,
    pub header_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SpeedEstimate {
    pub bytes_per_second: u32,
    pub unreliable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamingUpdate {
    Information(Information),
    PreloadedVideo(i64),
    UpdateVideo(i64),
    PreloadedAudio(i64),
    UpdateAudio(i64),
    WaitingForData(bool),
    SpeedEstimate(SpeedEstimate),
    MutedByOther,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamingError {
    OpenFailed,
    LoadFailed,
    InvalidData,
    NotStreamable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameRequest {
    pub resize: (u32, u32),
    pub outer: (u32, u32),
    pub rounding_radius: u32,
    pub colored_rgba: [u8; 4],
    pub blurred_background: bool,
    pub require_argb32: bool,
    pub keep_alpha: bool,
    pub strict: bool,
}

impl Default for FrameRequest {
    fn default() -> Self {
        Self {
            resize: (0, 0),
            outer: (0, 0),
            rounding_radius: 0,
            colored_rgba: [0, 0, 0, 0],
            blurred_background: false,
            require_argb32: true,
            keep_alpha: false,
            strict: true,
        }
    }
}

impl FrameRequest {
    pub fn is_empty(&self) -> bool {
        if self.blurred_background {
            self.outer.0 == 0 || self.outer.1 == 0
        } else {
            self.resize.0 == 0 || self.resize.1 == 0
        }
    }

    pub fn good_for(&self, other: &Self) -> bool {
        self.blurred_background == other.blurred_background
            && self.require_argb32 == other.require_argb32
            && self.keep_alpha == other.keep_alpha
            && self.colored_rgba == other.colored_rgba
            && ((self.strict && !other.strict) || (self == other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameFormat {
    None,
    ARGB32,
    YUV420,
    NV12,
    NativeTexture,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FrameWithInfo {
    pub format: FrameFormat,
    pub index: i32,
    pub alpha: bool,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Default for FrameFormat {
    fn default() -> Self {
        Self::None
    }
}

/// Ledger Target Symbol: `MediaStreamingCommonStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStreamingCommonStreaming {
    pub default_mode: Mode,
    pub default_options: PlaybackOptions,
    pub max_frame_area: u32,
    pub supports_speed_control: bool,
}

impl MediaStreamingCommonStreaming {
    pub fn new() -> Self {
        Self {
            default_mode: Mode::Both,
            default_options: PlaybackOptions::default(),
            max_frame_area: K_MAX_FRAME_AREA,
            supports_speed_control: true,
        }
    }
}
