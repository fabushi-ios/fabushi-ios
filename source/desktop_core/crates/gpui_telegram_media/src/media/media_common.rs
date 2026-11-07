//! Telegram Desktop 1:1 Parity Submodule: `MediaCommon`
//! Upstream Source: `Telegram/SourceFiles/media/media_common.h:L1-L61`
//! Subsystem: `media` | Feature: `TCG-N08`

pub const K_SPEED_MIN: f64 = 0.5;
pub const K_SPEED_MAX: f64 = 2.5;
pub const K_SPED_UP_DEFAULT: f64 = 1.7;

/// Media playback repeat mode corresponding to `Media::RepeatMode` (media_common.h:L17-L21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    None,
    One,
    All,
}

/// Media playlist ordering mode corresponding to `Media::OrderMode` (media_common.h:L23-L27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrderMode {
    #[default]
    Default,
    Reverse,
    Shuffle,
}

/// Video quality settings corresponding to `Media::VideoQuality` (media_common.h:L29-L40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoQuality {
    pub manual: bool,
    pub height: u32,
    pub original: bool,
}

impl VideoQuality {
    pub fn new(height: u32, manual: bool, original: bool) -> Self {
        Self {
            manual,
            height,
            original,
        }
    }
}

/// Validates whether frame dimensions are within area limit (media_common.h:L46-L54).
pub fn valid_frame_size(w: i32, h: i32, max_area: i64) -> bool {
    w > 0 && h > 0 && (w as i64 * h as i64 <= max_area)
}

/// Compares two playback speeds rounded to 1 decimal place (media_common.h:L56-L58).
pub fn equal_speeds(a: f64, b: f64) -> bool {
    (a * 10.0).round() as i32 == (b * 10.0).round() as i32
}

/// Parity target coordinator struct `MediaCommon`.
#[derive(Debug, Clone, Default)]
pub struct MediaCommon {
    pub repeat_mode: RepeatMode,
    pub order_mode: OrderMode,
    pub quality: VideoQuality,
    pub speed: f64,
}

impl MediaCommon {
    pub fn new() -> Self {
        Self {
            repeat_mode: RepeatMode::None,
            order_mode: OrderMode::Default,
            quality: VideoQuality::default(),
            speed: 1.0,
        }
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(K_SPEED_MIN, K_SPEED_MAX);
    }

    pub fn is_sped_up(&self) -> bool {
        equal_speeds(self.speed, K_SPED_UP_DEFAULT)
    }
}
