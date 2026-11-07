//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupSettingsGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_settings.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_settings.cpp:L64-L135`

use serde::{Deserialize, Serialize};

/// Maximum delay index count upstream (0..200 -> 0..2000ms)
pub const K_DELAYS_COUNT: usize = 201;
pub const K_MICROPHONE_TOOLTIP_AFTER_LOUD_COUNT: u32 = 3;
pub const K_DROP_LOUD_AFTER_QUIET_COUNT: u32 = 5;
pub const K_MICROPHONE_TOOLTIP_LEVEL_THRESHOLD: f32 = 0.2;
pub const K_MICROPHONE_TOOLTIP_CHECK_INTERVAL_MS: u64 = 500;

/// Map delay slider index (0..200) to milliseconds (0..2000ms)
/// calls_group_settings.cpp:L121-L123: DelayByIndex
pub fn delay_by_index(index: usize) -> u32 {
    let clamped = index.min(K_DELAYS_COUNT - 1);
    (clamped as u32) * 10
}

/// Format delay in milliseconds to localized string
/// calls_group_settings.cpp:L125-L135: FormatDelay
pub fn format_delay(delay_ms: u32) -> String {
    if delay_ms < 1000 {
        format!("{} ms", delay_ms)
    } else {
        format!("{:.2} s", (delay_ms as f64) / 1000.0)
    }
}

/// Upstream MicLevelTester for detecting speaking while muted
/// calls_group_settings.h:L31-L47, calls_group_settings.cpp:L64-L72
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicLevelTester {
    pub loud_count: u32,
    pub quiet_count: u32,
    pub tooltip_shown: bool,
}

impl Default for MicLevelTester {
    fn default() -> Self {
        Self {
            loud_count: 0,
            quiet_count: 0,
            tooltip_shown: false,
        }
    }
}

impl MicLevelTester {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process an audio input level sample [0.0..1.0] while muted
    pub fn update_sample(&mut self, level: f32) -> bool {
        if level >= K_MICROPHONE_TOOLTIP_LEVEL_THRESHOLD {
            self.loud_count = self.loud_count.saturating_add(1);
            self.quiet_count = 0;
            if self.loud_count >= K_MICROPHONE_TOOLTIP_AFTER_LOUD_COUNT && !self.tooltip_shown {
                self.tooltip_shown = true;
                return true;
            }
        } else {
            self.quiet_count = self.quiet_count.saturating_add(1);
            if self.quiet_count >= K_DROP_LOUD_AFTER_QUIET_COUNT {
                self.loud_count = 0;
                self.tooltip_shown = false;
            }
        }
        false
    }

    pub fn reset(&mut self) {
        self.loud_count = 0;
        self.quiet_count = 0;
        self.tooltip_shown = false;
    }
}

/// Group Call Settings State & Controller
/// Target Symbol: `CallsGroupSettingsGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupSettingsGroup {
    pub input_device_id: String,
    pub output_device_id: String,
    pub noise_suppression: bool,
    pub echo_cancellation: bool,
    pub push_to_talk: bool,
    pub ptt_shortcut: String,
    pub ptt_delay_ms: u32,
    pub join_muted: bool,
    pub messages_enabled: bool,
    pub invite_link_speaker: String,
    pub invite_link_listener: String,
}

impl Default for CallsGroupSettingsGroup {
    fn default() -> Self {
        Self {
            input_device_id: "default".to_string(),
            output_device_id: "default".to_string(),
            noise_suppression: true,
            echo_cancellation: true,
            push_to_talk: false,
            ptt_shortcut: "Space".to_string(),
            ptt_delay_ms: 200, // 20 * 10ms
            join_muted: false,
            messages_enabled: true,
            invite_link_speaker: String::new(),
            invite_link_listener: String::new(),
        }
    }
}

impl CallsGroupSettingsGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_ptt_delay_index(&mut self, index: usize) {
        self.ptt_delay_ms = delay_by_index(index);
    }

    pub fn get_formatted_ptt_delay(&self) -> String {
        format_delay(self.ptt_delay_ms)
    }

    pub fn toggle_join_muted(&mut self) -> bool {
        self.join_muted = !self.join_muted;
        self.join_muted
    }

    pub fn toggle_messages_enabled(&mut self) -> bool {
        self.messages_enabled = !self.messages_enabled;
        self.messages_enabled
    }

    pub fn invite_link(&self, for_speaker: bool) -> &str {
        if for_speaker && !self.invite_link_speaker.is_empty() {
            &self.invite_link_speaker
        } else {
            &self.invite_link_listener
        }
    }
}
