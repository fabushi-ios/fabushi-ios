//! Telegram Desktop 1:1 Parity Submodule: `MediaViewGroupThumbsView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_group_thumbs.h`
//! - `Telegram/SourceFiles/media/view/media_view_group_thumbs.cpp:L34, L83-L107, L109-L177`

use serde::{Deserialize, Serialize};

pub const K_THUMB_DURATION_MS: i64 = 150;
pub const MEDIAVIEW_GROUP_HEIGHT: i32 = 80;
pub const MEDIAVIEW_GROUP_WIDTH: i32 = 56;
pub const MEDIAVIEW_GROUP_WIDTH_MAX: i32 = 160;
pub const MEDIAVIEW_GROUP_SKIP: i32 = 3;
pub const MEDIAVIEW_GROUP_SKIP_CURRENT: i32 = 12;

/// Key identifying an individual thumbnail in the group strip
/// Upstream: `media_view_group_thumbs.h:L58`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GroupThumbKey {
    Photo(u64),
    Message(u64),
    Collage(i32),
    InstantView(i32),
}

/// An individual thumbnail item in the strip
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupThumbItem {
    pub key: GroupThumbKey,
    pub current_left: i32,
    pub current_width: i32,
    pub final_left: i32,
    pub final_width: i32,
    pub is_current: bool,
}

/// Primary target symbol for `media_view_group_thumbs.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewGroupThumbsView {
    pub available_width: i32,
    pub current_index: usize,
    pub items: Vec<GroupThumbItem>,
    pub is_hidden: bool,
}

impl Default for MediaViewGroupThumbsView {
    fn default() -> Self {
        Self {
            available_width: 800,
            current_index: 0,
            items: Vec::new(),
            is_hidden: false,
        }
    }
}

impl MediaViewGroupThumbsView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets group keys and recalculates layout
    /// Upstream: `media_view_group_thumbs.cpp:L109-L177`
    pub fn refresh(&mut self, keys: &[GroupThumbKey], active_index: usize, available_width: i32) {
        self.available_width = available_width;
        self.current_index = active_index.min(keys.len().saturating_sub(1));
        self.items.clear();

        let mut left = 0;
        for (i, &key) in keys.iter().enumerate() {
            let is_current = i == self.current_index;
            let width = if is_current {
                MEDIAVIEW_GROUP_WIDTH_MAX.min(MEDIAVIEW_GROUP_WIDTH * 2)
            } else {
                MEDIAVIEW_GROUP_WIDTH
            };
            let skip = if is_current {
                MEDIAVIEW_GROUP_SKIP_CURRENT
            } else {
                MEDIAVIEW_GROUP_SKIP
            };

            self.items.push(GroupThumbItem {
                key,
                current_left: left,
                current_width: width,
                final_left: left,
                final_width: width,
                is_current,
            });

            left += width + skip;
        }
    }

    /// Finds thumb key under cursor position x
    pub fn hit_test(&self, x: i32) -> Option<GroupThumbKey> {
        for item in &self.items {
            if x >= item.final_left && x < (item.final_left + item.final_width) {
                return Some(item.key);
            }
        }
        None
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }
}
