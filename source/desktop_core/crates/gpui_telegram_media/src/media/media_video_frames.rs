//! Telegram Desktop 1:1 Parity Submodule: `MediaVideoFrames`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/media_video_frames.h:L1-L46`
//! - `Telegram/SourceFiles/media/media_video_frames.cpp:L1-L401`
//! Subsystem: `media` | Feature: `TCG-N08`

pub const K_MAX_DECODE_SPAN_MS: i64 = 10_000;
pub const K_FORWARD_DECODE_WINDOW_MS: i64 = 3_000;
pub const K_MAX_DECODE_AREA: i64 = 4096 * 4096;

/// Video file metadata corresponding to `Media::Video::FileInfo` (media_video_frames.h:L12-L19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileInfo {
    pub width: u32,
    pub height: u32,
    pub duration_ms: i64,
}

impl FileInfo {
    pub fn new(width: u32, height: u32, duration_ms: i64) -> Self {
        Self {
            width,
            height,
            duration_ms,
        }
    }

    pub fn valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.duration_ms > 0
    }
}

/// Request for extracting keyframes corresponding to `Media::Video::ExtractRequest` (L25-L29).
#[derive(Debug, Clone, Default)]
pub struct ExtractRequest {
    pub positions_ms: Vec<i64>,
    pub box_width: u32,
    pub box_height: u32,
    pub cover: bool,
}

/// Extracted video frame with geometry and timestamp.
#[derive(Debug, Clone, Default)]
pub struct ExtractedFrame {
    pub position_ms: i64,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Calculates cover crop rectangle for centered aspect framing (media_video_frames.cpp:L180-L188).
pub fn compute_cover_crop(
    orig_w: u32,
    orig_h: u32,
    box_w: u32,
    box_h: u32,
) -> (u32, u32, u32, u32) {
    if box_w == 0 || box_h == 0 || orig_w == 0 || orig_h == 0 {
        return (0, 0, orig_w, orig_h);
    }
    let rw = box_w as f64 / orig_w as f64;
    let rh = box_h as f64 / orig_h as f64;
    let r = rw.max(rh);
    let scaled_w = (orig_w as f64 * r).round() as u32;
    let scaled_h = (orig_h as f64 * r).round() as u32;

    let x = (scaled_w.saturating_sub(box_w)) / 2;
    let y = (scaled_h.saturating_sub(box_h)) / 2;
    (x, y, box_w.min(scaled_w), box_h.min(scaled_h))
}

/// Parity target coordinator struct `MediaVideoFrames`.
#[derive(Debug, Clone, Default)]
pub struct MediaVideoFrames {
    pub info: FileInfo,
    pub extracted: Vec<ExtractedFrame>,
}

impl MediaVideoFrames {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_info(&mut self, width: u32, height: u32, duration_ms: i64) {
        self.info = FileInfo::new(width, height, duration_ms);
    }

    pub fn is_valid(&self) -> bool {
        self.info.valid()
    }

    pub fn add_frame(&mut self, pos_ms: i64, w: u32, h: u32, data: Vec<u8>) {
        self.extracted.push(ExtractedFrame {
            position_ms: pos_ms,
            width: w,
            height: h,
            data,
        });
    }
}
