//! Upstream Sources:
//! - `Telegram/SourceFiles/media/clip/media_clip_reader.h:L1-L249`
//! - `Telegram/SourceFiles/media/clip/media_clip_reader.cpp:L1-L1066`
//! Parity Module: `media_clip_reader` | Feature: `TCG-N52` (Triple-Buffered Animated Clip Reader FSM)

use serde::{Deserialize, Serialize};

/// `media_clip_reader.cpp:L36-L38`
pub const K_CLIP_THREADS_COUNT: usize = 8;
pub const K_AVERAGE_GIF_SIZE: usize = 320 * 240;
pub const K_WAIT_BEFORE_GIF_PAUSE_MS: i64 = 200;

/// `media_clip_reader.h:L47-L54`
pub const K_WAITING_FOR_DIMENSIONS_STEP: i32 = -3;
pub const K_WAITING_FOR_REQUEST_STEP: i32 = -2;
pub const K_WAITING_FOR_FIRST_FRAME_STEP: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipState {
    Reading,
    Error,
    Finished,
}

impl Default for ClipState {
    fn default() -> Self {
        Self::Reading
    }
}

/// `media_clip_reader.h:L147-L148`
pub fn show_frame_index(step: i32) -> usize {
    if step < 0 {
        0
    } else {
        (((step + 1) / 2) % 3) as usize
    }
}

pub fn write_frame_index(step: i32) -> usize {
    if step < 0 {
        0
    } else {
        (((step + 3) / 2) % 3) as usize
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipFrameSlot {
    pub position_ms: i64,
    pub index: usize,
    pub is_displayed: bool,
    pub width: u32,
    pub height: u32,
}

impl Default for ClipFrameSlot {
    fn default() -> Self {
        Self {
            position_ms: 0,
            index: 0,
            is_displayed: false,
            width: 0,
            height: 0,
        }
    }
}

/// Ledger Target Symbol: `MediaClipReaderClip`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaClipReaderClip {
    pub state: ClipState,
    pub width: u32,
    pub height: u32,
    pub duration_ms: i64,
    pub step: i32,
    pub frames: [ClipFrameSlot; 3],
    pub auto_paused_gif: bool,
    pub thread_index: usize,
    pub moves_count: usize,
}

impl Default for MediaClipReaderClip {
    fn default() -> Self {
        Self {
            state: ClipState::Reading,
            width: 0,
            height: 0,
            duration_ms: 0,
            step: K_WAITING_FOR_DIMENSIONS_STEP,
            frames: [
                ClipFrameSlot::default(),
                ClipFrameSlot::default(),
                ClipFrameSlot::default(),
            ],
            auto_paused_gif: false,
            thread_index: 0,
            moves_count: 0,
        }
    }
}

impl MediaClipReaderClip {
    pub fn new(width: u32, height: u32, duration_ms: i64, thread_index: usize) -> Self {
        Self {
            state: ClipState::Reading,
            width,
            height,
            duration_ms,
            step: if width > 0 && height > 0 {
                K_WAITING_FOR_FIRST_FRAME_STEP
            } else {
                K_WAITING_FOR_DIMENSIONS_STEP
            },
            frames: [
                ClipFrameSlot::default(),
                ClipFrameSlot::default(),
                ClipFrameSlot::default(),
            ],
            auto_paused_gif: false,
            thread_index: thread_index % K_CLIP_THREADS_COUNT,
            moves_count: 0,
        }
    }

    pub fn is_started(&self) -> bool {
        self.step >= K_WAITING_FOR_FIRST_FRAME_STEP
    }

    pub fn start(&mut self) {
        if self.step < 0 {
            self.step = 0;
        }
    }

    pub fn move_to_next_show(&mut self) -> bool {
        if self.step >= 0 {
            self.step = (self.step + 1) % 6;
            self.moves_count += 1;
            let show_idx = show_frame_index(self.step);
            self.frames[show_idx].is_displayed = true;
            true
        } else {
            false
        }
    }

    pub fn move_to_next_write(&mut self, position_ms: i64, width: u32, height: u32) {
        if self.step >= 0 {
            let write_idx = write_frame_index(self.step);
            self.frames[write_idx] = ClipFrameSlot {
                position_ms,
                index: self.moves_count,
                is_displayed: false,
                width,
                height,
            };
        }
    }

    pub fn current_frame(&self) -> &ClipFrameSlot {
        let idx = show_frame_index(self.step);
        &self.frames[idx]
    }
}
