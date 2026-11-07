//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_video_track.h:L1-L189`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_video_track.cpp:L1-L1441`
//! Parity Module: `media_streaming_video_track` | Feature: `TCG-N52` (Video Track Ring Buffer & Display Clock)

use super::media_streaming_common::{PlaybackOptions, TrackState, K_TIME_UNKNOWN};
use serde::{Deserialize, Serialize};

/// `media_streaming_video_track.h:L151`
pub const K_FRAMES_COUNT: usize = 4;
/// `media_streaming_video_track.cpp:L27-L28`
pub const K_DISPLAY_SKIPPED: i64 = -1;
pub const K_FINISHED_POSITION: i64 = i64::MAX;

/// `media_streaming_video_track.cpp:L33-L39`
pub fn world_time_delay(track_delay: i64, speed: f64) -> i64 {
    if track_delay <= 0 || speed <= 0.0 || (speed - 1.0).abs() < f64::EPSILON {
        track_delay
    } else {
        let adjusted = (track_delay as f64 / speed).round();
        if adjusted < 1.0 {
            1
        } else {
            adjusted as i64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BufferedVideoFrame {
    pub position: i64,
    pub display_time: i64,
    pub index: usize,
    pub is_displayed: bool,
    pub width: u32,
    pub height: u32,
}

impl Default for BufferedVideoFrame {
    fn default() -> Self {
        Self {
            position: K_TIME_UNKNOWN,
            display_time: K_TIME_UNKNOWN,
            index: 0,
            is_displayed: false,
            width: 0,
            height: 0,
        }
    }
}

/// Ledger Target Symbol: `MediaStreamingVideoTrackStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingVideoTrackStreaming {
    pub stream_index: i32,
    pub stream_time_base: (i32, i32),
    pub stream_duration: i64,
    pub rotation: i32,
    pub fps: f64,
    pub speed: f64,
    pub play_position: i64,
    pub received_till: i64,
    pub is_paused: bool,
    pub is_waiting_for_data: bool,
    pub ring_buffer: Vec<BufferedVideoFrame>,
    pub write_cursor: usize,
    pub read_cursor: usize,
    pub frames_displayed_count: usize,
}

impl Default for MediaStreamingVideoTrackStreaming {
    fn default() -> Self {
        Self {
            stream_index: 0,
            stream_time_base: (1, 1000),
            stream_duration: 0,
            rotation: 0,
            fps: 30.0,
            speed: 1.0,
            play_position: 0,
            received_till: 0,
            is_paused: false,
            is_waiting_for_data: false,
            ring_buffer: vec![BufferedVideoFrame::default(); K_FRAMES_COUNT],
            write_cursor: 0,
            read_cursor: 0,
            frames_displayed_count: 0,
        }
    }
}

impl MediaStreamingVideoTrackStreaming {
    pub fn new(options: &PlaybackOptions, duration: i64, fps: f64) -> Self {
        Self {
            stream_index: 0,
            stream_time_base: (1, 1000),
            stream_duration: duration,
            rotation: 0,
            fps: if fps > 0.0 { fps } else { 30.0 },
            speed: options.speed.clamp(0.5, 2.0),
            play_position: options.position,
            received_till: options.position,
            is_paused: false,
            is_waiting_for_data: false,
            ring_buffer: vec![BufferedVideoFrame::default(); K_FRAMES_COUNT],
            write_cursor: 0,
            read_cursor: 0,
            frames_displayed_count: 0,
        }
    }

    pub fn push_decoded_frame(&mut self, position: i64, width: u32, height: u32) {
        let slot = &mut self.ring_buffer[self.write_cursor % K_FRAMES_COUNT];
        slot.position = position;
        slot.display_time = position;
        slot.width = width;
        slot.height = height;
        slot.is_displayed = false;
        slot.index = self.write_cursor;

        self.write_cursor += 1;
        self.received_till = self.received_till.max(position);
        self.is_waiting_for_data = false;
    }

    pub fn mark_frame_displayed(&mut self, now_position: i64) -> Option<BufferedVideoFrame> {
        let slot = &mut self.ring_buffer[self.read_cursor % K_FRAMES_COUNT];
        if slot.position != K_TIME_UNKNOWN && !slot.is_displayed {
            slot.is_displayed = true;
            self.play_position = now_position.max(slot.position);
            self.frames_displayed_count += 1;
            let frame = slot.clone();
            self.read_cursor += 1;
            Some(frame)
        } else {
            None
        }
    }

    pub fn pause(&mut self) {
        self.is_paused = true;
    }

    pub fn resume(&mut self) {
        self.is_paused = false;
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.0);
    }

    pub fn track_state(&self) -> TrackState {
        TrackState {
            position: self.play_position,
            received_till: self.received_till,
            duration: self.stream_duration,
        }
    }
}
