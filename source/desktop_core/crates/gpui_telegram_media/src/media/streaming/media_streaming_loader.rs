//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader.h:L1-L81`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader.cpp:L1-L95`
//! Parity Module: `media_streaming_loader` | Feature: `TCG-N52` (Streaming Part Loader & Priority Queue)

use super::media_streaming_common::SpeedEstimate;
use serde::{Deserialize, Serialize};

pub const K_PART_SIZE: usize = 128 * 1024;
pub const K_FAILED_OFFSET: i64 = -1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedPart {
    pub offset: i64,
    pub bytes: Vec<u8>,
}

impl LoadedPart {
    pub fn failed() -> Self {
        Self {
            offset: K_FAILED_OFFSET,
            bytes: Vec::new(),
        }
    }

    /// `media_streaming_loader.cpp:L13-L17`
    pub fn is_valid(&self, file_size: i64) -> bool {
        if self.offset == K_FAILED_OFFSET {
            return false;
        }
        let len = self.bytes.len();
        len == K_PART_SIZE || (self.offset + len as i64 == file_size)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub value: i64,
    pub priority: i32,
}

/// `media_streaming_loader.cpp:L19-L29`
/// Higher priority first; tie-breaker: lower offset value first.
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match other.priority.cmp(&self.priority) {
            std::cmp::Ordering::Equal => self.value.cmp(&other.value),
            ord => ord,
        }
    }
}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorityQueue {
    entries: Vec<QueueEntry>,
    current_priority: i32,
}

impl PriorityQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// `media_streaming_loader.cpp:L31-L42`
    pub fn add(&mut self, value: i64) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.value == value) {
            if self.entries[pos].priority != self.current_priority {
                self.entries.remove(pos);
                self.entries.push(QueueEntry {
                    value,
                    priority: self.current_priority,
                });
                self.entries.sort();
                return true;
            }
            false
        } else {
            self.entries.push(QueueEntry {
                value,
                priority: self.current_priority,
            });
            self.entries.sort();
            true
        }
    }

    /// `media_streaming_loader.cpp:L44-L51`
    pub fn remove(&mut self, value: i64) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.value == value) {
            self.entries.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `media_streaming_loader.cpp:L57-L61`
    pub fn front(&self) -> Option<i64> {
        self.entries.first().map(|e| e.value)
    }

    /// `media_streaming_loader.cpp:L63-L70`
    pub fn take(&mut self) -> Option<i64> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.entries.remove(0).value)
        }
    }

    /// `media_streaming_loader.cpp:L72-L83`
    pub fn take_in_range(&mut self, from: i64, till: i64) -> Vec<i64> {
        let mut taken = Vec::new();
        self.entries.retain(|entry| {
            if entry.value >= from && entry.value < till {
                taken.push(entry.value);
                false
            } else {
                true
            }
        });
        taken.sort();
        taken
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// `media_streaming_loader.cpp:L89-L91`
    pub fn reset_priorities(&mut self) {
        self.current_priority += 1;
    }
}

pub trait Loader: Send + Sync {
    fn size(&self) -> i64;
    fn load(&mut self, offset: i64);
    fn cancel(&mut self, offset: i64);
    fn reset_priorities(&mut self);
    fn set_priority(&mut self, priority: i32);
    fn stop(&mut self);
    fn speed_estimate(&self) -> SpeedEstimate;
}

/// Ledger Target Symbol: `MediaStreamingLoaderStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingLoaderStreaming {
    pub file_size: i64,
    pub priority_queue: PriorityQueue,
    pub is_stopped: bool,
    pub active_priority: i32,
    pub speed: SpeedEstimate,
}

impl Default for MediaStreamingLoaderStreaming {
    fn default() -> Self {
        Self {
            file_size: 0,
            priority_queue: PriorityQueue::new(),
            is_stopped: false,
            active_priority: 0,
            speed: SpeedEstimate::default(),
        }
    }
}

impl MediaStreamingLoaderStreaming {
    pub fn new(file_size: i64) -> Self {
        Self {
            file_size,
            priority_queue: PriorityQueue::new(),
            is_stopped: false,
            active_priority: 0,
            speed: SpeedEstimate::default(),
        }
    }
}

impl Loader for MediaStreamingLoaderStreaming {
    fn size(&self) -> i64 {
        self.file_size
    }

    fn load(&mut self, offset: i64) {
        if !self.is_stopped && offset >= 0 && offset < self.file_size {
            self.priority_queue.add(offset);
        }
    }

    fn cancel(&mut self, offset: i64) {
        self.priority_queue.remove(offset);
    }

    fn reset_priorities(&mut self) {
        self.priority_queue.reset_priorities();
    }

    fn set_priority(&mut self, priority: i32) {
        self.active_priority = priority;
    }

    fn stop(&mut self) {
        self.is_stopped = true;
        self.priority_queue.clear();
    }

    fn speed_estimate(&self) -> SpeedEstimate {
        self.speed
    }
}
