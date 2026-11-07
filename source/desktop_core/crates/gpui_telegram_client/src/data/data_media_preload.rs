//! Target Symbol: DataMediaPreload
//! Telegram Desktop 1:1 Submodule: `data/data_media_preload`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreloadTask {
    pub media_id: u64,
    pub is_video: bool,
    pub priority: u32,
    pub byte_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataMediaPreload {
    pub max_concurrent_preloads: usize,
    pub queue: VecDeque<PreloadTask>,
    pub active_preload_ids: Vec<u64>,
}

impl Default for DataMediaPreload {
    fn default() -> Self {
        Self {
            max_concurrent_preloads: 2,
            queue: VecDeque::new(),
            active_preload_ids: Vec::new(),
        }
    }
}

impl DataMediaPreload {
    pub fn new(max_concurrent_preloads: usize) -> Self {
        Self {
            max_concurrent_preloads,
            queue: VecDeque::new(),
            active_preload_ids: Vec::new(),
        }
    }

    pub fn enqueue(&mut self, task: PreloadTask) {
        if !self.queue.iter().any(|t| t.media_id == task.media_id)
            && !self.active_preload_ids.contains(&task.media_id)
        {
            self.queue.push_back(task);
        }
    }

    pub fn poll_next(&mut self) -> Option<PreloadTask> {
        if self.active_preload_ids.len() < self.max_concurrent_preloads {
            if let Some(task) = self.queue.pop_front() {
                self.active_preload_ids.push(task.media_id);
                return Some(task);
            }
        }
        None
    }

    pub fn finish(&mut self, media_id: u64) {
        self.active_preload_ids.retain(|&id| id != media_id);
    }

    pub fn cancel(&mut self, media_id: u64) {
        self.queue.retain(|t| t.media_id != media_id);
        self.active_preload_ids.retain(|&id| id != media_id);
    }
}
