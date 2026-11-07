//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_pinned_list.h:L1-L64
//! - Telegram/SourceFiles/dialogs/dialogs_pinned_list.cpp:L1-L179

use crate::dialogs::key::DialogKey;
use serde::{Deserialize, Serialize};

/// Pinned dialogs order list managing 1-indexed pin positions and capacity limits.
/// Upstream: `dialogs_pinned_list.h:L22-61`, `dialogs_pinned_list.cpp:L20-177`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DialogsPinnedList {
    pub filter_id: i32,
    pub limit: usize,
    data: Vec<DialogKey>,
}

impl DialogsPinnedList {
    pub fn new(filter_id: i32, limit: usize) -> Self {
        assert!(limit > 0, "PinnedList limit must be positive");
        Self {
            filter_id,
            limit,
            data: Vec::new(),
        }
    }

    pub fn set_limit(&mut self, limit: usize) {
        assert!(limit > 0, "PinnedList limit must be positive");
        if self.limit == limit {
            return;
        }
        self.limit = limit;
        self.apply_limit(self.limit);
    }

    pub fn size(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn order(&self) -> &[DialogKey] {
        &self.data
    }

    pub fn contains(&self, key: &DialogKey) -> bool {
        self.data.contains(key)
    }

    /// Lookup 1-indexed pinned position (1 is topmost, 0 if unpinned).
    pub fn pinned_index(&self, key: &DialogKey) -> usize {
        self.data
            .iter()
            .position(|k| k == key)
            .map(|idx| idx + 1)
            .unwrap_or(0)
    }

    /// Add pinned dialog to the last place if not already pinned.
    /// Upstream: `dialogs_pinned_list.cpp:L42-52`
    pub fn add_pinned(&mut self, key: DialogKey) -> usize {
        self.add_pinned_get_position(key)
    }

    fn add_pinned_get_position(&mut self, key: DialogKey) -> usize {
        if let Some(pos) = self.data.iter().position(|k| k == &key) {
            return pos;
        }
        self.apply_limit(self.limit.saturating_sub(1));
        let position = self.data.len();
        self.data.push(key);
        position
    }

    /// Set pinned state. When pinned=true, rotates newly pinned dialog to the top (index 0).
    /// Upstream: `dialogs_pinned_list.cpp:L54-77`
    pub fn set_pinned(&mut self, key: DialogKey, pinned: bool) {
        if pinned {
            let position = self.add_pinned_get_position(key);
            if position > 0 {
                // std::rotate(begin, begin + position, begin + position + 1)
                self.data[0..=position].rotate_right(1);
            }
        } else if let Some(idx) = self.data.iter().position(|k| k == &key) {
            self.data.remove(idx);
        }
    }

    /// Swap positions of two pinned dialogs.
    /// Upstream: `dialogs_pinned_list.cpp:L167-176`
    pub fn reorder(&mut self, key1: DialogKey, key2: DialogKey) -> bool {
        let idx1 = self.data.iter().position(|k| k == &key1);
        let idx2 = self.data.iter().position(|k| k == &key2);

        if let (Some(i1), Some(i2)) = (idx1, idx2) {
            if i1 != i2 {
                self.data.swap(i1, i2);
                return true;
            }
        }
        false
    }

    /// Apply external order list from cloud session.
    /// Upstream: `dialogs_pinned_list.cpp:L91-165`
    pub fn apply_list(&mut self, list: Vec<DialogKey>) {
        self.clear();
        for key in list {
            if self.data.len() < self.limit && !self.data.contains(&key) {
                self.data.push(key);
            }
        }
    }

    pub fn apply_limit(&mut self, limit: usize) {
        while self.data.len() > limit {
            self.data.pop();
        }
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}
