//! Group Call Video / Audio Participant Item: `CallsGroupItem`
//! Manages rendering tiles and pinning states in the group call grid.

use super::calls_group_common::{MuteState, ParticipantState};
use serde::{Deserialize, Serialize};

/// Visual representation of a video/audio participant tile in the call viewport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupItem {
    pub participant_id: u64,
    pub name: String,
    pub has_video: bool,
    pub is_screen_share: bool,
    pub is_pinned: bool,
    pub mute_state: MuteState,
    pub volume: u32,
    pub audio_energy: f32,
}

impl CallsGroupItem {
    pub fn from_participant(state: &ParticipantState, has_video: bool, is_screen_share: bool) -> Self {
        Self {
            participant_id: state.id,
            name: state.name.clone(),
            has_video,
            is_screen_share,
            is_pinned: false,
            mute_state: state.mute_state,
            volume: state.volume,
            audio_energy: state.audio_level,
        }
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.is_pinned = pinned;
    }

    pub fn update_audio(&mut self, energy: f32, mute: MuteState) {
        self.audio_energy = energy;
        self.mute_state = mute;
    }

    pub fn display_title(&self) -> String {
        if self.is_screen_share {
            format!("{} (Screen)", self.name)
        } else {
            self.name.clone()
        }
    }
}
