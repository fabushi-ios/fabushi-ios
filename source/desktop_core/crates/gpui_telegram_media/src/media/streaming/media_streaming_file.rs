//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_file.h:L1-L127`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_file.cpp:L1-L512`
//! Parity Module: `media_streaming_file` | Feature: `TCG-N52` (Streaming Custom AVIO Demuxer File)

use super::media_streaming_common::StreamingError;
use serde::{Deserialize, Serialize};

/// `media_streaming_file.cpp:L18-L19`
pub const K_MAX_SINGLE_READ_AMOUNT: usize = 8 * 1024 * 1024;
pub const K_MAX_QUEUED_PACKETS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOptions {
    pub position: i64,
    pub duration_override: i64,
    pub seekable: bool,
    pub hw_allow: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            position: 0,
            duration_override: 0,
            seekable: true,
            hw_allow: false,
        }
    }
}

/// Ledger Target Symbol: `MediaStreamingFileStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingFileStreaming {
    pub file_size: i64,
    pub current_offset: i64,
    pub is_active: bool,
    pub is_interrupted: bool,
    pub is_failed: bool,
    pub is_read_till_end: bool,
    pub is_full_in_cache: bool,
    pub queued_packets_count: usize,
    pub start_options: StartOptions,
}

impl Default for MediaStreamingFileStreaming {
    fn default() -> Self {
        Self {
            file_size: 0,
            current_offset: 0,
            is_active: false,
            is_interrupted: false,
            is_failed: false,
            is_read_till_end: false,
            is_full_in_cache: false,
            queued_packets_count: 0,
            start_options: StartOptions::default(),
        }
    }
}

impl MediaStreamingFileStreaming {
    pub fn new(size: i64) -> Self {
        Self {
            file_size: size,
            current_offset: 0,
            is_active: false,
            is_interrupted: false,
            is_failed: false,
            is_read_till_end: false,
            is_full_in_cache: false,
            queued_packets_count: 0,
            start_options: StartOptions::default(),
        }
    }

    pub fn start(&mut self, options: StartOptions) {
        self.is_active = true;
        self.is_interrupted = false;
        self.is_failed = false;
        self.is_read_till_end = false;
        self.start_options = options;
        if options.seekable && options.position > 0 {
            self.current_offset = 0; // Seeks will be executed during demuxing
        }
    }

    /// `media_streaming_file.cpp:L56-L101`
    pub fn read_bytes(&mut self, amount: usize) -> Result<usize, StreamingError> {
        if self.is_interrupted || self.is_failed {
            return Err(StreamingError::OpenFailed);
        }
        if amount > K_MAX_SINGLE_READ_AMOUNT {
            return Err(StreamingError::InvalidData);
        }
        if self.current_offset >= self.file_size {
            self.is_read_till_end = true;
            return Ok(0);
        }

        let remaining = (self.file_size - self.current_offset) as usize;
        let read = amount.min(remaining);
        self.current_offset += read as i64;

        if self.current_offset >= self.file_size {
            self.is_read_till_end = true;
        }

        Ok(read)
    }

    /// `media_streaming_file.cpp:L103-L117`
    pub fn seek_offset(&mut self, offset: i64) -> Result<i64, StreamingError> {
        if self.is_failed || offset < 0 || offset > self.file_size {
            return Err(StreamingError::InvalidData);
        }
        self.current_offset = offset;
        self.is_read_till_end = offset >= self.file_size;
        Ok(self.current_offset)
    }

    /// `media_streaming_file.cpp:L358-L382`
    pub fn enqueue_packet(&mut self) -> bool {
        self.queued_packets_count += 1;
        self.queued_packets_count >= K_MAX_QUEUED_PACKETS
    }

    pub fn clear_queued_packets(&mut self) {
        self.queued_packets_count = 0;
    }

    pub fn interrupt(&mut self) {
        self.is_interrupted = true;
        self.is_active = false;
    }
}
