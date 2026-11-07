//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMembersRowGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_members_row.h (L1-L230)
//!   - Telegram/SourceFiles/calls/group/calls_group_members_row.cpp (L1-L822)

use super::calls_group_common::{MuteState, ParticipantState};
use serde::{Deserialize, Serialize};

/// Animation duration for audio level interpolation.
/// Upstream: `calls_group_members_row.cpp:L26`
pub const K_LEVEL_DURATION_MS: f64 = 100.0 + 500.0 * 0.23;

/// Primary speaking blob expansion scale factor.
/// Upstream: `calls_group_members_row.cpp:L27`
pub const K_BLOB_SCALE: f64 = 0.605;

/// Secondary minor blob scale factor.
/// Upstream: `calls_group_members_row.cpp:L28`
pub const K_MINOR_BLOB_FACTOR: f32 = 0.9;

/// Minimum scale for userpic avatar when audio level is high.
/// Upstream: `calls_group_members_row.cpp:L29`
pub const K_USERPIC_MIN_SCALE: f64 = 0.8;

/// Maximum audio level ceiling.
/// Upstream: `calls_group_members_row.cpp:L30`
pub const K_MAX_LEVEL: f64 = 1.0;

/// Wide mode layout multiplier.
/// Upstream: `calls_group_members_row.cpp:L31`
pub const K_WIDE_SCALE: i32 = 5;

/// Visual presentation style for members row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MembersRowStyle {
    #[default]
    Normal,
    Compact,
    Wide,
}

/// Compute speaking blob pulsation radii.
/// Upstream: `calls_group_members_row.cpp:L380-L410`
pub fn compute_speaking_blob_radii(base_radius: f32, level: f32, phase: f32) -> (f32, f32) {
    let clamped_level = level.clamp(0.0, K_MAX_LEVEL as f32);
    let scale = K_BLOB_SCALE as f32;
    let wave = 1.0 + 0.15 * phase.sin();
    let primary = base_radius * (1.0 + scale * clamped_level * wave);
    let minor = base_radius * (1.0 + scale * K_MINOR_BLOB_FACTOR * clamped_level * wave);
    (primary, minor)
}

/// Parity component representing an individual member row.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupMembersRowGroup {
    pub participant: ParticipantState,
    pub style: MembersRowStyle,
    pub is_selected: bool,
    pub is_hovered: bool,
    pub animated_level: f32,
    pub animation_phase: f32,
}

impl CallsGroupMembersRowGroup {
    pub fn new(participant: ParticipantState) -> Self {
        Self {
            participant,
            style: MembersRowStyle::Normal,
            is_selected: false,
            is_hovered: false,
            animated_level: 0.0,
            animation_phase: 0.0,
        }
    }

    pub fn update_level(&mut self, target_level: f32, delta_ms: f32) {
        let alpha = (delta_ms / K_LEVEL_DURATION_MS as f32).clamp(0.0, 1.0);
        self.animated_level += (target_level - self.animated_level) * alpha;
        self.animation_phase += delta_ms * 0.005;
        if self.animation_phase > std::f32::consts::PI * 2.0 {
            self.animation_phase -= std::f32::consts::PI * 2.0;
        }
    }

    pub fn get_blob_radii(&self, base_radius: f32) -> (f32, f32) {
        if self.participant.mute_state == MuteState::Speaking {
            compute_speaking_blob_radii(base_radius, self.animated_level, self.animation_phase)
        } else {
            (base_radius, base_radius)
        }
    }

    pub fn status_text(&self) -> String {
        match self.participant.mute_state {
            MuteState::Speaking => "Speaking".to_string(),
            MuteState::RaisedHand => "Wants to speak".to_string(),
            MuteState::Unmuted => "Listening".to_string(),
            MuteState::MutedByMe => format!("Muted ({}%)", self.participant.volume / 100),
            MuteState::MutedByAdmin => "Muted by admin".to_string(),
        }
    }
}
