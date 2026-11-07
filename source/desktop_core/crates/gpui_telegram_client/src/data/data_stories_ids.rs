//! Target Symbol: DataStoriesIds
//! Telegram Desktop 1:1 Submodule: `data/data_stories_ids`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataStoriesIds {
    pub ids: BTreeSet<i32>,
}

impl DataStoriesIds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, story_id: i32) -> bool {
        self.ids.insert(story_id)
    }

    pub fn remove(&mut self, story_id: i32) -> bool {
        self.ids.remove(&story_id)
    }

    pub fn contains(&self, story_id: i32) -> bool {
        self.ids.contains(&story_id)
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    pub fn latest(&self) -> Option<i32> {
        self.ids.iter().next_back().copied()
    }

    pub fn earliest(&self) -> Option<i32> {
        self.ids.iter().next().copied()
    }
}
