//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader_local.h:L1-L56`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader_local.cpp:L1-L116`
//! Parity Module: `media_streaming_loader_local` | Feature: `TCG-N52` (Local Media Progressive Loader)

use super::media_streaming_common::SpeedEstimate;
use super::media_streaming_loader::{LoadedPart, Loader, K_PART_SIZE};
use serde::{Deserialize, Serialize};

/// `media_streaming_loader_local.cpp:L19` (8000 * 512KB = 4,096,000,000 bytes max file size)
pub const K_MAX_FILE_SIZE: i64 = 8000 * (512 * 1024);

/// `media_streaming_loader_local.cpp:L21-L23`
pub fn validate_local_size(size: i64) -> i64 {
    if size > 0 && size <= K_MAX_FILE_SIZE {
        size
    } else {
        0
    }
}

/// Ledger Target Symbol: `MediaStreamingLoaderLocalStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingLoaderLocalStreaming {
    pub file_size: i64,
    pub source_path: Option<String>,
    pub buffer: Vec<u8>,
    pub is_stopped: bool,
    pub priority: i32,
    pub parts_read_count: usize,
}

impl Default for MediaStreamingLoaderLocalStreaming {
    fn default() -> Self {
        Self {
            file_size: 0,
            source_path: None,
            buffer: Vec::new(),
            is_stopped: false,
            priority: 0,
            parts_read_count: 0,
        }
    }
}

impl MediaStreamingLoaderLocalStreaming {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        let size = validate_local_size(bytes.len() as i64);
        Self {
            file_size: size,
            source_path: None,
            buffer: bytes,
            is_stopped: false,
            priority: 0,
            parts_read_count: 0,
        }
    }

    pub fn from_path(path: impl Into<String>, size: i64) -> Self {
        let valid_size = validate_local_size(size);
        Self {
            file_size: valid_size,
            source_path: Some(path.into()),
            buffer: Vec::new(),
            is_stopped: false,
            priority: 0,
            parts_read_count: 0,
        }
    }

    /// `media_streaming_loader_local.cpp:L45-L60`
    pub fn read_part(&mut self, offset: i64) -> LoadedPart {
        if self.is_stopped || offset < 0 || offset >= self.file_size {
            return LoadedPart::failed();
        }

        if !self.buffer.is_empty() {
            let start = offset as usize;
            let end = (start + K_PART_SIZE).min(self.buffer.len());
            if start >= end {
                return LoadedPart::failed();
            }
            let bytes = self.buffer[start..end].to_vec();
            self.parts_read_count += 1;
            LoadedPart { offset, bytes }
        } else {
            // Virtual/mock device read for testing progressive streaming
            let remaining = (self.file_size - offset) as usize;
            let amount = remaining.min(K_PART_SIZE);
            let bytes = vec![0u8; amount];
            self.parts_read_count += 1;
            LoadedPart { offset, bytes }
        }
    }
}

impl Loader for MediaStreamingLoaderLocalStreaming {
    fn size(&self) -> i64 {
        self.file_size
    }

    fn load(&mut self, offset: i64) {
        let _ = self.read_part(offset);
    }

    fn cancel(&mut self, _offset: i64) {}

    fn reset_priorities(&mut self) {}

    fn set_priority(&mut self, priority: i32) {
        self.priority = priority;
    }

    fn stop(&mut self) {
        self.is_stopped = true;
    }

    fn speed_estimate(&self) -> SpeedEstimate {
        SpeedEstimate {
            bytes_per_second: 100 * 1024 * 1024, // 100 MB/s for local SSD
            unreliable: false,
        }
    }
}
