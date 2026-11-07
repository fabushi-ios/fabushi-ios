//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_list.h:L1-L106
//! - Telegram/SourceFiles/dialogs/dialogs_list.cpp:L1-L270
//! - Telegram/SourceFiles/dialogs/dialogs_common.h:L26-36

use crate::dialogs::entry::DialogEntryData;
use crate::dialogs::key::DialogKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortMode {
    #[default]
    Date = 0,
    Name = 1,
    Add = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PositionChange {
    pub from: i32,
    pub to: i32,
    pub height: i32,
}

/// Representation of a single dialog item row in a list.
/// Upstream: `dialogs_row.h:L84-162`, `dialogs_row.cpp:L396-436`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogRowItem {
    pub key: DialogKey,
    pub entry: DialogEntryData,
    pub top: i32,
    pub height: i32,
    pub index: usize,
    pub sort_key: u64,
}

impl DialogRowItem {
    pub fn new(key: DialogKey, entry: DialogEntryData, index: usize, top: i32) -> Self {
        let sort_key = entry.compute_sort_position(false);
        let mut row = Self {
            key,
            entry,
            top,
            height: 62,
            index,
            sort_key,
        };
        row.recount_height(0.0);
        row
    }

    /// Recount row height based on style and narrow mode ratio.
    /// Upstream: `dialogs_row.cpp:L424-432`
    pub fn recount_height(&mut self, narrow_ratio: f64) {
        let base_height = if self.entry.is_forum {
            if self.entry.has_filter_tags { 96 } else { 80 }
        } else if self.entry.has_filter_tags {
            72
        } else {
            62
        };

        self.height = if base_height == 62 {
            62
        } else {
            // anim::interpolate(st.height, defaultDialogRow.height, narrowRatio)
            let diff = (62 - base_height) as f64 * narrow_ratio;
            (base_height as f64 + diff).round() as i32
        };
    }
}

/// Dialogs list maintaining sorted rows and coordinates.
/// Upstream: `dialogs_list.h:L17-104`, `dialogs_list.cpp:L16-268`
#[derive(Debug, Clone, Default)]
pub struct DialogsList {
    pub sort_mode: SortMode,
    pub filter_id: i32,
    pub date_order_version: u64,
    pub narrow_ratio: f64,
    pub frozen: bool,
    rows: Vec<DialogRowItem>,
    pending_adjust: Vec<DialogKey>,
}

