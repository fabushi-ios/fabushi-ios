//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupCallGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_call.h (L1-L816)
//!   - Telegram/SourceFiles/calls/group/calls_group_call.cpp (L1-L4378)

use super::calls_group_common::{CallError, JoinInfo, MuteState, ParticipantState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum invite items processed per batch slice.
/// Upstream: `calls_group_call.cpp:L53`
pub const K_MAX_INVITE_PER_SLICE: usize = 10;

/// Interval for checking speaking expiration.
/// Upstream: `calls_group_call.cpp:L54`
pub const K_CHECK_LAST_SPOKE_INTERVAL_MS: u64 = 1000;

/// Timeout waiting for join acknowledgment.
/// Upstream: `calls_group_call.cpp:L55`
pub const K_CHECK_JOINED_TIMEOUT_MS: u64 = 4000;

/// Interval between sending speaking action pings.
/// Upstream: `calls_group_call.cpp:L56`
pub const K_UPDATE_SEND_ACTION_EACH_MS: u64 = 500;

/// Interval for playing connecting ring sound.
/// Upstream: `calls_group_call.cpp:L57`
pub const K_PLAY_CONNECTING_EACH_MS: u64 = 3056;

/// High-level call lifecycle state.
/// Upstream: `calls_group_call.h:L360-L368`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CallState {
    #[default]
    Disconnected,
    Connecting,
    Active,
    Rejoining,
    Terminated,
}

/// Parity coordinator struct for `CallsGroupCallGroup`.
#[derive(Debug, Clone)]
pub struct CallsGroupCallGroup {
    pub call_id: u64,
    pub self_peer_id: u64,
    pub state: CallState,
    pub participants: HashMap<u64, ParticipantState>,
    pub is_recording: bool,
    pub is_rtmp: bool,
    pub can_manage: bool,
    pub last_error: Option<CallError>,
    pub active_speakers: Vec<u64>,
}

impl Default for CallsGroupCallGroup {
    fn default() -> Self {
        Self {
            call_id: 0,
            self_peer_id: 0,
            state: CallState::Disconnected,
            participants: HashMap::new(),
            is_recording: false,
            is_rtmp: false,
            can_manage: false,
            last_error: None,
            active_speakers: Vec::new(),
        }
    }
}

impl CallsGroupCallGroup {
    pub fn new(call_id: u64, self_peer_id: u64, can_manage: bool) -> Self {
        let mut group = Self {
            call_id,
            self_peer_id,
            state: CallState::Disconnected,
            participants: HashMap::new(),
            is_recording: false,
            is_rtmp: false,
            can_manage,
            last_error: None,
            active_speakers: Vec::new(),
        };
        // Add self participant
        group.participants.insert(
            self_peer_id,
            ParticipantState {
                id: self_peer_id,
                name: "Me".to_string(),
                mute_state: MuteState::MutedByMe,
                volume: 10000,
                audio_level: 0.0,
                raise_hand_timestamp: None,
                is_self: true,
                can_self_unmute: true,
            },
        );
        group
    }

    pub fn join(&mut self, info: JoinInfo) {
        self.call_id = info.call_id;
        self.state = CallState::Connecting;
        if let Some(self_part) = self.participants.get_mut(&self.self_peer_id) {
            self_part.mute_state = if info.muted {
                MuteState::MutedByMe
            } else {
                MuteState::Unmuted
            };
        }
    }

    pub fn mark_connected(&mut self) {
        self.state = CallState::Active;
    }

    pub fn add_participant(&mut self, state: ParticipantState) {
        let id = state.id;
        self.participants.insert(id, state);
    }

    pub fn remove_participant(&mut self, peer_id: u64) {
        self.participants.remove(&peer_id);
        self.active_speakers.retain(|&id| id != peer_id);
    }

    pub fn toggle_self_mute(&mut self) -> MuteState {
        if let Some(self_part) = self.participants.get_mut(&self.self_peer_id) {
            match self_part.mute_state {
                MuteState::Unmuted | MuteState::Speaking => {
                    self_part.mute_state = MuteState::MutedByMe;
                    self_part.audio_level = 0.0;
                }
                MuteState::MutedByMe => {
                    self_part.mute_state = MuteState::Unmuted;
                }
                MuteState::MutedByAdmin => {
                    // Cannot unmute directly, raise hand instead
                    self_part.mute_state = MuteState::RaisedHand;
                    self_part.raise_hand_timestamp = Some(chrono::Utc::now().timestamp());
                }
                MuteState::RaisedHand => {
                    // Lower hand
                    self_part.mute_state = MuteState::MutedByAdmin;
                    self_part.raise_hand_timestamp = None;
                }
            }
            self_part.mute_state
        } else {
            MuteState::MutedByMe
        }
    }

    pub fn update_audio_level(&mut self, peer_id: u64, level: f32) {
        if let Some(part) = self.participants.get_mut(&peer_id) {
            part.audio_level = level.clamp(0.0, 1.0);
            if part.audio_level > 0.05 && part.mute_state == MuteState::Unmuted {
                part.mute_state = MuteState::Speaking;
                if !self.active_speakers.contains(&peer_id) {
                    self.active_speakers.push(peer_id);
                }
            } else if part.audio_level <= 0.05 && part.mute_state == MuteState::Speaking {
                part.mute_state = MuteState::Unmuted;
                self.active_speakers.retain(|&id| id != peer_id);
            }
        }
    }

    pub fn set_participant_volume(&mut self, peer_id: u64, volume: u32) {
        if let Some(part) = self.participants.get_mut(&peer_id) {
            part.volume = volume.min(20000);
        }
    }

    pub fn set_recording(&mut self, recording: bool) {
        self.is_recording = recording;
    }

    pub fn terminate(&mut self) {
        self.state = CallState::Terminated;
        self.participants.clear();
        self.active_speakers.clear();
    }
}
