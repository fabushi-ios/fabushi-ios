//! Telegram Desktop 1:1 Parity Module: `SceneItemVideoScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_video.cpp` (L1-L141)
//! - `Telegram/SourceFiles/editor/scene/scene_item_video.h` (L1-L54)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream playback frame side: `scene_item_video.cpp:L16`
pub const K_PLAYBACK_FRAME_SIDE: u32 = 512;

/// Scales video thumbnail to fit within 512px box while preserving aspect ratio.
/// Upstream: `scene_item_video.cpp:L18-L27`
pub fn frame_size_for(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (K_PLAYBACK_FRAME_SIDE, K_PLAYBACK_FRAME_SIDE);
    }
    if width <= K_PLAYBACK_FRAME_SIDE && height <= K_PLAYBACK_FRAME_SIDE {
        return (width, height);
    }
    let scale = (K_PLAYBACK_FRAME_SIDE as f64) / ((width.max(height)) as f64);
    (
        ((width as f64) * scale).round() as u32,
        ((height as f64) * scale).round() as u32,
    )
}

/// Authentic representation of embedded video item in graphics scene.
/// Upstream: `scene_item_video.h:L15-L53` & `scene_item_video.cpp:L31-L140`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemVideoScene {
    pub video_id: u64,
    pub path: String,
    pub duration_ms: u64,
    pub trim_from_ms: u64,
    pub trim_till_ms: u64,
    pub volume: f64,
    pub sound_enabled: bool,
    pub frame_size: (u32, u32),
    pub flipped: bool,
}

impl Default for SceneItemVideoScene {
    fn default() -> Self {
        Self::new(1, "video.mp4".to_string(), 10000, 1920, 1080)
    }
}

impl SceneItemVideoScene {
    pub fn new(id: u64, path: String, duration_ms: u64, orig_w: u32, orig_h: u32) -> Self {
        let frame_size = frame_size_for(orig_w, orig_h);
        Self {
            video_id: id,
            path,
            duration_ms,
            trim_from_ms: 0,
            trim_till_ms: duration_ms,
            volume: 1.0,
            sound_enabled: true,
            frame_size,
            flipped: false,
        }
    }

    pub fn set_trim(&mut self, from_ms: u64, till_ms: u64) {
        self.trim_from_ms = from_ms.min(self.duration_ms);
        self.trim_till_ms = till_ms.clamp(self.trim_from_ms, self.duration_ms);
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }
}
