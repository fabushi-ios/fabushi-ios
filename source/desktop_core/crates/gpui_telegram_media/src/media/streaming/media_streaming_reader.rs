//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_reader.h:L1-L287`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_reader.cpp:L1-L1443`
//! Parity Module: `media_streaming_reader` | Feature: `TCG-N52` (Streaming Segmented Chunk Cache & Demux Reader)

use super::media_streaming_loader::K_PART_SIZE;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// `media_streaming_reader.cpp:L21-L30`
pub const K_PARTS_IN_SLICE: usize = 64;
pub const K_IN_SLICE: usize = K_PARTS_IN_SLICE * K_PART_SIZE; // 8 MB
pub const K_MAX_PARTS_IN_HEADER: usize = 64;
pub const K_MAX_ONLY_IN_HEADER: usize = 80 * K_PART_SIZE; // 10 MB
pub const K_PARTS_OUTSIDE_FIRST_SLICE_GOOD: usize = 8;
pub const K_SLICES_IN_MEMORY: usize = 2;
pub const K_PRELOAD_PARTS_AHEAD: usize = 8; // 1 MB
pub const K_DOWNLOADER_REQUESTS_LIMIT: usize = 4;

/// `media_streaming_reader.cpp:L60-L65`
pub fn slices_count(total_size: usize) -> usize {
    if total_size == 0 {
        0
    } else {
        (total_size + K_IN_SLICE - 1) / K_IN_SLICE
    }
}

pub fn slice_index_for_offset(offset: usize) -> usize {
    offset / K_IN_SLICE
}

pub fn part_index_for_offset(offset: usize) -> usize {
    offset / K_PART_SIZE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderFillState {
    Success,
    WaitingCache,
    WaitingRemote,
    Failed,
}

/// Ledger Target Symbol: `MediaStreamingReaderStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingReaderStreaming {
    pub total_size: usize,
    pub header_size: usize,
    pub is_header_done: bool,
    pub is_streaming_active: bool,
    pub active_slices: VecDeque<usize>,
    pub cached_parts: HashMap<usize, Vec<u8>>,
    pub is_full_in_cache: bool,
    pub bytes_read_total: usize,
    pub parts_loaded_count: usize,
}

impl Default for MediaStreamingReaderStreaming {
    fn default() -> Self {
        Self {
            total_size: 0,
            header_size: 0,
            is_header_done: false,
            is_streaming_active: false,
            active_slices: VecDeque::new(),
            cached_parts: HashMap::new(),
            is_full_in_cache: false,
            bytes_read_total: 0,
            parts_loaded_count: 0,
        }
    }
}

impl MediaStreamingReaderStreaming {
    pub fn new(total_size: usize) -> Self {
        Self {
            total_size,
            header_size: 0,
            is_header_done: false,
            is_streaming_active: false,
            active_slices: VecDeque::new(),
            cached_parts: HashMap::new(),
            is_full_in_cache: false,
            bytes_read_total: 0,
            parts_loaded_count: 0,
        }
    }

    pub fn header_done(&mut self, header_size: usize) {
        self.header_size = header_size;
        self.is_header_done = true;
    }

    pub fn insert_part(&mut self, offset: usize, bytes: Vec<u8>) {
        let part_idx = part_index_for_offset(offset);
        let slice_idx = slice_index_for_offset(offset);

        if !self.active_slices.contains(&slice_idx) {
            if self.active_slices.len() >= K_SLICES_IN_MEMORY {
                // Evict oldest slice parts
                if let Some(evicted_slice) = self.active_slices.pop_front() {
                    let from_part = evicted_slice * K_PARTS_IN_SLICE;
                    let to_part = from_part + K_PARTS_IN_SLICE;
                    self.cached_parts.retain(|&k, _| k < from_part || k >= to_part);
                }
            }
            self.active_slices.push_back(slice_idx);
        }

        self.cached_parts.insert(part_idx, bytes);
        self.parts_loaded_count += 1;

        let total_parts = (self.total_size + K_PART_SIZE - 1) / K_PART_SIZE;
        if self.cached_parts.len() >= total_parts {
            self.is_full_in_cache = true;
        }
    }

    pub fn fill(&mut self, offset: usize, buffer: &mut [u8]) -> ReaderFillState {
        if offset >= self.total_size {
            return ReaderFillState::Failed;
        }

        let part_idx = part_index_for_offset(offset);
        if let Some(part) = self.cached_parts.get(&part_idx) {
            let part_offset = offset % K_PART_SIZE;
            if part_offset < part.len() {
                let available = part.len() - part_offset;
                let to_copy = buffer.len().min(available);
                buffer[..to_copy].copy_from_slice(&part[part_offset..part_offset + to_copy]);
                self.bytes_read_total += to_copy;
                ReaderFillState::Success
            } else {
                ReaderFillState::WaitingRemote
            }
        } else {
            ReaderFillState::WaitingRemote
        }
    }

    pub fn preload_offsets_ahead(&self, current_offset: usize) -> Vec<usize> {
        let current_part = part_index_for_offset(current_offset);
        let mut needed = Vec::new();

        for i in 1..=K_PRELOAD_PARTS_AHEAD {
            let p = current_part + i;
            let byte_offset = p * K_PART_SIZE;
            if byte_offset < self.total_size && !self.cached_parts.contains_key(&p) {
                needed.push(byte_offset);
            }
        }

        needed
    }
}
