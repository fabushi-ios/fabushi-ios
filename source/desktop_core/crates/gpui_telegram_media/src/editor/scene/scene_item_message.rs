//! Telegram Desktop 1:1 Parity Module: `SceneItemMessageScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_message.cpp` (L1-L499)
//! - `Telegram/SourceFiles/editor/scene/scene_item_message.h` (L1-L121)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream maximum ratio: `scene_item_message.cpp:L32`
pub const K_MAX_RATIO: i32 = 8;

/// Upstream playback frame side: `scene_item_message.cpp:L33`
pub const K_PLAYBACK_FRAME_SIDE: i32 = 1024;

/// Computes centered cover rectangle preserving aspect ratio.
/// Upstream: `scene_item_message.cpp:L43-L56`
pub fn compute_cover_rect(hole: (f64, f64, f64, f64), video_w: f64, video_h: f64) -> (f64, f64, f64, f64) {
    let (hx, hy, hw, hh) = hole;
    if video_w <= 0.0 || video_h <= 0.0 || hw <= 0.0 || hh <= 0.0 {
        return hole;
    }
    let scale = (hw / video_w).max(hh / video_h);
    let sw = (video_w * scale).ceil();
    let sh = (video_h * scale).ceil();
    let cx = hx + hw / 2.0;
    let cy = hy + hh / 2.0;
    (cx - sw / 2.0, cy - sh / 2.0, sw, sh)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageVideoOptions {
    pub play: bool,
    pub sound: bool,
}

/// Authentic representation of forwarded quote message sticker scene item.
/// Upstream: `scene_item_message.h:L28-L120` & `scene_item_message.cpp:L34-L495`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemMessageScene {
    pub message_id: u64,
    pub sender_name: String,
    pub snippet_text: String,
    pub is_dark: bool,
    pub video_options: MessageVideoOptions,
    pub width: f64,
    pub height: f64,
}

impl Default for SceneItemMessageScene {
    fn default() -> Self {
        Self {
            message_id: 1,
            sender_name: "Alice".to_string(),
            snippet_text: "Forwarded story message".to_string(),
            is_dark: false,
            video_options: MessageVideoOptions { play: false, sound: false },
            width: 320.0,
            height: 96.0,
        }
    }
}

impl SceneItemMessageScene {
    pub fn new(id: u64, sender: String, text: String, dark: bool) -> Self {
        Self {
            message_id: id,
            sender_name: sender,
            snippet_text: text,
            is_dark: dark,
            video_options: MessageVideoOptions { play: false, sound: false },
            width: 320.0,
            height: 96.0,
        }
    }

    pub fn set_video_playback(&mut self, play: bool, sound: bool) {
        self.video_options.play = play;
        self.video_options.sound = sound;
    }
}
