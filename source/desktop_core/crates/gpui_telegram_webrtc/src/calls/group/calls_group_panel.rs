//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupPanelGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_panel.h (L1-L293)
//!   - Telegram/SourceFiles/calls/group/calls_group_panel.cpp (L1-L3135)

use super::calls_group_common::{PanelMode, StickedTooltip};
use serde::{Deserialize, Serialize};

/// Spacebar push-to-talk release delay threshold to prevent micro-cutoffs.
/// Upstream: `calls_group_panel.cpp:L82`
pub const K_SPACE_PUSH_TO_TALK_DELAY_MS: u64 = 250;

/// Recording indicator pulsing cycle duration.
/// Upstream: `calls_group_panel.cpp:L83`
pub const K_RECORDING_ANIMATION_DURATION_MS: u64 = 1200;

/// Recording indicator opacity.
/// Upstream: `calls_group_panel.cpp:L84`
pub const K_RECORDING_OPACITY: f32 = 0.6;

/// Controls dock background opacity.
/// Upstream: `calls_group_panel.cpp:L86`
pub const K_CONTROLS_BACKGROUND_OPACITY: f32 = 0.8;

/// Override active color alpha channel.
/// Upstream: `calls_group_panel.cpp:L87`
pub const K_OVERRIDE_ACTIVE_COLOR_BG_ALPHA: u8 = 172;

/// Controls bar button action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelButton {
    Mute,
    Video,
    ScreenShare,
    Leave,
    Settings,
}

/// Parity component for `CallsGroupPanelGroup`.
#[derive(Debug, Clone)]
pub struct CallsGroupPanelGroup {
    pub title: String,
    pub participant_count: usize,
    pub mode: PanelMode,
    pub is_muted: bool,
    pub is_video_enabled: bool,
    pub is_screen_sharing: bool,
    pub is_recording: bool,
    pub sticked_tooltip: StickedTooltip,
    pub space_pressed: bool,
    pub space_release_timer_ms: Option<u64>,
}

impl Default for CallsGroupPanelGroup {
    fn default() -> Self {
        Self {
            title: "Voice Chat".to_string(),
            participant_count: 0,
            mode: PanelMode::Normal,
            is_muted: true,
            is_video_enabled: false,
            is_screen_sharing: false,
            is_recording: false,
            sticked_tooltip: StickedTooltip::None,
            space_pressed: false,
            space_release_timer_ms: None,
        }
    }
}

impl CallsGroupPanelGroup {
    pub fn new(title: &str, participant_count: usize) -> Self {
        Self {
            title: title.to_string(),
            participant_count,
            ..Default::default()
        }
    }

    pub fn set_mode(&mut self, mode: PanelMode) {
        self.mode = mode;
    }

    pub fn toggle_mute(&mut self) -> bool {
        self.is_muted = !self.is_muted;
        self.is_muted
    }

    pub fn toggle_video(&mut self) -> bool {
        self.is_video_enabled = !self.is_video_enabled;
        self.is_video_enabled
    }

    pub fn toggle_screen_share(&mut self) -> bool {
        self.is_screen_sharing = !self.is_screen_sharing;
        self.is_screen_sharing
    }

    /// Handles spacebar press for Push-to-Talk.
    /// Unmutes mic immediately and cancels any pending release timer.
    pub fn on_space_down(&mut self) {
        self.space_pressed = true;
        self.space_release_timer_ms = None;
        self.is_muted = false;
    }

    /// Handles spacebar release for Push-to-Talk.
    /// Initiates the 250ms delay timer before muting mic.
    pub fn on_space_up(&mut self) {
        self.space_pressed = false;
        self.space_release_timer_ms = Some(K_SPACE_PUSH_TO_TALK_DELAY_MS);
    }

    /// Advance timer by `delta_ms`. If release timer expires, re-mutes microphone.
    pub fn tick_timer(&mut self, delta_ms: u64) {
        if let Some(timer) = self.space_release_timer_ms {
            if delta_ms >= timer {
                self.space_release_timer_ms = None;
                if !self.space_pressed {
                    self.is_muted = true;
                }
            } else {
                self.space_release_timer_ms = Some(timer - delta_ms);
            }
        }
    }
}
