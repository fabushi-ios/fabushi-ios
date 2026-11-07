//! 1-on-1 Voice & Video Call State Machine
//! Upstream: Telegram/SourceFiles/calls/calls_call.h:L70-L290, calls_call.cpp:L47-L200
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallType {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    Starting,
    WaitingInit,
    WaitingInitAck,
    Established,
    FailedHangingUp,
    Failed,
    HangingUp,
    MigrationHangingUp,
    Ended,
    EndedByOtherDevice,
    ExchangingKeys,
    Waiting,
    Requesting,
    WaitingIncoming,
    Ringing,
    Busy,
    WaitingUserConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteAudioState {
    Muted,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteVideoState {
    Inactive,
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteBatteryState {
    Low,
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsCall {
    pub id: i64,
    pub user_id: u64,
    pub call_type: CallType,
    pub state: CallState,
    pub video_enabled: bool,
    pub muted: bool,
    pub remote_audio: RemoteAudioState,
    pub remote_video: RemoteVideoState,
    pub remote_battery: RemoteBatteryState,
    pub signal_bar_count: i32,
    pub key_sha256: Option<[u8; 32]>,
    pub duration_ms: u64,
    pub rating_requested: bool,
    pub rating_value: Option<i32>,
    pub camera_sharing: bool,
    pub screen_sharing: bool,
}

impl CallsCall {
    pub const K_MIN_LAYER: i32 = 65;
    pub const K_HANGUP_TIMEOUT_MS: u64 = 5000;
    pub const K_SHA256_SIZE: usize = 32;
    pub const K_AUTH_KEY_SIZE: usize = 256;
    pub const K_DEFAULT_VERSION: &'static str = "2.4.4";
    pub const K_SOUND_SAMPLE_MS: u64 = 100;
    pub const K_SIGNAL_BAR_STARTING: i32 = -1;
    pub const K_SIGNAL_BAR_FINISHED: i32 = -2;
    pub const K_SIGNAL_BAR_COUNT: i32 = 4;

    pub fn new(id: i64, user_id: u64, call_type: CallType, video: bool) -> Self {
        let initial_state = match call_type {
            CallType::Outgoing => CallState::Requesting,
            CallType::Incoming => CallState::WaitingIncoming,
        };
        Self {
            id,
            user_id,
            call_type,
            state: initial_state,
            video_enabled: video,
            muted: false,
            remote_audio: RemoteAudioState::Active,
            remote_video: if video { RemoteVideoState::Active } else { RemoteVideoState::Inactive },
            remote_battery: RemoteBatteryState::Normal,
            signal_bar_count: Self::K_SIGNAL_BAR_STARTING,
            key_sha256: None,
            duration_ms: 0,
            rating_requested: false,
            rating_value: None,
            camera_sharing: video,
            screen_sharing: false,
        }
    }

    pub fn is_incoming_waiting(&self) -> bool {
        self.call_type == CallType::Incoming && self.state == CallState::WaitingIncoming
    }

    pub fn answer(&mut self) {
        if self.state == CallState::WaitingIncoming {
            self.state = CallState::ExchangingKeys;
        }
    }

    pub fn keys_exchanged(&mut self, sha: [u8; 32]) {
        self.key_sha256 = Some(sha);
        self.state = CallState::Established;
        self.signal_bar_count = Self::K_SIGNAL_BAR_COUNT;
    }

    pub fn hangup(&mut self) {
        self.state = CallState::HangingUp;
        self.signal_bar_count = Self::K_SIGNAL_BAR_FINISHED;
    }

    pub fn finish(&mut self, rating_requested: bool) {
        self.state = CallState::Ended;
        self.rating_requested = rating_requested;
    }

    pub fn redial(&mut self) {
        self.state = CallState::Requesting;
        self.duration_ms = 0;
        self.signal_bar_count = Self::K_SIGNAL_BAR_STARTING;
        self.key_sha256 = None;
    }

    pub fn set_rating(&mut self, rating: i32) {
        self.rating_value = Some(rating.clamp(1, 5));
    }

    pub fn toggle_camera_sharing(&mut self, enabled: bool) {
        self.camera_sharing = enabled;
        if enabled {
            self.screen_sharing = false;
        }
    }

    pub fn toggle_screen_sharing(&mut self, enabled: bool) {
        self.screen_sharing = enabled;
        if enabled {
            self.camera_sharing = false;
        }
    }
}
