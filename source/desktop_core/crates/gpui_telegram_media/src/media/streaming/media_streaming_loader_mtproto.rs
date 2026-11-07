//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader_mtproto.h:L1-L82`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_loader_mtproto.cpp:L1-L222`
//! Parity Module: `media_streaming_loader_mtproto` | Feature: `TCG-N52` (MTProto Cloud Streaming Loader)

use super::media_streaming_common::SpeedEstimate;
use super::media_streaming_loader::{LoadedPart, Loader, PriorityQueue, K_PART_SIZE};
use serde::{Deserialize, Serialize};

/// `media_streaming_loader_mtproto.cpp:L19-L20`
pub const K_CHECK_STATS_INTERVAL_MS: i64 = 1000;
pub const K_INITIAL_STATS_WAIT_MS: i64 = 5000;
pub const K_MAX_SPEED_BPS: u32 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsEntry {
    pub start: i64,
    pub end: Option<i64>,
    pub offset: i64,
}

/// `media_streaming_loader_mtproto.cpp:L173-L218`
pub fn compute_speed_estimate(stats: &[StatsEntry], now: i64) -> SpeedEstimate {
    let from = now.saturating_sub(K_INITIAL_STATS_WAIT_MS);
    let active_stats: Vec<&StatsEntry> = stats
        .iter()
        .filter(|e| e.start >= from || e.end.map_or(true, |end| end >= from))
        .collect();

    if active_stats.is_empty() {
        return SpeedEstimate::default();
    }

    let mut duration_counted_till = active_stats[0].start;
    let mut duration: i64 = 0;
    let mut received: i64 = 0;

    for entry in &active_stats {
        if entry.start > duration_counted_till {
            duration_counted_till = entry.start;
        }
        let till = entry.end.unwrap_or(now);
        if till > duration_counted_till {
            duration += till - duration_counted_till;
            duration_counted_till = till;
        }
        if entry.end.is_some() {
            received += K_PART_SIZE as i64;
        }
    }

    if duration > 0 {
        let bps = (received.saturating_mul(1000) / duration) as u32;
        SpeedEstimate {
            bytes_per_second: bps.min(K_MAX_SPEED_BPS),
            unreliable: received < (3 * K_PART_SIZE as i64),
        }
    } else {
        SpeedEstimate::default()
    }
}

/// Ledger Target Symbol: `MediaStreamingLoaderMtprotoStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingLoaderMtprotoStreaming {
    pub file_size: i64,
    pub dc_id: u32,
    pub location_id: u64,
    pub priority: i32,
    pub requested_offsets: PriorityQueue,
    pub in_flight_offsets: Vec<i64>,
    pub stats: Vec<StatsEntry>,
    pub first_request_start: i64,
    pub is_stopped: bool,
}

impl Default for MediaStreamingLoaderMtprotoStreaming {
    fn default() -> Self {
        Self {
            file_size: 0,
            dc_id: 1,
            location_id: 0,
            priority: 0,
            requested_offsets: PriorityQueue::new(),
            in_flight_offsets: Vec::new(),
            stats: Vec::new(),
            first_request_start: 0,
            is_stopped: false,
        }
    }
}

impl MediaStreamingLoaderMtprotoStreaming {
    pub fn new(file_size: i64, dc_id: u32, location_id: u64) -> Self {
        Self {
            file_size,
            dc_id,
            location_id,
            priority: 0,
            requested_offsets: PriorityQueue::new(),
            in_flight_offsets: Vec::new(),
            stats: Vec::new(),
            first_request_start: 0,
            is_stopped: false,
        }
    }

    /// `media_streaming_loader_mtproto.cpp:L129-L141`
    pub fn take_next_request(&mut self, now: i64) -> Option<i64> {
        let offset = self.requested_offsets.take()?;
        if self.first_request_start == 0 {
            self.first_request_start = now;
        }
        self.in_flight_offsets.push(offset);
        self.stats.push(StatsEntry {
            start: now,
            end: None,
            offset,
        });
        Some(offset)
    }

    /// `media_streaming_loader_mtproto.cpp:L143-L159`
    pub fn feed_part(&mut self, offset: i64, bytes: Vec<u8>, now: i64) -> LoadedPart {
        self.in_flight_offsets.retain(|&o| o != offset);
        if let Some(entry) = self.stats.iter_mut().find(|e| e.offset == offset && e.end.is_none()) {
            entry.end = Some(now);
        }
        LoadedPart { offset, bytes }
    }
}

impl Loader for MediaStreamingLoaderMtprotoStreaming {
    fn size(&self) -> i64 {
        self.file_size
    }

    fn load(&mut self, offset: i64) {
        if !self.is_stopped && offset >= 0 && offset < self.file_size {
            if !self.in_flight_offsets.contains(&offset) {
                self.requested_offsets.add(offset);
            }
        }
    }

    fn cancel(&mut self, offset: i64) {
        self.requested_offsets.remove(offset);
        self.in_flight_offsets.retain(|&o| o != offset);
    }

    fn reset_priorities(&mut self) {
        self.requested_offsets.reset_priorities();
    }

    fn set_priority(&mut self, priority: i32) {
        self.priority = priority;
    }

    fn stop(&mut self) {
        self.is_stopped = true;
        self.requested_offsets.clear();
        self.in_flight_offsets.clear();
    }

    fn speed_estimate(&self) -> SpeedEstimate {
        compute_speed_estimate(&self.stats, chrono::Utc::now().timestamp_millis())
    }
}
