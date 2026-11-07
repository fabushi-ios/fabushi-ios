//! Target Symbol: DataStreaming
//! Telegram Desktop 1:1 Submodule: `data/data_streaming`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamBuffer {
    pub start_offset: u64,
    pub length: usize,
    pub is_eof: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataStreaming {
    pub file_size: u64,
    pub playback_offset: u64,
    pub buffered_ranges: Vec<(u64, u64)>, // (start, end)
    pub is_buffering: bool,
    pub buffer_watermark_bytes: u64,
}

impl Default for DataStreaming {
    fn default() -> Self {
        Self {
            file_size: 0,
            playback_offset: 0,
            buffered_ranges: Vec::new(),
            is_buffering: false,
            buffer_watermark_bytes: 1024 * 1024, // 1MB initial buffer
        }
    }
}

impl DataStreaming {
    pub fn new(file_size: u64) -> Self {
        Self {
            file_size,
            playback_offset: 0,
            buffered_ranges: Vec::new(),
            is_buffering: false,
            buffer_watermark_bytes: 1024 * 1024,
        }
    }

    pub fn append_buffered_range(&mut self, start: u64, length: u64) {
        let end = (start + length).min(self.file_size);
        self.buffered_ranges.push((start, end));
        self.merge_ranges();
    }

    fn merge_ranges(&mut self) {
        if self.buffered_ranges.is_empty() {
            return;
        }
        self.buffered_ranges.sort_by_key(|&(s, _)| s);
        let mut merged = Vec::new();
        let (mut cur_s, mut cur_e) = self.buffered_ranges[0];

        for &(s, e) in &self.buffered_ranges[1..] {
            if s <= cur_e {
                cur_e = cur_e.max(e);
            } else {
                merged.push((cur_s, cur_e));
                cur_s = s;
                cur_e = e;
            }
        }
        merged.push((cur_s, cur_e));
        self.buffered_ranges = merged;
    }

    pub fn can_playback(&self) -> bool {
        for &(start, end) in &self.buffered_ranges {
            if start <= self.playback_offset {
                let available = end.saturating_sub(self.playback_offset);
                if available >= self.buffer_watermark_bytes || end >= self.file_size {
                    return true;
                }
            }
        }
        false
    }

    pub fn advance_playback(&mut self, bytes_read: u64) {
        self.playback_offset = (self.playback_offset + bytes_read).min(self.file_size);
    }
}
