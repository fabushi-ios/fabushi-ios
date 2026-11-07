//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMembersGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_members.h (L1-L132)
//!   - Telegram/SourceFiles/calls/group/calls_group_members.cpp (L1-L2236)

use super::calls_group_common::{MuteState, ParticipantState};
use std::collections::HashMap;

/// Duration to highlight raised hand status.
/// Upstream: `calls_group_members.cpp:L49`
pub const K_KEEP_RAISED_HAND_STATUS_DURATION_MS: u64 = 3000;

/// Parity component for `CallsGroupMembersGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupMembersGroup {
    pub members: HashMap<u64, ParticipantState>,
    pub search_query: String,
}

impl CallsGroupMembersGroup {
    pub fn new() -> Self {
        Self {
            members: HashMap::new(),
            search_query: String::new(),
        }
    }

    pub fn set_search_query(&mut self, query: &str) {
        self.search_query = query.trim().to_lowercase();
    }

    pub fn update_or_insert(&mut self, member: ParticipantState) {
        self.members.insert(member.id, member);
    }

    pub fn remove(&mut self, id: u64) {
        self.members.remove(&id);
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Sort members by priority:
    /// 1. Active speakers (ordered by audio energy descending)
    /// 2. Raised hands (ordered by timestamp ascending)
    /// 3. Unmuted listeners
    /// 4. Muted listeners
    pub fn sorted_members(&self) -> Vec<ParticipantState> {
        let mut list: Vec<ParticipantState> = self
            .members
            .values()
            .filter(|m| {
                if self.search_query.is_empty() {
                    true
                } else {
                    m.name.to_lowercase().contains(&self.search_query)
                }
            })
            .cloned()
            .collect();

        list.sort_by(|a, b| {
            let rank = |m: &ParticipantState| match m.mute_state {
                MuteState::Speaking => 0,
                MuteState::RaisedHand => 1,
                MuteState::Unmuted => 2,
                MuteState::MutedByMe | MuteState::MutedByAdmin => 3,
            };
            let rank_a = rank(a);
            let rank_b = rank(b);
            if rank_a != rank_b {
                rank_a.cmp(&rank_b)
            } else if a.mute_state == MuteState::Speaking {
                // Higher audio level first
                b.audio_level.partial_cmp(&a.audio_level).unwrap_or(std::cmp::Ordering::Equal)
            } else if a.mute_state == MuteState::RaisedHand {
                // Earlier timestamp first
                a.raise_hand_timestamp.cmp(&b.raise_hand_timestamp)
            } else {
                a.name.cmp(&b.name)
            }
        });

        list
    }

    /// Get all participants with raised hands waiting for admin approval.
    pub fn raised_hands(&self) -> Vec<ParticipantState> {
        let mut list: Vec<ParticipantState> = self
            .members
            .values()
            .filter(|m| m.mute_state == MuteState::RaisedHand)
            .cloned()
            .collect();
        list.sort_by_key(|m| m.raise_hand_timestamp.unwrap_or(i64::MAX));
        list
    }

    /// Admin approves speak request.
    pub fn approve_speak(&mut self, id: u64) -> bool {
        if let Some(m) = self.members.get_mut(&id) {
            if m.mute_state == MuteState::RaisedHand {
                m.mute_state = MuteState::Unmuted;
                m.raise_hand_timestamp = None;
                m.can_self_unmute = true;
                return true;
            }
        }
        false
    }

    /// Admin mutes member.
    pub fn mute_member(&mut self, id: u64) -> bool {
        if let Some(m) = self.members.get_mut(&id) {
            m.mute_state = MuteState::MutedByAdmin;
            m.audio_level = 0.0;
            m.can_self_unmute = false;
            return true;
        }
        false
    }
}
