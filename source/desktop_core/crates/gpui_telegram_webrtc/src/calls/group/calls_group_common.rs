//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupCommonGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_common.h (L1-L215)
//!   - Telegram/SourceFiles/calls/group/calls_group_common.cpp (L1-L544)

use serde::{Deserialize, Serialize};

/// Group call panel display mode.
/// Upstream: `calls_group_common.h:L133-L137`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PanelMode {
    #[default]
    Normal,
    Wide,
    Fullscreen,
}

/// Video streaming quality request.
/// Upstream: `calls_group_common.h:L139-L144`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoQuality {
    Low,
    #[default]
    Medium,
    High,
    FullHd,
}

/// Participant mute and speech state.
/// Upstream: `calls_group_call.h:L71-L85`, `calls_group_common.h:L106-L112`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MuteState {
    #[default]
    MutedByAdmin,
    MutedByMe,
    Unmuted,
    Speaking,
    RaisedHand,
}

/// Call error enum.
/// Upstream: `calls_group_common.h:L145-L153`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CallError {
    #[default]
    Unknown,
    NoServerConnection,
    TooManyParticipants,
    IncompatibleProtocol,
    MicrophonePermissionDenied,
}

/// In-call sticky tooltip hint types.
/// Upstream: `calls_group_common.h:L155-L162`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StickedTooltip {
    #[default]
    None,
    MuteHint,
    ScreenShareHint,
    RecordingHint,
    NoiseSuppressionHint,
}

/// Information needed to join a voice chat / group call.
/// Upstream: `calls_group_common.h:L123-L131`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinInfo {
    pub call_id: u64,
    pub join_as_peer_id: u64,
    pub muted: bool,
    pub video_enabled: bool,
    pub schedule_date: Option<i64>,
}

/// RTMP streaming parameters.
/// Upstream: `calls_group_common.h:L118-L122`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtmpInfo {
    pub url: String,
    pub key: String,
}

/// Request to adjust participant audio volume.
/// Upstream: `calls_group_common.h:L99-L105`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeRequest {
    pub participant_id: u64,
    pub volume: u32, // 0..20000 (10000 = 100%)
}

/// Request to mute/unmute a participant.
/// Upstream: `calls_group_common.h:L93-L98`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuteRequest {
    pub participant_id: u64,
    pub mute: bool,
}

/// State representation of a participant in a group call.
/// Upstream: `calls_group_common.h:L106-L112`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantState {
    pub id: u64,
    pub name: String,
    pub mute_state: MuteState,
    pub volume: u32,
    pub audio_level: f32, // 0.0 .. 1.0
    pub raise_hand_timestamp: Option<i64>,
    pub is_self: bool,
    pub can_self_unmute: bool,
}

impl Default for ParticipantState {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            mute_state: MuteState::MutedByAdmin,
            volume: 10000,
            audio_level: 0.0,
            raise_hand_timestamp: None,
            is_self: false,
            can_self_unmute: false,
        }
    }
}

/// Convert normalized volume ratio (0.0 .. 2.0) to Telegram integer volume (0 .. 20000).
/// Upstream: `calls_group_common.cpp:L120-L135`
pub fn ratio_to_volume(ratio: f32) -> u32 {
    let clamped = ratio.clamp(0.0, 2.0);
    (clamped * 10000.0).round() as u32
}

/// Convert Telegram integer volume (0 .. 20000) to normalized ratio (0.0 .. 2.0).
/// Upstream: `calls_group_common.cpp:L137-L145`
pub fn volume_to_ratio(volume: u32) -> f32 {
    let clamped = volume.min(20000);
    clamped as f32 / 10000.0
}

/// Format duration in seconds as `mm:ss` or `hh:mm:ss`.
/// Upstream: `calls_group_common.cpp:L210-L230`
pub fn format_call_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

/// Primary parity coordinator struct for `CallsGroupCommonGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupCommonGroup {
    pub mode: PanelMode,
    pub default_quality: VideoQuality,
    pub default_volume: u32,
}

impl CallsGroupCommonGroup {
    pub fn new() -> Self {
        Self {
            mode: PanelMode::Normal,
            default_quality: VideoQuality::Medium,
            default_volume: 10000,
        }
    }

    pub fn set_panel_mode(&mut self, mode: PanelMode) {
        self.mode = mode;
    }

    pub fn is_wide(&self) -> bool {
        self.mode == PanelMode::Wide
    }

    pub fn is_fullscreen(&self) -> bool {
        self.mode == PanelMode::Fullscreen
    }
}
