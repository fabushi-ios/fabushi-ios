//! Upstream Source: `Telegram/SourceFiles/media/streaming/media_streaming_file_delegate.h:L1-L42`
//! Parity Module: `media_streaming_file_delegate` | Feature: `TCG-N52` (Streaming Demuxer Delegate)

use super::media_streaming_common::{Mode, StreamingError};
use serde::{Deserialize, Serialize};

pub trait FileDelegate: Send + Sync {
    fn file_open_mode(&self) -> Mode;
    fn file_ready(&mut self, header_size: usize, has_video: bool, has_audio: bool) -> bool;
    fn file_error(&mut self, error: StreamingError);
    fn file_waiting_for_data(&mut self);
    fn file_full_in_cache(&mut self, full_in_cache: bool);
    fn file_process_end_of_file(&mut self);
    fn file_process_packets(&mut self, packets_count: usize) -> bool;
    fn file_read_more(&self) -> bool;
}

/// Ledger Target Symbol: `MediaStreamingFileDelegateStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingFileDelegateStreaming {
    pub open_mode: Mode,
    pub is_ready: bool,
    pub header_size: usize,
    pub has_video: bool,
    pub has_audio: bool,
    pub last_error: Option<StreamingError>,
    pub is_waiting_for_data: bool,
    pub is_full_in_cache: bool,
    pub eof_reached: bool,
    pub packets_processed: usize,
    pub should_read_more: bool,
}

impl Default for MediaStreamingFileDelegateStreaming {
    fn default() -> Self {
        Self {
            open_mode: Mode::Both,
            is_ready: false,
            header_size: 0,
            has_video: false,
            has_audio: false,
            last_error: None,
            is_waiting_for_data: false,
            is_full_in_cache: false,
            eof_reached: false,
            packets_processed: 0,
            should_read_more: true,
        }
    }
}

impl FileDelegate for MediaStreamingFileDelegateStreaming {
    fn file_open_mode(&self) -> Mode {
        self.open_mode
    }

    fn file_ready(&mut self, header_size: usize, has_video: bool, has_audio: bool) -> bool {
        self.is_ready = true;
        self.header_size = header_size;
        self.has_video = has_video;
        self.has_audio = has_audio;
        true
    }

    fn file_error(&mut self, error: StreamingError) {
        self.last_error = Some(error);
    }

    fn file_waiting_for_data(&mut self) {
        self.is_waiting_for_data = true;
    }

    fn file_full_in_cache(&mut self, full_in_cache: bool) {
        self.is_full_in_cache = full_in_cache;
    }

    fn file_process_end_of_file(&mut self) {
        self.eof_reached = true;
    }

    fn file_process_packets(&mut self, packets_count: usize) -> bool {
        self.packets_processed += packets_count;
        self.is_waiting_for_data = false;
        self.should_read_more
    }

    fn file_read_more(&self) -> bool {
        self.should_read_more
    }
}
