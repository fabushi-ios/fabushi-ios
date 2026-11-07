//! Calls Main UI Panel & Interaction Dock
//! Upstream: Telegram/SourceFiles/calls/calls_panel.h:L75-L155, calls_panel.cpp:L79-L250
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelActionState {
    Answer,
    Hangup,
    Redial,
    StartCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsPanel {
    pub call_id: i64,
    pub user_id: u64,
    pub action_state: PanelActionState,
    pub controls_visible: bool,
    pub is_muted: bool,
    pub is_camera_on: bool,
    pub is_screen_sharing: bool,
    pub is_minimized: bool,
    pub low_battery_warning: bool,
    pub autohide_timeout_ms: u64,
    pub last_interaction_ms: u64,
}

impl CallsPanel {
    pub const K_HIDE_CONTROLS_TIMEOUT_MS: u64 = 5000;
    pub const K_HIDE_CONTROLS_QUICK_TIMEOUT_MS: u64 = 2000;
    pub const K_NAME_FADE_DURATION_MS: u64 = 70;

    pub fn new(call_id: i64, user_id: u64, is_incoming: bool) -> Self {
        Self {
            call_id,
            user_id,
            action_state: if is_incoming {
                PanelActionState::Answer
            } else {
                PanelActionState::Hangup
            },
            controls_visible: true,
            is_muted: false,
            is_camera_on: false,
            is_screen_sharing: false,
            is_minimized: false,
            low_battery_warning: false,
            autohide_timeout_ms: Self::K_HIDE_CONTROLS_TIMEOUT_MS,
            last_interaction_ms: 0,
        }
    }

    pub fn user_activity(&mut self, now_ms: u64) {
        self.controls_visible = true;
        self.last_interaction_ms = now_ms;
    }

    pub fn check_autohide(&mut self, now_ms: u64) {
        if self.controls_visible
            && now_ms.saturating_sub(self.last_interaction_ms) >= self.autohide_timeout_ms
        {
            self.controls_visible = false;
        }
    }

    pub fn toggle_microphone(&mut self) -> bool {
        self.is_muted = !self.is_muted;
        self.is_muted
    }

    pub fn toggle_camera(&mut self) -> bool {
        self.is_camera_on = !self.is_camera_on;
        if self.is_camera_on {
            self.is_screen_sharing = false;
        }
        self.is_camera_on
    }

    pub fn toggle_screen_sharing(&mut self) -> bool {
        self.is_screen_sharing = !self.is_screen_sharing;
        if self.is_screen_sharing {
            self.is_camera_on = false;
        }
        self.is_screen_sharing
    }

    pub fn set_battery_low(&mut self, is_low: bool) {
        self.low_battery_warning = is_low;
    }

    pub fn set_action_state(&mut self, state: PanelActionState) {
        self.action_state = state;
    }
}
