//! Telegram Desktop 1:1 Parity Submodule: `EditorAudioMenu`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_audio_menu.h:L1-L81`, `Telegram/SourceFiles/editor/editor_audio_menu.cpp:L1-L229`

use serde::{Deserialize, Serialize};

pub const MUTE_DURATION_MS: i64 = 150;
pub const SPEAKER_THRESHOLDS: [f64; 3] = [0.05, 0.25, 0.75];
pub const VOLUME_STICKIED_VALUES: [(f64, f64); 3] = [
    (0.25, 0.02),
    (0.50, 0.02),
    (0.75, 0.02),
];

/// Applies magnetic snapping to volume slider when close to 25%, 50%, or 75%.
/// Upstream: `editor_audio_menu.cpp:L29-L34`
#[inline]
pub fn apply_volume_sticking(value: f64) -> f64 {
    for (target, delta) in VOLUME_STICKIED_VALUES {
        if (value - target).abs() <= delta {
            return target;
        }
    }
    value.clamp(0.0, 1.0)
}

/// Returns the number of speaker sound waves visible (0 to 3).
/// Upstream: `editor_audio_menu.cpp:L27`
#[inline]
pub fn speaker_arc_count(volume: f64) -> usize {
    if volume <= SPEAKER_THRESHOLDS[0] {
        0
    } else if volume <= SPEAKER_THRESHOLDS[1] {
        1
    } else if volume <= SPEAKER_THRESHOLDS[2] {
        2
    } else {
        3
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuVolumeItem {
    pub volume: f64,
    pub is_muted: bool,
}

impl Default for MenuVolumeItem {
    fn default() -> Self {
        Self {
            volume: 1.0,
            is_muted: false,
        }
    }
}

/// Target symbol EditorAudioMenu managing audio volume slider and track removal action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorAudioMenu {
    pub volume_item: MenuVolumeItem,
    pub is_open: bool,
    pub track_removed: bool,
}

impl Default for EditorAudioMenu {
    fn default() -> Self {
        Self {
            volume_item: MenuVolumeItem::default(),
            is_open: false,
            track_removed: false,
        }
    }
}

impl EditorAudioMenu {
    pub fn new(volume: f64) -> Self {
        Self {
            volume_item: MenuVolumeItem {
                volume: volume.clamp(0.0, 1.0),
                is_muted: volume <= 0.0,
            },
            is_open: false,
            track_removed: false,
        }
    }

    pub fn set_volume(&mut self, raw_value: f64) -> f64 {
        let snapped = apply_volume_sticking(raw_value);
        self.volume_item.volume = snapped;
        self.volume_item.is_muted = snapped <= 0.0;
        snapped
    }

    pub fn speaker_waves(&self) -> usize {
        speaker_arc_count(self.volume_item.volume)
    }

    pub fn remove_track(&mut self) {
        self.track_removed = true;
        self.is_open = false;
    }
}
