//! Group Call Session Controller: `CallsGroupController`
//! Bridges high-level call orchestration with WebRTC transport and member state.

use super::calls_group_call::{CallState, CallsGroupCallGroup};
use super::calls_group_common::{JoinInfo, MuteState};

/// Controller managing high-level group call actions and event handling.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupController {
    pub inner: CallsGroupCallGroup,
    pub title: String,
    pub is_muted_locally: bool,
}

impl CallsGroupController {
    pub fn new(call_id: u64, self_peer_id: u64, title: &str, is_admin: bool) -> Self {
        Self {
            inner: CallsGroupCallGroup::new(call_id, self_peer_id, is_admin),
            title: title.to_string(),
            is_muted_locally: true,
        }
    }

    pub fn start_join(&mut self, muted: bool, video: bool) {
        let info = JoinInfo {
            call_id: self.inner.call_id,
            join_as_peer_id: self.inner.self_peer_id,
            muted,
            video_enabled: video,
            schedule_date: None,
        };
        self.is_muted_locally = muted;
        self.inner.join(info);
    }

    pub fn on_network_connected(&mut self) {
        self.inner.mark_connected();
    }

    pub fn toggle_mic(&mut self) -> MuteState {
        let state = self.inner.toggle_self_mute();
        self.is_muted_locally = matches!(state, MuteState::MutedByMe | MuteState::MutedByAdmin);
        state
    }

    pub fn handle_audio_level(&mut self, peer_id: u64, level: f32) {
        self.inner.update_audio_level(peer_id, level);
    }

    pub fn is_active(&self) -> bool {
        self.inner.state == CallState::Active
    }

    pub fn participant_count(&self) -> usize {
        self.inner.participants.len()
    }
}
