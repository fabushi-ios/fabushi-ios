//! Telegram Desktop 1:1 Parity Submodule: `media_player_widget.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Top Player Header Widget)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_widget.h:L47-L80`
//!           `Telegram/SourceFiles/media/player/media_player_widget.cpp:L44-L100`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerRepeatState {
    #[default]
    None,
    One,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerOrderState {
    #[default]
    Default,
    Reverse,
    Shuffle,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerWidgetPlayer {
    pub is_visible: bool,
    pub track_title: String,
    pub track_performer: String,
    pub current_position_ms: i64,
    pub total_duration_ms: i64,
    pub repeat_state: PlayerRepeatState,
    pub order_state: PlayerOrderState,
    pub is_playing: bool,
    pub speed: f64,
    pub volume: f64,
}

impl MediaPlayerWidgetPlayer {
    pub fn new() -> Self {
        Self {
            is_visible: false,
            track_title: String::new(),
            track_performer: String::new(),
            current_position_ms: 0,
            total_duration_ms: 0,
            repeat_state: PlayerRepeatState::None,
            order_state: PlayerOrderState::Default,
            is_playing: false,
            speed: 1.0,
            volume: 1.0,
        }
    }

    pub fn set_track(&mut self, title: String, performer: String, duration_ms: i64) {
        self.track_title = title;
        self.track_performer = performer;
        self.total_duration_ms = duration_ms;
        self.current_position_ms = 0;
        self.is_visible = true;
    }

    pub fn cycle_repeat_state(&mut self) -> PlayerRepeatState {
        self.repeat_state = match self.repeat_state {
            PlayerRepeatState::None => PlayerRepeatState::All,
            PlayerRepeatState::All => PlayerRepeatState::One,
            PlayerRepeatState::One => PlayerRepeatState::None,
        };
        self.repeat_state
    }

    pub fn cycle_order_state(&mut self) -> PlayerOrderState {
        self.order_state = match self.order_state {
            PlayerOrderState::Default => PlayerOrderState::Reverse,
            PlayerOrderState::Reverse => PlayerOrderState::Shuffle,
            PlayerOrderState::Shuffle => PlayerOrderState::Default,
        };
        self.order_state
    }

    pub fn progress_ratio(&self) -> f64 {
        if self.total_duration_ms > 0 {
            (self.current_position_ms as f64 / self.total_duration_ms as f64).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn formatted_time(&self) -> String {
        let cur_secs = self.current_position_ms / 1000;
        let tot_secs = self.total_duration_ms / 1000;
        format!(
            "{:02}:{:02} / {:02}:{:02}",
            cur_secs / 60,
            cur_secs % 60,
            tot_secs / 60,
            tot_secs % 60
        )
    }
}