impl DialogsList {
    pub fn new(sort_mode: SortMode, filter_id: i32) -> Self {
        Self {
            sort_mode,
            filter_id,
            date_order_version: 0,
            narrow_ratio: 0.0,
            frozen: false,
            rows: Vec::new(),
            pending_adjust: Vec::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rows(&self) -> &[DialogRowItem] {
        &self.rows
    }

    pub fn rows_mut(&mut self) -> &mut [DialogRowItem] {
        &mut self.rows
    }

    pub fn height(&self) -> i32 {
        self.rows
            .last()
            .map(|r| r.top + r.height)
            .unwrap_or(0)
    }

    pub fn contains(&self, key: &DialogKey) -> bool {
        self.rows.iter().any(|r| &r.key == key)
    }

    pub fn get_row(&self, key: &DialogKey) -> Option<&DialogRowItem> {
        self.rows.iter().find(|r| &r.key == key)
    }

    pub fn get_row_mut(&mut self, key: &DialogKey) -> Option<&mut DialogRowItem> {
        self.rows.iter_mut().find(|r| &r.key == key)
    }

    pub fn clear(&mut self) {
        self.frozen = false;
        self.pending_adjust.clear();
        self.rows.clear();
    }

    /// Add a dialog to the end and adjust by date if in date sort mode.
    /// Upstream: `dialogs_list.cpp:L27-41`
    pub fn add_to_end(&mut self, entry: DialogEntryData) -> usize {
        let key = DialogKey::History(entry.id);
        if let Some(pos) = self.rows.iter().position(|r| r.key == key) {
            return pos;
        }
        let top = self.height();
        let index = self.rows.len();
        let row = DialogRowItem::new(key, entry, index, top);
        self.rows.push(row);

        if self.sort_mode == SortMode::Date {
            self.adjust_by_date(index);
        }
        self.rows.iter().position(|r| r.key == key).unwrap_or(index)
    }

    /// Adjust row position by date key using slice rotation.
    /// Upstream: `dialogs_list.cpp:L84-120`
    pub fn adjust_by_date(&mut self, index: usize) {
        if index >= self.rows.len() {
            return;
        }
        if self.frozen {
            let key = self.rows[index].key;
            if !self.pending_adjust.contains(&key) {
                self.pending_adjust.push(key);
            }
            return;
        }

        let key_sort = self.rows[index].sort_key;
        // Check forward movement (lower sort_key than preceding items)
        if index + 1 < self.rows.len() && self.rows[index + 1].sort_key > key_sort {
            let mut before = self.rows.len();
            for j in (index + 2)..self.rows.len() {
                if self.rows[j].sort_key <= key_sort {
                    before = j;
                    break;
                }
            }
            self.rotate(index, index + 1, before);
        } else if index > 0 && self.rows[index - 1].sort_key < key_sort {
            let mut after = 0;
            for j in 0..index {
                if self.rows[j].sort_key <= key_sort {
                    after = j;
                    break;
                }
            }
            self.rotate(after, index, index + 1);
        }
    }

    /// Adjust row position by alphabetical name sort key.
    /// Upstream: `dialogs_list.cpp:L62-82`
    pub fn adjust_by_name(&mut self, key: &DialogKey) {
        let Some(index) = self.rows.iter().position(|r| &r.key == key) else {
            return;
        };
        let sort_key = self.rows[index].entry.sort_key_name.clone();

        let before = self.rows[index + 1..]
            .iter()
            .position(|r| r.entry.sort_key_name.cmp(&sort_key) >= std::cmp::Ordering::Equal)
            .map(|offset| index + 1 + offset)
            .unwrap_or(self.rows.len());

        if before != index + 1 {
            self.rotate(index, index + 1, before);
        } else if index > 0 {
            let after = self.rows[..index]
                .iter()
                .rposition(|r| r.entry.sort_key_name.cmp(&sort_key) <= std::cmp::Ordering::Equal)
                .map(|p| p + 1)
                .unwrap_or(0);
            if after != index {
                self.rotate(after, index, index + 1);
            }
        }
    }

    /// In-place slice rotation with coordinate & index recalculation.
    /// Upstream: `dialogs_list.cpp:L212-227`
    pub fn rotate(&mut self, first: usize, middle: usize, last: usize) {
        if first >= middle || middle >= last || last > self.rows.len() {
            return;
        }
        let mut top = self.rows[first].top;
        self.rows[first..last].rotate_left(middle - first);

        for i in first..last {
            self.rows[i].index = i;
            self.rows[i].top = top;
            top += self.rows[i].height;
        }

        // Recompute remaining tops after `last`
        for i in last..self.rows.len() {
            self.rows[i].index = i;
            self.rows[i].top = top;
            top += self.rows[i].height;
        }
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    pub fn unfreeze(&mut self) {
        self.frozen = false;
        let pending = std::mem::take(&mut self.pending_adjust);
        for key in pending {
            if let Some(pos) = self.rows.iter().position(|r| r.key == key) {
                self.adjust_by_date(pos);
            }
        }
        if !self.is_sorted_by_date() {
            self.sort_by_date();
        }
    }

    pub fn is_sorted_by_date(&self) -> bool {
        for i in 1..self.rows.len() {
            if self.rows[i - 1].sort_key < self.rows[i].sort_key {
                return false;
            }
        }
        true
    }

    /// Stable sort by date order.
    /// Upstream: `dialogs_list.cpp:L154-167`
    pub fn sort_by_date(&mut self) {
        self.rows.sort_by(|a, b| b.sort_key.cmp(&a.sort_key));
        let mut top = 0;
        for (i, row) in self.rows.iter_mut().enumerate() {
            row.index = i;
            row.top = top;
            top += row.height;
        }
    }

    /// Remove an item and shift subsequent rows up.
    /// Upstream: `dialogs_list.cpp:L229-250`
    pub fn remove(&mut self, key: &DialogKey) -> bool {
        let Some(index) = self.rows.iter().position(|r| &r.key == key) else {
            return false;
        };
        self.pending_adjust.retain(|k| k != key);
        let removed = self.rows.remove(index);
        let mut top = removed.top;
        for i in index..self.rows.len() {
            self.rows[i].index = i;
            self.rows[i].top = top;
            top += self.rows[i].height;
        }
        true
    }

    /// Binary search for a row by Y coordinate.
    /// Upstream: `dialogs_list.cpp:L252-267`
    pub fn row_at_y(&self, y: i32) -> Option<&DialogRowItem> {
        let index = self.find_by_y(y)?;
        let row = &self.rows[index];
        if y >= row.top && y < row.top + row.height {
            Some(row)
        } else {
            None
        }
    }

    /// Binary search matching `ranges::lower_bound` on `top + height - 1`.
    pub fn find_by_y(&self, y: i32) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let partition = self.rows.partition_point(|row| (row.top + row.height - 1) < y);
        if partition < self.rows.len() {
            Some(partition)
        } else {
            None
        }
    }
}
