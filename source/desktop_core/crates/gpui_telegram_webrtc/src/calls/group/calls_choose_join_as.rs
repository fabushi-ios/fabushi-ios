//! Telegram Desktop 1:1 Parity Submodule: `CallsChooseJoinAsGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_choose_join_as.h (L1-L65)
//!   - Telegram/SourceFiles/calls/group/calls_choose_join_as.cpp (L1-L516)

use super::calls_group_common::JoinInfo;
use serde::{Deserialize, Serialize};

/// Upstream constant: `kLabelRefreshInterval = 10 * crl::time(1000)` (10 seconds)
/// Upstream: `calls_choose_join_as.cpp:L35`
pub const K_LABEL_REFRESH_INTERVAL_MS: u64 = 10_000;

/// An available identity to join the group call as.
/// Upstream: `calls_choose_join_as.h:L22-L28`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinAsOption {
    pub peer_id: u64,
    pub title: String,
    pub is_channel: bool,
    pub participant_count: Option<u32>,
    pub is_selected: bool,
}

/// Controller managing the "Choose Join As" dialogue flow.
/// Upstream: `calls_choose_join_as.h:L24-L45`, `calls_choose_join_as.cpp:L70-L150`
#[derive(Debug, Clone, Default)]
pub struct ChooseJoinAsProcess {
    pub call_id: u64,
    pub personal_peer_id: u64,
    pub options: Vec<JoinAsOption>,
    pub selected_peer_id: Option<u64>,
}

impl ChooseJoinAsProcess {
    pub fn new(call_id: u64, personal_peer_id: u64, personal_name: &str) -> Self {
        let personal_option = JoinAsOption {
            peer_id: personal_peer_id,
            title: personal_name.to_string(),
            is_channel: false,
            participant_count: None,
            is_selected: true,
        };
        Self {
            call_id,
            personal_peer_id,
            options: vec![personal_option],
            selected_peer_id: Some(personal_peer_id),
        }
    }

    pub fn add_channel_option(&mut self, channel_id: u64, title: &str, subscriber_count: u32) {
        self.options.push(JoinAsOption {
            peer_id: channel_id,
            title: title.to_string(),
            is_channel: true,
            participant_count: Some(subscriber_count),
            is_selected: false,
        });
    }

    pub fn select_option(&mut self, peer_id: u64) -> bool {
        let mut found = false;
        for opt in &mut self.options {
            if opt.peer_id == peer_id {
                opt.is_selected = true;
                found = true;
            } else {
                opt.is_selected = false;
            }
        }
        if found {
            self.selected_peer_id = Some(peer_id);
        }
        found
    }

    pub fn selected_peer(&self) -> u64 {
        self.selected_peer_id.unwrap_or(self.personal_peer_id)
    }

    pub fn create_join_info(&self, muted: bool, video_enabled: bool) -> JoinInfo {
        JoinInfo {
            call_id: self.call_id,
            join_as_peer_id: self.selected_peer(),
            muted,
            video_enabled,
            schedule_date: None,
        }
    }
}

/// Primary parity coordinator struct for `CallsChooseJoinAsGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsChooseJoinAsGroup {
    pub process: Option<ChooseJoinAsProcess>,
}

impl CallsChooseJoinAsGroup {
    pub fn new() -> Self {
        Self { process: None }
    }

    pub fn start(&mut self, call_id: u64, personal_id: u64, personal_name: &str) {
        self.process = Some(ChooseJoinAsProcess::new(call_id, personal_id, personal_name));
    }

    pub fn add_channel(&mut self, channel_id: u64, title: &str, subscribers: u32) {
        if let Some(ref mut proc) = self.process {
            proc.add_channel_option(channel_id, title, subscribers);
        }
    }

    pub fn select(&mut self, peer_id: u64) -> bool {
        if let Some(ref mut proc) = self.process {
            proc.select_option(peer_id)
        } else {
            false
        }
    }

    pub fn finish(&self, muted: bool, video: bool) -> Option<JoinInfo> {
        self.process.as_ref().map(|p| p.create_join_info(muted, video))
    }
}
