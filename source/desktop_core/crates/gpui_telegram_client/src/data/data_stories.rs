//! Target Symbol: DataStories
//! Telegram Desktop 1:1 Submodule: `data/data_stories`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataStories {
    pub peer_id: u64,
    pub active_story_ids: Vec<i32>,
    pub max_read_story_id: i32,
    pub pinned_story_ids: Vec<i32>,
    pub archive_story_ids: Vec<i32>,
    pub total_active_count: usize,
}

impl Default for DataStories {
    fn default() -> Self {
        Self {
            peer_id: 0,
            active_story_ids: Vec::new(),
            max_read_story_id: 0,
            pinned_story_ids: Vec::new(),
            archive_story_ids: Vec::new(),
            total_active_count: 0,
        }
    }
}

impl DataStories {
    pub fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            ..Default::default()
        }
    }

    pub fn set_active_stories(&mut self, ids: Vec<i32>, max_read_id: i32) {
        self.active_story_ids = ids;
        self.max_read_story_id = max_read_id;
        self.total_active_count = self.active_story_ids.len();
    }

    pub fn has_unseen_stories(&self) -> bool {
        self.active_story_ids.iter().any(|&id| id > self.max_read_story_id)
    }

    pub fn mark_read(&mut self, story_id: i32) {
        self.max_read_story_id = self.max_read_story_id.max(story_id);
    }

    pub fn unread_count(&self) -> usize {
        self.active_story_ids.iter().filter(|&&id| id > self.max_read_story_id).count()
    }
}
