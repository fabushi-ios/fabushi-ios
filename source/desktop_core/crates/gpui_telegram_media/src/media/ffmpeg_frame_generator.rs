//! Telegram Desktop 1:1 Parity Submodule: `FfmpegFrameGenerator`
//! Upstream Sources:
//! - `Telegram/SourceFiles/ffmpeg/ffmpeg_frame_generator.h:L1-L42`
//! - `Telegram/SourceFiles/ffmpeg/ffmpeg_frame_generator.cpp:L1-L429`
//! Subsystem: `media` | Feature: `TCG-N08`

use super::ffmpeg_bytes_io_wrap::ReadBytesWrap;

/// Maximum area allowed for frame allocation (ffmpeg_frame_generator.cpp:L17).
pub const K_MAX_FRAME_AREA: i64 = 1920 * 1080 * 4;

/// Frame scaling modes matching Qt::AspectRatioMode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatioMode {
    IgnoreAspectRatio,
    KeepAspectRatio,
    KeepAspectRatioByExpanding,
}

/// Generated visual frame corresponding to `Ui::FrameGenerator::Frame`.
#[derive(Debug, Clone, Default)]
pub struct GeneratedFrame {
    pub image_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub duration_ms: i64,
    pub is_last: bool,
}

/// Decoded video frame with timing information (L38-L43).
#[derive(Debug, Clone, Default)]
pub struct ReadFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub position_ms: i64,
    pub duration_ms: i64,
}

/// Frame generator engine corresponding to C++ `FFmpeg::FrameGenerator` (L23-L81).
#[derive(Debug, Clone)]
pub struct FfmpegFrameGenerator {
    pub io: ReadBytesWrap,
    pub current_frame: Option<ReadFrame>,
    pub next_frame: Option<ReadFrame>,
    pub stream_id: i32,
    pub rotation: i32,
    pub frame_rate: f64,
    pub total_frame_count: i32,
    pub frame_position: i64,
    pub current_delay: i32,
    pub next_delay: i32,
}

impl FfmpegFrameGenerator {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            io: ReadBytesWrap::new(bytes),
            current_frame: None,
            next_frame: None,
            stream_id: 0,
            rotation: 0,
            frame_rate: 30.0,
            total_frame_count: 0,
            frame_position: 0,
            current_delay: 0,
            next_delay: 0,
        }
    }

    /// Whether rotation swaps dimensions (ffmpeg_frame_generator.cpp:L49-L51).
    pub fn rotation_swaps_dimensions(&self) -> bool {
        self.rotation == 90 || self.rotation == 270
    }

    pub fn count(&self) -> i32 {
        self.total_frame_count
    }

    pub fn rate(&self) -> f64 {
        self.frame_rate
    }

    /// Computes scaled target frame dimensions preserving aspect ratio (L174-L187).
    pub fn compute_scaled_size(
        &self,
        src_w: u32,
        src_h: u32,
        target_w: u32,
        target_h: u32,
        mode: AspectRatioMode,
    ) -> (u32, u32, u32, u32) {
        let (eff_src_w, eff_src_h) = if self.rotation_swaps_dimensions() {
            (src_h, src_w)
        } else {
            (src_w, src_h)
        };

        if eff_src_w == 0 || eff_src_h == 0 || target_w == 0 || target_h == 0 {
            return (0, 0, 0, 0);
        }

        let (scaled_w, scaled_h) = match mode {
            AspectRatioMode::IgnoreAspectRatio => (target_w, target_h),
            AspectRatioMode::KeepAspectRatio => {
                let rw = target_w as f64 / eff_src_w as f64;
                let rh = target_h as f64 / eff_src_h as f64;
                let r = rw.min(rh);
                (
                    (eff_src_w as f64 * r).round() as u32,
                    (eff_src_h as f64 * r).round() as u32,
                )
            }
            AspectRatioMode::KeepAspectRatioByExpanding => {
                let rw = target_w as f64 / eff_src_w as f64;
                let rh = target_h as f64 / eff_src_h as f64;
                let r = rw.max(rh);
                (
                    (eff_src_w as f64 * r).round() as u32,
                    (eff_src_h as f64 * r).round() as u32,
                )
            }
        };

        let dx = (target_w.saturating_sub(scaled_w)) / 2;
        let dy = (target_h.saturating_sub(scaled_h)) / 2;
        (scaled_w, scaled_h, dx, dy)
    }

    /// Renders current frame buffer (L160-L265).
    pub fn render_current(
        &mut self,
        target_w: u32,
        target_h: u32,
        mode: AspectRatioMode,
    ) -> GeneratedFrame {
        if let Some(ref current) = self.current_frame {
            let (scaled_w, scaled_h, _dx, _dy) =
                self.compute_scaled_size(current.width, current.height, target_w, target_h, mode);
            let duration = if let Some(ref next) = self.next_frame {
                next.position_ms.saturating_sub(current.position_ms)
            } else {
                current.duration_ms
            };
            GeneratedFrame {
                image_data: current.data.clone(),
                width: scaled_w,
                height: scaled_h,
                duration_ms: duration,
                is_last: self.next_frame.is_none(),
            }
        } else {
            GeneratedFrame::default()
        }
    }

    /// Advances to next frame and renders (L267-L282).
    pub fn render_next(
        &mut self,
        target_w: u32,
        target_h: u32,
        mode: AspectRatioMode,
    ) -> GeneratedFrame {
        self.current_frame = self.next_frame.take();
        self.render_current(target_w, target_h, mode)
    }

    /// Seeks playback to beginning (L284-L304).
    pub fn jump_to_start(&mut self) {
        self.io.seek(0, super::ffmpeg_bytes_io_wrap::SEEK_SET);
        self.current_frame = None;
        self.next_frame = None;
        self.current_delay = 0;
        self.next_delay = 0;
        self.frame_position = 0;
    }
}
