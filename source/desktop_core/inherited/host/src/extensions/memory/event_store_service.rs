//! Memory Host Event Store, Types & Bounded Sanitization
//!
//! Mirrors upstream OpenClaw `src/memory-host-sdk/event-store.ts` & `event-types.ts`:
//! Stores memory recall and promotion events with strict payload limits (10 items, 256B path, 2KB text)
//! and bounds diagnostic truncation while keeping aggregate counts exact.

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

pub const MEMORY_HOST_EVENTS_PLUGIN_ID: &str = "memory-core";
pub const MEMORY_HOST_EVENTS_NAMESPACE: &str = "memory-host.events";
pub const MEMORY_HOST_EVENT_CURSORS_NAMESPACE: &str = "memory-host.event-cursors";

pub const MAX_MEMORY_HOST_EVENTS: usize = 10_000;
pub const MAX_MEMORY_HOST_EVENT_CURSORS: usize = 1_000;
pub const MAX_MEMORY_HOST_EVENT_JSON_BYTES: usize = 8 * 1024;
pub const MAX_MEMORY_HOST_EVENT_ITEMS: usize = 10;
pub const MAX_MEMORY_HOST_EVENT_TEXT_BYTES: usize = 2 * 1024;
pub const MAX_MEMORY_HOST_EVENT_PATH_BYTES: usize = 256;
pub const WORKSPACE_HASH_BYTES: usize = 24;

/// Resolves retired JSONL source path for doctor migration discovery:
/// `<workspace_dir>/memory/.dreams/events.jsonl`
pub fn resolve_memory_host_event_log_path(workspace_dir: impl AsRef<Path>) -> PathBuf {
    workspace_dir.as_ref().join("memory").join(".dreams").join("events.jsonl")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallResult {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryHostPromotionCandidateItem {
    pub key: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    pub recall_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryHostEventRecord {
    #[serde(rename = "memory.recall.recorded")]
    RecallRecorded {
        timestamp: String,
        query: String,
        result_count: usize,
        results: Vec<MemoryRecallResult>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        storage_truncated: bool,
    },
    #[serde(rename = "memory.recall.skipped")]
    RecallSkipped {
        timestamp: String,
        query: String,
        reason: String,
        eligible_result_count: usize,
        skipped_result_count: usize,
        results: Vec<MemoryRecallResult>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        storage_truncated: bool,
    },
    #[serde(rename = "memory.promotion.applied")]
    PromotionApplied {
        timestamp: String,
        memory_path: String,
        applied: usize,
        candidates: Vec<MemoryHostPromotionCandidateItem>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        storage_truncated: bool,
    },
    #[serde(rename = "memory.dream.completed")]
    DreamCompleted {
        timestamp: String,
        phase: String,
        outcome: String, // "completed" | "failed"
        duration_ms: u64,
        candidates_seen: usize,
        promoted_count: usize,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        storage_truncated: bool,
    },
}

impl MemoryHostEventRecord {
    pub fn is_truncated(&self) -> bool {
        match self {
            Self::RecallRecorded { storage_truncated, .. } => *storage_truncated,
            Self::RecallSkipped { storage_truncated, .. } => *storage_truncated,
            Self::PromotionApplied { storage_truncated, .. } => *storage_truncated,
            Self::DreamCompleted { storage_truncated, .. } => *storage_truncated,
        }
    }
}

/// Clamps string to max bytes without splitting UTF-8 characters.
fn truncate_str_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Normalizes and bounds a memory host event for storage.
/// Clamps array items to 10, paths to 256 bytes, text to 2048 bytes, and sets storage_truncated = true if bounded.
pub fn normalize_memory_host_event_for_storage(event: MemoryHostEventRecord) -> MemoryHostEventRecord {
    match event {
        MemoryHostEventRecord::RecallRecorded {
            timestamp,
            query,
            result_count,
            mut results,
            mut storage_truncated,
        } => {
            let orig_len = results.len();
            if orig_len > MAX_MEMORY_HOST_EVENT_ITEMS {
                results.truncate(MAX_MEMORY_HOST_EVENT_ITEMS);
                storage_truncated = true;
            }
            let bounded_query = truncate_str_bytes(&query, MAX_MEMORY_HOST_EVENT_TEXT_BYTES);
            if bounded_query.len() < query.len() {
                storage_truncated = true;
            }
            for res in &mut results {
                let bounded_path = truncate_str_bytes(&res.path, MAX_MEMORY_HOST_EVENT_PATH_BYTES);
                if bounded_path.len() < res.path.len() {
                    res.path = bounded_path.to_string();
                    storage_truncated = true;
                }
            }
            MemoryHostEventRecord::RecallRecorded {
                timestamp,
                query: bounded_query.to_string(),
                result_count, // Keeps exact aggregate count!
                results,
                storage_truncated,
            }
        }
        MemoryHostEventRecord::PromotionApplied {
            timestamp,
            memory_path,
            applied,
            mut candidates,
            mut storage_truncated,
        } => {
            let orig_len = candidates.len();
            if orig_len > MAX_MEMORY_HOST_EVENT_ITEMS {
                candidates.truncate(MAX_MEMORY_HOST_EVENT_ITEMS);
                storage_truncated = true;
            }
            let bounded_mem_path = truncate_str_bytes(&memory_path, MAX_MEMORY_HOST_EVENT_PATH_BYTES);
            if bounded_mem_path.len() < memory_path.len() {
                storage_truncated = true;
            }
            for cand in &mut candidates {
                let bounded_path = truncate_str_bytes(&cand.path, MAX_MEMORY_HOST_EVENT_PATH_BYTES);
                if bounded_path.len() < cand.path.len() {
                    cand.path = bounded_path.to_string();
                    storage_truncated = true;
                }
            }
            MemoryHostEventRecord::PromotionApplied {
                timestamp,
                memory_path: bounded_mem_path.to_string(),
                applied,
                candidates,
                storage_truncated,
            }
        }
        other => other,
    }
}

/// Applies limit slice to recent events (taking the most recent up to `limit`).
pub fn apply_memory_host_event_limit<T: Clone>(events: &[T], limit: Option<usize>) -> Vec<T> {
    match limit {
        None => events.to_vec(),
        Some(0) => Vec::new(),
        Some(n) => {
            if events.len() <= n {
                events.to_vec()
            } else {
                events[events.len() - n..].to_vec()
            }
        }
    }
}
