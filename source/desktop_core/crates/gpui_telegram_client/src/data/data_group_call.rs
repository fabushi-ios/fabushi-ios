//! Target Symbol: DataGroupCall
//! Telegram Desktop 1:1 Submodule: `data/data_group_call`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCallParticipant {
    pub peer_id: u64,
    pub date: i64,
    pub is_muted: bool,
    pub can_speak: bool,
    pub is_speaking: bool,
    pub has_video: bool,
    pub has_screen_sharing: bool,
    pub volume: u32, // 0..20000 (10000 = 100%)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataGroupCall {
    pub id: u64,
    pub access_hash: u64,
    pub title: Option<String>,
    pub participants_count: u32,
    pub is_active: bool,
    pub can_start_video: bool,
    pub is_rtmp_stream: bool,
    pub participants: Vec<GroupCallParticipant>,
}

impl Default for DataGroupCall {
    fn default() -> Self {
        Self {
            id: 0,
            access_hash: 0,
            title: None,
            participants_count: 0,
            is_active: false,
            can_start_video: true,
            is_rtmp_stream: false,
            participants: Vec::new(),
        }
    }
}

impl DataGroupCall {
    pub fn new(id: u64, access_hash: u64) -> Self {
        Self {
            id,
            access_hash,
            is_active: true,
            ..Default::default()
        }
    }

    pub fn add_participant(&mut self, p: GroupCallParticipant) {
        if !self.participants.iter().any(|existing| existing.peer_id == p.peer_id) {
            self.participants.push(p);
            self.participants_count = self.participants.len() as u32;
        }
    }

    pub fn remove_participant(&mut self, peer_id: u64) {
        self.participants.retain(|p| p.peer_id != peer_id);
        self.participants_count = self.participants.len() as u32;
    }

    pub fn set_participant_muted(&mut self, peer_id: u64, muted: bool) {
        if let Some(p) = self.participants.iter_mut().find(|p| p.peer_id == peer_id) {
            p.is_muted = muted;
        }
    }
}
