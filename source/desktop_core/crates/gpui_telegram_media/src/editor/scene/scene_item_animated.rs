//! Telegram Desktop 1:1 Parity Module: `SceneItemAnimatedScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_animated.cpp` (L1-L108)
//! - `Telegram/SourceFiles/editor/scene/scene_item_animated.h` (L1-L53)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AnimatedEntityKind {
    #[default]
    Sticker,
    Video,
    Message,
}

/// Authentic representation of animated graphics element state machine.
/// Upstream: `scene_item_animated.h:L15-L51` & `scene_item_animated.cpp:L30-L105`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemAnimatedScene {
    pub kind: AnimatedEntityKind,
    pub loop_duration_ms: u64,
    pub trim_from_ms: u64,
    pub trim_till_ms: u64,
    pub is_playing: bool,
    pub current_frame: usize,
    pub total_frames: usize,
    pub mirrored: bool,
}

impl Default for SceneItemAnimatedScene {
    fn default() -> Self {
        Self {
            kind: AnimatedEntityKind::Sticker,
            loop_duration_ms: 3000,
            trim_from_ms: 0,
            trim_till_ms: 3000,
            is_playing: true,
            current_frame: 0,
            total_frames: 60,
            mirrored: false,
        }
    }
}

impl SceneItemAnimatedScene {
    pub fn new(kind: AnimatedEntityKind, duration_ms: u64, frames: usize) -> Self {
        Self {
            kind,
            loop_duration_ms: duration_ms,
            trim_from_ms: 0,
            trim_till_ms: duration_ms,
            is_playing: true,
            current_frame: 0,
            total_frames: frames.max(1),
            mirrored: false,
        }
    }

    pub fn set_trim(&mut self, from_ms: u64, till_ms: u64) {
        let max_d = self.loop_duration_ms;
        self.trim_from_ms = from_ms.min(max_d);
        self.trim_till_ms = till_ms.clamp(self.trim_from_ms, max_d);
    }

    pub fn advance_frame(&mut self) -> usize {
        if self.is_playing && self.total_frames > 0 {
            self.current_frame = (self.current_frame + 1) % self.total_frames;
        }
        self.current_frame
    }

    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.mirrored = mirrored;
    }
}
