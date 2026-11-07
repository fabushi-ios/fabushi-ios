//! Telegram Desktop 1:1 Parity Submodule: `EditorAudioDiscButton`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_audio_disc_button.h:L1-L58`, `Telegram/SourceFiles/editor/editor_audio_disc_button.cpp:L1-L155`

use super::photo_editor_common::AudioTrack;
use serde::{Deserialize, Serialize};

/// Duration of a single 360-degree rotation of the vinyl disc.
/// Upstream: `editor_audio_disc_button.cpp:L23`
pub const SPIN_DURATION_MS: i64 = 4000;

/// Computes the spinning disc angle in degrees.
/// Upstream: `editor_audio_disc_button.cpp:L87-L93`
#[inline]
pub fn angle_at_time(spin_base: f64, spin_started_ms: i64, now_ms: i64, is_active: bool) -> f64 {
    if !is_active {
        spin_base
    } else {
        let passed = (now_ms - spin_started_ms).max(0);
        (spin_base + (passed as f64 * 360.0) / SPIN_DURATION_MS as f64) % 360.0
    }
}

/// Target symbol EditorAudioDiscButton representing rotating music disc icon on media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorAudioDiscButton {
    pub track: Option<AudioTrack>,
    pub is_active: bool,
    pub spin_base: f64,
    pub spin_started_ms: i64,
}

pub type AudioDiscButton = EditorAudioDiscButton;

impl Default for EditorAudioDiscButton {
    fn default() -> Self {
        Self {
            track: None,
            is_active: false,
            spin_base: 0.0,
            spin_started_ms: 0,
        }
    }
}

impl EditorAudioDiscButton {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_track(&mut self, track: Option<AudioTrack>) {
        self.track = track;
    }

    pub fn set_active(&mut self, active: bool, now_ms: i64) {
        if self.is_active == active {
            return;
        }
        self.spin_base = self.current_angle(now_ms);
        self.is_active = active;
        self.spin_started_ms = now_ms;
    }

    pub fn current_angle(&self, now_ms: i64) -> f64 {
        angle_at_time(self.spin_base, self.spin_started_ms, now_ms, self.is_active)
    }
}
