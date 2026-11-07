//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditorContent`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor_content.h:L1-L128`, `Telegram/SourceFiles/editor/photo_editor_content.cpp:L1-L631`

use super::editor_crop::EditorCrop;
use super::editor_paint::EditorPaint;
use super::photo_editor_common::{AudioTrack, PhotoModifications};
use super::photo_editor_inner_common::{Brush, EditorMode, PhotoEditorMode};
use serde::{Deserialize, Serialize};

pub const EXPANSION_ROOM_RATIO: f64 = 0.70;
pub const LAYOUT_SMOOTH_TAU: f64 = 60.0;
pub const LAYOUT_MAX_FRAME_DELTA_MS: i64 = 64;
pub const LAYOUT_EPSILON: f64 = 0.5;

/// Computes the Chebyshev distance between two rects (max coordinate delta).
/// Upstream: `photo_editor_content.cpp:L35-L42`
#[inline]
pub fn max_distance(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    (a.0 - b.0).abs()
        .max((a.1 - b.1).abs())
        .max((a.2 - b.2).abs())
        .max((a.3 - b.3).abs())
}

/// Target symbol PhotoEditorContent coordinating canvas layout, crop frame, paint engine, and audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoEditorContent {
    pub photo_width: u32,
    pub photo_height: u32,
    pub crop: EditorCrop,
    pub paint: EditorPaint,
    pub audio_track: Option<AudioTrack>,
    pub mode: PhotoEditorMode,
    pub expansion_room: bool,
    pub is_audio_selected: bool,
    pub durations_linked: bool,
}

impl Default for PhotoEditorContent {
    fn default() -> Self {
        Self::new(1280, 720)
    }
}

impl PhotoEditorContent {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            photo_width: width,
            photo_height: height,
            crop: EditorCrop::new(width as f64, height as f64),
            paint: EditorPaint::new(width, height),
            audio_track: None,
            mode: PhotoEditorMode {
                mode: EditorMode::Transform,
                action: Default::default(),
            },
            expansion_room: false,
            is_audio_selected: false,
            durations_linked: false,
        }
    }

    pub fn apply_mode(&mut self, mode: PhotoEditorMode) {
        self.mode = mode;
    }

    pub fn apply_brush(&mut self, brush: Brush) {
        self.paint.apply_brush(brush);
    }

    pub fn set_audio_track(&mut self, track: AudioTrack) {
        self.audio_track = Some(track);
    }

    pub fn remove_audio(&mut self) {
        self.audio_track = None;
        self.is_audio_selected = false;
    }

    pub fn save_modifications(&self) -> PhotoModifications {
        PhotoModifications {
            angle: self.crop.angle,
            flipped: self.crop.flipped,
            crop_x: self.crop.crop.x.round() as i32,
            crop_y: self.crop.crop.y.round() as i32,
            crop_width: self.crop.crop.width.round() as i32,
            crop_height: self.crop.crop.height.round() as i32,
            crop_type: Default::default(),
            crop_mode: Default::default(),
            corners_level: self.crop.corners_level,
        }
    }

    /// Computes target rect centered in canvas with optional expansion room for controls.
    /// Upstream: `photo_editor_content.cpp:L87-L91`
    pub fn layout_target(
        &self,
        canvas_w: f64,
        canvas_h: f64,
        room: bool,
    ) -> (f64, f64, f64, f64) {
        let avail_h = if room {
            canvas_h * EXPANSION_ROOM_RATIO
        } else {
            canvas_h
        };
        let (img_w, img_h) = (self.photo_width as f64, self.photo_height as f64);
        let scale = (canvas_w / img_w).min(avail_h / img_h);
        let target_w = img_w * scale;
        let target_h = img_h * scale;
        let target_x = (canvas_w - target_w) / 2.0;
        let target_y = (avail_h - target_h) / 2.0;
        (target_x, target_y, target_w, target_h)
    }
}
