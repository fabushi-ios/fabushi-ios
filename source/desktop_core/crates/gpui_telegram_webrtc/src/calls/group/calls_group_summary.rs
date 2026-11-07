//! Group Call Summary & Metrics: `CallsGroupSummary`
//! Tracks call statistics, duration, and participant metrics upon conclusion.

use super::calls_group_common::format_call_duration;
use serde::{Deserialize, Serialize};

/// High-level metrics summarizing a completed group call session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallsGroupSummary {
    pub call_id: u64,
    pub start_timestamp: i64,
    pub end_timestamp: i64,
    pub peak_participants: usize,
    pub total_joined: usize,
    pub was_recorded: bool,
    pub was_rtmp_broadcast: bool,
}

impl CallsGroupSummary {
    pub fn new(call_id: u64, start: i64, end: i64) -> Self {
        Self {
            call_id,
            start_timestamp: start,
            end_timestamp: end,
            peak_participants: 0,
            total_joined: 0,
            was_recorded: false,
            was_rtmp_broadcast: false,
        }
    }

    pub fn duration_seconds(&self) -> u64 {
        (self.end_timestamp - self.start_timestamp).max(0) as u64
    }

    pub fn formatted_duration(&self) -> String {
        format_call_duration(self.duration_seconds())
    }
}
