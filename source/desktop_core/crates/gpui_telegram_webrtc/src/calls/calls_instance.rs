//! Calls Instance Coordinator
//! Upstream: Telegram/SourceFiles/calls/calls_instance.h:L83-L170, calls_instance.cpp:L50-L200
#![allow(dead_code)]

use super::calls_call::{CallState, CallType, CallsCall};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallSound {
    Connecting,
    Busy,
    Ended,
    GroupCallStart,
    GroupCallEnd,
    GroupCallAllowed,
    GroupCallConnect,
    GroupCallRecordingStart,
}

impl CallSound {
    pub fn sound_track_name(self) -> &'static str {
        match self {
            Self::Connecting => "call_connect",
            Self::Busy => "call_busy",
            Self::Ended => "call_end",
            Self::GroupCallStart => "group_call_start",
            Self::GroupCallEnd => "group_call_end",
            Self::GroupCallAllowed => "group_call_allowed",
            Self::GroupCallConnect => "group_call_connect",
            Self::GroupCallRecordingStart => "group_call_recording_start",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConferenceInviteMessages {
    pub incoming: HashSet<i64>,
    pub outgoing: HashSet<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsInstance {
    pub current_call: Option<CallsCall>,
    pub active_group_call_id: Option<i64>,
    pub conference_invites: HashMap<i64, HashMap<u64, ConferenceInviteMessages>>,
    pub last_sound_played: Option<String>,
    pub server_config_updated_at: u64,
}

impl CallsInstance {
    pub const K_SERVER_CONFIG_UPDATE_TIMEOUT_MS: u64 = 24 * 3600 * 1000;

    pub fn new() -> Self {
        Self {
            current_call: None,
            active_group_call_id: None,
            conference_invites: HashMap::new(),
            last_sound_played: None,
            server_config_updated_at: 0,
        }
    }

    pub fn in_call(&self) -> bool {
        self.current_call.as_ref().map_or(false, |c| {
            c.state != CallState::Ended && c.state != CallState::Failed
        })
    }

    pub fn start_outgoing_call(&mut self, call_id: i64, user_id: u64, video: bool) -> &mut CallsCall {
        self.play_sound(CallSound::Connecting);
        let call = CallsCall::new(call_id, user_id, CallType::Outgoing, video);
        self.current_call = Some(call);
        self.current_call.as_mut().unwrap()
    }

    pub fn play_sound(&mut self, sound: CallSound) {
        self.last_sound_played = Some(sound.sound_track_name().to_string());
    }

    pub fn destroy_current_call(&mut self) {
        if let Some(mut call) = self.current_call.take() {
            call.finish(false);
            self.play_sound(CallSound::Ended);
        }
    }

    pub fn register_conference_invite(&mut self, conf_id: i64, user_id: u64, msg_id: i64, incoming: bool) {
        let conf_entry = self.conference_invites.entry(conf_id).or_default();
        let user_entry = conf_entry.entry(user_id).or_default();
        if incoming {
            user_entry.incoming.insert(msg_id);
        } else {
            user_entry.outgoing.insert(msg_id);
        }
    }

    pub fn unregister_conference_invite(&mut self, conf_id: i64, user_id: u64, msg_id: i64, incoming: bool) {
        if let Some(conf_entry) = self.conference_invites.get_mut(&conf_id) {
            if let Some(user_entry) = conf_entry.get_mut(&user_id) {
                if incoming {
                    user_entry.incoming.remove(&msg_id);
                } else {
                    user_entry.outgoing.remove(&msg_id);
                }
            }
        }
    }
}
