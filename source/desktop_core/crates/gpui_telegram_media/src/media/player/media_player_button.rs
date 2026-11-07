//! Telegram Desktop 1:1 Parity Submodule: `media_player_button.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Player Control Buttons & Morphing Icon Layout)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_button.h:L25-L60`
//!           `Telegram/SourceFiles/media/player/media_player_button.cpp:L20-L100`

use serde::{Deserialize, Serialize};

/// Upstream: `media_player_button.h:L27-L31`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayButtonState {
    #[default]
    Play,
    Pause,
    Cancel,
}

/// Speed label text formatting
/// Upstream: `media_player_button.cpp:L29-L31`
pub fn format_speed_text(speed: f64) -> String {
    let rounded = (speed * 10.0).round() / 10.0;
    format!("{:.1}X", rounded).replace(".0X", "X")
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerButtonPlayer {
    pub state: PlayButtonState,
    pub speed: f64,
    pub is_hovered: bool,
    pub is_pressed: bool,
}

impl MediaPlayerButtonPlayer {
    pub fn new() -> Self {
        Self {
            state: PlayButtonState::Play,
            speed: 1.0,
            is_hovered: false,
            is_pressed: false,
        }
    }

    pub fn set_state(&mut self, state: PlayButtonState) {
        self.state = state;
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.5);
    }

    pub fn speed_label(&self) -> String {
        format_speed_text(self.speed)
    }

    pub fn toggle_play_pause(&mut self) -> PlayButtonState {
        self.state = match self.state {
            PlayButtonState::Play => PlayButtonState::Pause,
            PlayButtonState::Pause => PlayButtonState::Play,
            PlayButtonState::Cancel => PlayButtonState::Play,
        };
        self.state
    }
}
