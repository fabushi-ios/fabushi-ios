//! Telegram Desktop 1:1 Parity Submodule: `MediaVideoEncode`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/media_video_encode.h:L1-L190`
//! - `Telegram/SourceFiles/media/media_video_encode.cpp:L1-L2767`
//! Subsystem: `media` | Feature: `TCG-N08`

pub const K_MIN_BITRATE: i32 = 600_000;
pub const K_MAX_BITRATE: i32 = 6_800_000;
pub const K_WEBM_STICKER_CRF: i32 = 28;
pub const K_WEBM_STICKER_CRF_LADDER: [Option<i32>; 6] = [
    None,
    Some(34),
    Some(40),
    Some(47),
    Some(55),
    Some(63),
];
pub const K_MAX_SOURCE_SIZE: i64 = 1000 * 1024 * 1024;
pub const K_MAX_TRANSCODE_AREA: i64 = 4096 * 4096;
pub const K_AUDIO_FREQUENCY: i32 = 48_000;
pub const K_AUDIO_BITRATE_PER_CHANNEL: i32 = 64_000;
pub const K_RATE_CONTROL_FPS: f64 = 60.0;
pub const K_SILENT_AUDIO_BITRATE: i32 = 7_000;

/// Sanitizes frame rate to valid encoding interval (media_video_encode.cpp:L79-L81).
pub fn sanitized_fps(fps: f64) -> f64 {
    if fps > 1.0 && fps < 121.0 {
        fps
    } else {
        30.0
    }
}

/// Computes target encoding bitrate (media_video_encode.cpp:L83-L87).
pub fn target_bitrate(width: u32, height: u32, fps: f64) -> i32 {
    let pixels = (width as u64) * (height as u64);
    let bits = (pixels as f64) * sanitized_fps(fps) * 0.07;
    (bits.round() as i32).clamp(K_MIN_BITRATE, K_MAX_BITRATE)
}

/// Downscales dimensions so that the shorter side matches `target_shorter_side` (L170).
pub fn downscaled_size(orig_w: u32, orig_h: u32, target_shorter_side: u32) -> (u32, u32) {
    if orig_w == 0 || orig_h == 0 || target_shorter_side == 0 {
        return (orig_w, orig_h);
    }
    let shorter = orig_w.min(orig_h);
    if shorter <= target_shorter_side {
        return (orig_w, orig_h);
    }
    let ratio = target_shorter_side as f64 / shorter as f64;
    let new_w = ((orig_w as f64 * ratio).round() as u32) & !1;
    let new_h = ((orig_h as f64 * ratio).round() as u32) & !1;
    (new_w.max(2), new_h.max(2))
}

/// Computes transcoded duration given trim bounds (L2665-L2673).
pub fn transcoded_duration(from_ms: i64, till_ms: i64, duration_ms: i64) -> i64 {
    let from = from_ms.clamp(0, duration_ms);
    let till = if till_ms > from {
        till_ms.min(duration_ms)
    } else {
        duration_ms
    };
    (till - from).max(0)
}

/// Video source descriptor corresponding to `Media::Encode::VideoSource` (media_video_encode.h:L64-L97).
#[derive(Debug, Clone, Default)]
pub struct VideoSourceDescriptor {
    pub path: String,
    pub target_shorter_side: u32,
    pub from_ms: i64,
    pub till_ms: i64,
    pub remove_audio: bool,
    pub fps_limit: f64,
    pub webm_crf: Option<i32>,
}

/// Parity target coordinator struct `MediaVideoEncode`.
#[derive(Debug, Clone, Default)]
pub struct MediaVideoEncode {
    pub source: VideoSourceDescriptor,
    pub computed_bitrate: i32,
    pub output_duration_ms: i64,
}

impl MediaVideoEncode {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn plan_transcode(&mut self, orig_w: u32, orig_h: u32, fps: f64, total_dur_ms: i64) {
        let (dw, dh) = if self.source.target_shorter_side > 0 {
            downscaled_size(orig_w, orig_h, self.source.target_shorter_side)
        } else {
            (orig_w, orig_h)
        };
        self.computed_bitrate = target_bitrate(dw, dh, fps);
        self.output_duration_ms =
            transcoded_duration(self.source.from_ms, self.source.till_ms, total_dur_ms);
    }
}
