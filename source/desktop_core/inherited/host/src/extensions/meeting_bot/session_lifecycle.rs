//! OpenClaw Meeting Bot Session Lifecycle, Modes & Cleanup Tracking
//!
//! Mirrors:
//! - `src/meeting-bot/meeting-modes.ts`
//! - `src/meeting-bot/session-cleanup-tracker.ts`
//! - `src/meeting-bot/session-types.ts`
//! - `src/meeting-bot/realtime-agent-engine.ts`
//! - `src/meeting-bot/participation-browser.ts`
//! - `src/meeting-bot/status-call-ownership-source.ts`
//! - `src/meeting-bot/agent-consult.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub fn is_meeting_talk_back_mode(mode: &str) -> bool {
    mode == "agent" || mode == "bidi"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingBrowserHealth {
    pub in_call: bool,
    pub mic_muted: bool,
    pub audio_input_routed: bool,
    pub audio_output_routed: bool,
    pub manual_action: Option<String>,
}

pub fn is_meeting_realtime_route_ready(mode: &str, health: &MeetingBrowserHealth) -> bool {
    is_meeting_talk_back_mode(mode)
        && health.in_call
        && !health.mic_muted
        && health.audio_input_routed
        && health.audio_output_routed
        && health.manual_action.is_none()
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MeetingSessionCleanupState {
    pub browser_left: bool,
    pub browser_settled: bool,
    pub stop_settled: bool,
    pub unpublished: bool,
}

impl MeetingSessionCleanupState {
    pub fn is_fully_cleaned(&self) -> bool {
        self.browser_settled && self.stop_settled && self.unpublished
    }
}

#[derive(Debug, Default)]
pub struct MeetingSessionCleanupTracker {
    states: HashMap<String, MeetingSessionCleanupState>,
}

impl MeetingSessionCleanupTracker {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    pub fn register_session(&mut self, session_id: impl Into<String>) {
        self.states.entry(session_id.into()).or_default();
    }

    pub fn mark_browser_left(&mut self, session_id: &str) {
        if let Some(state) = self.states.get_mut(session_id) {
            state.browser_left = true;
            state.browser_settled = true;
        }
    }

    pub fn mark_stop_settled(&mut self, session_id: &str) {
        if let Some(state) = self.states.get_mut(session_id) {
            state.stop_settled = true;
        }
    }

    pub fn mark_unpublished(&mut self, session_id: &str) {
        if let Some(state) = self.states.get_mut(session_id) {
            state.unpublished = true;
        }
    }

    pub fn is_cleanup_complete(&self, session_id: &str) -> bool {
        self.states.get(session_id).map_or(false, |s| s.is_fully_cleaned())
    }
}
