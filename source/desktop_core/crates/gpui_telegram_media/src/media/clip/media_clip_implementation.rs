//! Upstream Sources:
//! - `Telegram/SourceFiles/media/clip/media_clip_implementation.h:L1-L76`
//! - `Telegram/SourceFiles/media/clip/media_clip_implementation.cpp:L1-L32`
//! Parity Module: `media_clip_implementation` | Feature: `TCG-N52` (Clip Reader Backend Abstraction)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipMode {
    Silent,
    Inspecting,
}

impl Default for ClipMode {
    fn default() -> Self {
        Self::Silent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipReadResult {
    Success,
    Error,
    EndOfFile,
}

pub trait ClipReaderImplementation: Send + Sync {
    fn read_frames_till(&mut self, frame_ms: i64, system_ms: i64) -> ClipReadResult;
    fn frame_real_time(&self) -> i64;
    fn frame_presentation_time(&self) -> i64;
    fn duration_ms(&self) -> i64;
    fn start(&mut self, mode: ClipMode, position_ms: &mut i64) -> bool;
}

/// Ledger Target Symbol: `MediaClipImplementationClip`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaClipImplementationClip {
    pub mode: ClipMode,
    pub data_size: usize,
    pub frame_real_time_ms: i64,
    pub frame_presentation_time_ms: i64,
    pub duration: i64,
    pub is_started: bool,
    pub current_frame_index: usize,
}

impl Default for MediaClipImplementationClip {
    fn default() -> Self {
        Self {
            mode: ClipMode::Silent,
            data_size: 0,
            frame_real_time_ms: 0,
            frame_presentation_time_ms: 0,
            duration: 0,
            is_started: false,
            current_frame_index: 0,
        }
    }
}

impl MediaClipImplementationClip {
    pub fn new(data_size: usize, duration: i64) -> Self {
        Self {
            mode: ClipMode::Silent,
            data_size,
            frame_real_time_ms: 0,
            frame_presentation_time_ms: 0,
            duration,
            is_started: false,
            current_frame_index: 0,
        }
    }
}

impl ClipReaderImplementation for MediaClipImplementationClip {
    fn read_frames_till(&mut self, frame_ms: i64, _system_ms: i64) -> ClipReadResult {
        if !self.is_started || self.duration == 0 {
            return ClipReadResult::Error;
        }

        if frame_ms >= self.duration {
            // Loop back to start for animated GIFs/clips
            self.frame_presentation_time_ms = 0;
            self.frame_real_time_ms = 0;
            self.current_frame_index = 0;
            return ClipReadResult::EndOfFile;
        }

        self.frame_presentation_time_ms = frame_ms;
        self.frame_real_time_ms = frame_ms;
        self.current_frame_index += 1;
        ClipReadResult::Success
    }

    fn frame_real_time(&self) -> i64 {
        self.frame_real_time_ms
    }

    fn frame_presentation_time(&self) -> i64 {
        self.frame_presentation_time_ms
    }

    fn duration_ms(&self) -> i64 {
        self.duration
    }

    fn start(&mut self, mode: ClipMode, position_ms: &mut i64) -> bool {
        self.mode = mode;
        self.is_started = true;
        self.frame_presentation_time_ms = *position_ms;
        true
    }
}
