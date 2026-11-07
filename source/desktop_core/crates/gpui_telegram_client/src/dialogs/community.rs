//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_community_chats_list.h:L1-L78
//! - Telegram/SourceFiles/dialogs/dialogs_community_chats_list.cpp:L1-L264
//! - Telegram/SourceFiles/dialogs/dialogs_community_requestable_list.h:L1-L45
//! - Telegram/SourceFiles/dialogs/dialogs_community_requestable_list.cpp:L1-L159
//! - Telegram/SourceFiles/dialogs/dialogs_community_rows_view.h:L1-L56
//! - Telegram/SourceFiles/dialogs/dialogs_community_rows_view.cpp:L1-L112

use serde::{Deserialize, Serialize};

/// Community chats view categorization.
/// Upstream: `dialogs_community_chats_list.h:L32-35`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CommunityChatsKind {
    #[default]
    Joined,
    Viewable,
}

/// Community chat item.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CommunityChatEntry {
    pub peer_id: i64,
    pub title: String,
    pub sort_key: u64,
    pub time_id: i64,
    pub is_joined: bool,
    pub is_broadcast: bool,
    pub members_count: Option<i32>,
}

/// Lightweight vertical layout container maintaining prefix-sum top offsets.
/// Upstream: `dialogs_community_rows_view.h:L17-53`, `dialogs_community_rows_view.cpp:L16-110`
#[derive(Debug, Clone, Default)]
pub struct CommunityRowsView {
    rows: Vec<CommunityChatEntry>,
    tops: Vec<i32>,
}

impl CommunityRowsView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[CommunityChatEntry] {
        &self.rows
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.tops.clear();
    }

    pub fn add(&mut self, entry: CommunityChatEntry) {
        self.rows.push(entry);
    }

    /// Compute cumulative tops prefix-sum array.
    /// Upstream: `dialogs_community_rows_view.cpp:L43-51`
    pub fn finalize(&mut self, row_height: i32) {
        self.tops.resize(self.rows.len() + 1, 0);
        let mut top = 0;
        for i in 0..self.rows.len() {
            self.tops[i] = top;
            top += row_height;
        }
        if let Some(last) = self.tops.last_mut() {
            *last = top;
        }
    }

    pub fn height(&self) -> i32 {
        self.tops.last().copied().unwrap_or(0)
    }

    pub fn row_top(&self, index: usize) -> Option<i32> {
        self.tops.get(index).copied()
    }

    /// Binary search for row index by vertical pixel Y offset.
    /// Upstream: `dialogs_community_rows_view.cpp:L64-70`
    pub fn index_by_y(&self, y: i32) -> Option<usize> {
        if self.tops.is_empty() || y < 0 || y >= self.height() {
            return None;
        }
        // std::upper_bound(begin(tops), end(tops), y) - 1
        let idx = self.tops.partition_point(|&top| top <= y);
        if idx > 0 {
            Some(idx - 1)
        } else {
            None
        }
    }

    pub fn contains_peer(&self, peer_id: i64) -> bool {
        self.rows.iter().any(|r| r.peer_id == peer_id)
    }
}

/// Controller managing community channel member chats.
/// Upstream: `dialogs_community_chats_list.h:L37-75`, `dialogs_community_chats_list.cpp:L41-140`
#[derive(Debug, Clone, Default)]
pub struct CommunityChatsList {
    pub community_id: i64,
    pub kind: CommunityChatsKind,
    pub view: CommunityRowsView,
    pub selected_index: Option<usize>,
}

impl CommunityChatsList {
    pub fn new(community_id: i64, kind: CommunityChatsKind) -> Self {
        Self {
            community_id,
            kind,
            view: CommunityRowsView::new(),
            selected_index: None,
        }
    }

    /// Rebuild chat rows with deterministic descending sort by (sortKey, timeId).
    /// Upstream: `dialogs_community_chats_list.cpp:L100-138`
    pub fn rebuild(&mut self, mut entries: Vec<CommunityChatEntry>, row_height: i32) {
        self.view.clear();
        self.selected_index = None;

        if self.kind == CommunityChatsKind::Joined {
            entries.retain(|e| e.is_joined);
            // Upstream: ranges::stable_sort(joined, ranges::greater(), [](history) { return pair(sortKey, timeId); })
            entries.sort_by(|a, b| {
                match b.sort_key.cmp(&a.sort_key) {
                    std::cmp::Ordering::Equal => b.time_id.cmp(&a.time_id),
                    ord => ord,
                }
            });
        } else {
            entries.retain(|e| !e.is_joined);
        }

        for entry in entries {
            self.view.add(entry);
        }
        self.view.finalize(row_height);
    }
}
