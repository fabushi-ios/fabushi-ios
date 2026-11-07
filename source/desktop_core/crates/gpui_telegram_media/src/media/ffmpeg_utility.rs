//! Telegram Desktop 1:1 Parity Submodule: `FfmpegUtility`
//! Upstream Sources:
//! - `Telegram/SourceFiles/ffmpeg/ffmpeg_utility.h:L1-L262`
//! - `Telegram/SourceFiles/ffmpeg/ffmpeg_utility.cpp:L1-L884`
//! Subsystem: `media` | Feature: `TCG-N08`

pub const K_ALIGN_IMAGE_BY: i32 = 64;
pub const K_MAX_SCALE_BY_ASPECT_RATIO: i32 = 16;
pub const K_AVIO_BLOCK_SIZE: i32 = 4096;
pub const K_MAX_FRAME_STORAGE_BYTES: i64 = 64 * 1024 * 1024;
pub const K_PIXEL_BYTES_SIZE: i32 = 4;
pub const K_TIME_UNKNOWN: i64 = i64::MIN;

/// FFmpeg error wrapper corresponding to `FFmpeg::AvErrorWrap` (ffmpeg_utility.h:L44-L71).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AvErrorWrap {
    pub code: i32,
}

impl AvErrorWrap {
    pub fn new(code: i32) -> Self {
        Self { code }
    }

    pub fn failed(&self) -> bool {
        self.code < 0
    }

    pub fn text(&self) -> String {
        if self.code == 0 {
            "Success".to_string()
        } else if self.code == super::ffmpeg_bytes_io_wrap::AVERROR_EOF {
            "End of file".to_string()
        } else {
            format!("FFmpeg error code: {}", self.code)
        }
    }
}

/// Normalizes rotation degrees to {0, 90, 180, 270} (ffmpeg_utility.cpp:L76-L82).
pub fn normalize_rotation(rotation: i32) -> i32 {
    let mut result = rotation % 360;
    if result < 0 {
        result += 360;
    }
    if result == 90 || result == 180 || result == 270 {
        result
    } else {
        0
    }
}

/// Checks if rotation swaps width and height.
pub fn rotation_swap_width_height(rotation: i32) -> bool {
    let r = normalize_rotation(rotation);
    r == 90 || r == 270
}

/// Transposes (width, height) based on normalized rotation.
pub fn transpose_size_by_rotation(width: u32, height: u32, rotation: i32) -> (u32, u32) {
    if rotation_swap_width_height(rotation) {
        (height, width)
    } else {
        (width, height)
    }
}

/// Converts FFmpeg presentation timestamp to milliseconds (ffmpeg_utility.cpp:L697-L701).
pub fn pts_to_time(pts: i64, num: i32, den: i32) -> i64 {
    if pts == i64::MIN || den == 0 {
        K_TIME_UNKNOWN
    } else {
        (pts * 1000 * num as i64) / den as i64
    }
}

/// Converts FFmpeg presentation timestamp to milliseconds with ceil (L703-L707).
pub fn pts_to_time_ceil(pts: i64, num: i32, den: i32) -> i64 {
    if pts == i64::MIN || den == 0 {
        K_TIME_UNKNOWN
    } else {
        let den_i64 = den as i64;
        (pts * 1000 * num as i64 + den_i64 - 1) / den_i64
    }
}

/// Converts millisecond time to FFmpeg presentation timestamp (L709-L713).
pub fn time_to_pts(time_ms: i64, num: i32, den: i32) -> i64 {
    if time_ms == K_TIME_UNKNOWN || num == 0 {
        i64::MIN
    } else {
        (time_ms * den as i64) / (1000 * num as i64)
    }
}

/// Premultiplies an ARGB pixel in place (ffmpeg_utility.cpp:L861-L881).
pub fn premultiply_pixel(r: u8, g: u8, b: u8, a: u8) -> (u8, u8, u8, u8) {
    if a == 255 {
        (r, g, b, a)
    } else if a == 0 {
        (0, 0, 0, 0)
    } else {
        let alpha = a as u32;
        (
            ((r as u32 * alpha + 127) / 255) as u8,
            ((g as u32 * alpha + 127) / 255) as u8,
            ((b as u32 * alpha + 127) / 255) as u8,
            a,
        )
    }
}

/// Un-premultiplies an ARGB pixel (ffmpeg_utility.cpp:L827-L859).
pub fn unpremultiply_pixel(r: u8, g: u8, b: u8, a: u8) -> (u8, u8, u8, u8) {
    if a == 255 || a == 0 {
        (r, g, b, a)
    } else {
        let alpha = a as u32;
        (
            ((r as u32 * 255 + (alpha / 2)) / alpha).min(255) as u8,
            ((g as u32 * 255 + (alpha / 2)) / alpha).min(255) as u8,
            ((b as u32 * 255 + (alpha / 2)) / alpha).min(255) as u8,
            a,
        )
    }
}

/// Parity target coordinator struct `FfmpegUtility`.
#[derive(Debug, Clone, Default)]
pub struct FfmpegUtility {
    pub multi_thread: bool,
    pub thread_count: i32,
    pub last_error: AvErrorWrap,
}

impl FfmpegUtility {
    pub fn new() -> Self {
        Self {
            multi_thread: true,
            thread_count: 0,
            last_error: AvErrorWrap::default(),
        }
    }

    pub fn set_last_error(&mut self, code: i32) {
        self.last_error = AvErrorWrap::new(code);
    }
}
