//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupToastsGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_toasts.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_toasts.cpp:L24-L181`

use serde::{Deserialize, Serialize};

/// Upstream error toast display duration (2000ms)
/// calls_group_toasts.cpp:L24
pub const K_ERROR_DURATION_MS: u64 = 2000;

/// Upstream group call error types
/// calls_group_toasts.cpp:L162-L177
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupCallError {
    NoCamera,
    CameraFailed,
    ScreenFailed,
    MutedNoCamera,
    MutedNoScreen,
    DisabledNoCamera,
    DisabledNoScreen,
}

impl GroupCallError {
    pub fn translation_key(&self) -> &'static str {
        match self {
            Self::NoCamera => "lng_call_error_no_camera",
            Self::CameraFailed => "lng_group_call_failed_camera",
            Self::ScreenFailed => "lng_group_call_failed_screen",
            Self::MutedNoCamera => "lng_group_call_muted_no_camera",
            Self::MutedNoScreen => "lng_group_call_muted_no_screen",
            Self::DisabledNoCamera => "lng_group_call_chat_no_camera",
            Self::DisabledNoScreen => "lng_group_call_chat_no_screen",
        }
    }
}

/// Video pinned toast selector distinguishing camera/screen and self/peer
/// calls_group_toasts.cpp:L121-L142: setupPinnedVideo
pub fn pinned_video_toast_key(is_camera: bool, is_me: bool, is_pinned: bool) -> &'static str {
    match (is_camera, is_me, is_pinned) {
        (true, true, true) => "lng_group_call_pinned_camera_me",
        (true, true, false) => "lng_group_call_unpinned_camera_me",
        (false, true, true) => "lng_group_call_pinned_screen_me",
        (false, true, false) => "lng_group_call_unpinned_screen_me",
        (true, false, true) => "lng_group_call_pinned_camera",
        (true, false, false) => "lng_group_call_unpinned_camera",
        (false, false, true) => "lng_group_call_pinned_screen",
        (false, false, false) => "lng_group_call_unpinned_screen",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveToast {
    pub text: String,
    pub duration_ms: u64,
    pub is_error: bool,
}

/// Group Call Toasts Manager
/// Target Symbol: `CallsGroupToastsGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CallsGroupToastsGroup {
    pub active_toasts: Vec<ActiveToast>,
    pub current_title: String,
    pub current_join_as: String,
    pub has_raised_hand: bool,
}

impl CallsGroupToastsGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_error(&mut self, error: GroupCallError) {
        self.active_toasts.push(ActiveToast {
            text: error.translation_key().to_string(),
            duration_ms: K_ERROR_DURATION_MS,
            is_error: true,
        });
    }

    pub fn on_rejoin_as_changed(&mut self, new_name: &str, is_broadcast: bool) {
        if self.current_join_as != new_name {
            self.current_join_as = new_name.to_string();
            let key = if is_broadcast {
                "lng_group_call_join_as_changed_channel"
            } else {
                "lng_group_call_join_as_changed"
            };
            self.active_toasts.push(ActiveToast {
                text: format!("{}: {}", key, new_name),
                duration_ms: 3000,
                is_error: false,
            });
        }
    }

    pub fn on_title_changed(&mut self, new_title: &str, is_broadcast: bool) {
        if self.current_title != new_title {
            self.current_title = new_title.to_string();
            let key = if is_broadcast {
                "lng_group_call_title_changed_channel"
            } else {
                "lng_group_call_title_changed"
            };
            self.active_toasts.push(ActiveToast {
                text: format!("{}: {}", key, new_title),
                duration_ms: 3000,
                is_error: false,
            });
        }
    }

    pub fn on_mute_state_changed(&mut self, was_force_muted: bool, now_raised_hand: bool) {
        if was_force_muted && now_raised_hand {
            self.has_raised_hand = true;
            self.active_toasts.push(ActiveToast {
                text: "lng_group_call_tooltip_raised_hand".to_string(),
                duration_ms: 3000,
                is_error: false,
            });
        }
    }

    pub fn on_pinned_video_toggled(&mut self, is_camera: bool, is_me: bool, is_pinned: bool, peer_name: &str) {
        let key = pinned_video_toast_key(is_camera, is_me, is_pinned);
        let text = if is_me {
            key.to_string()
        } else {
            format!("{}: {}", key, peer_name)
        };
        self.active_toasts.push(ActiveToast {
            text,
            duration_ms: 3000,
            is_error: false,
        });
    }

    pub fn clear_expired(&mut self) {
        self.active_toasts.clear();
    }
}
