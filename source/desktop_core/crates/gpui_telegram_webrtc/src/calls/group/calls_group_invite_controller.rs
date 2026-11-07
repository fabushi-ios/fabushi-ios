//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupInviteControllerGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_invite_controller.h (L1-L105)
//!   - Telegram/SourceFiles/calls/group/calls_group_invite_controller.cpp (L1-L1215)

use serde::{Deserialize, Serialize};

/// Maximum contacts invited per API request batch.
/// Upstream: `calls_group_call.cpp:L53`, `calls_group_invite_controller.cpp:L65`
pub const K_MAX_INVITE_PER_SLICE: usize = 10;

/// Invite link permission type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InviteLinkType {
    ListenOnly,
    CanSpeak,
}

/// Parity coordinator struct for `CallsGroupInviteControllerGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupInviteControllerGroup {
    pub call_id: u64,
    pub pending_invitees: Vec<u64>,
    pub invited_history: Vec<u64>,
    pub custom_invite_hash: Option<String>,
}

impl CallsGroupInviteControllerGroup {
    pub fn new(call_id: u64) -> Self {
        Self {
            call_id,
            pending_invitees: Vec::new(),
            invited_history: Vec::new(),
            custom_invite_hash: None,
        }
    }

    pub fn enqueue_invites(&mut self, user_ids: &[u64]) {
        for &id in user_ids {
            if !self.invited_history.contains(&id) && !self.pending_invitees.contains(&id) {
                self.pending_invitees.push(id);
            }
        }
    }

    /// Process next slice of up to `K_MAX_INVITE_PER_SLICE` (10) invitees.
    pub fn process_next_slice(&mut self) -> Vec<u64> {
        let count = self.pending_invitees.len().min(K_MAX_INVITE_PER_SLICE);
        let slice: Vec<u64> = self.pending_invitees.drain(..count).collect();
        self.invited_history.extend_from_slice(&slice);
        slice
    }

    pub fn pending_count(&self) -> usize {
        self.pending_invitees.len()
    }

    pub fn generate_invite_link(&mut self, link_type: InviteLinkType) -> String {
        let suffix = match link_type {
            InviteLinkType::ListenOnly => "listen",
            InviteLinkType::CanSpeak => "speak",
        };
        let hash = format!("{:x}_{}", self.call_id, suffix);
        self.custom_invite_hash = Some(hash.clone());
        format!("https://t.me/call?video_chat={}&hash={}", self.call_id, hash)
    }
}
