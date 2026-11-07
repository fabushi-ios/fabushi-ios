//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_indexed_list.h:L1-L104
//! - Telegram/SourceFiles/dialogs/dialogs_indexed_list.cpp:L1-L285
//! - Telegram/SourceFiles/dialogs/dialogs_common.h:L147-150

use std::collections::HashMap;
use crate::dialogs::entry::DialogEntryData;
use crate::dialogs::key::DialogKey;
use crate::dialogs::list::{DialogRowItem, DialogsList, SortMode};
use serde::{Deserialize, Serialize};

/// Link descriptor mapping an entry into the main list and per-letter buckets.
/// Upstream: `dialogs_common.h:L147-150`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RowsByLetter {
    pub main_key: DialogKey,
    pub letters: Vec<char>,
}

/// Indexed dialog list maintaining alphabetical buckets and date order.
/// Upstream: `dialogs_indexed_list.h:L19-102`, `dialogs_indexed_list.cpp:L16-283`
#[derive(Debug, Clone, Default)]
pub struct DialogsIndexedList {
    pub sort_mode: SortMode,
    pub filter_id: i32,
    pub date_version: u64,
    list: DialogsList,
    index: HashMap<char, DialogsList>,
}

impl DialogsIndexedList {
    pub fn new(sort_mode: SortMode, filter_id: i32) -> Self {
        Self {
            sort_mode,
            filter_id,
            date_version: 1,
            list: DialogsList::new(sort_mode, filter_id),
            index: HashMap::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.list.size()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn height(&self) -> i32 {
        self.list.height()
    }

    pub fn all(&self) -> &DialogsList {
        &self.list
    }

    pub fn all_mut(&mut self) -> &mut DialogsList {
        &mut self.list
    }

    pub fn get_row(&self, key: &DialogKey) -> Option<&DialogRowItem> {
        self.list.get_row(key)
    }

    pub fn contains(&self, key: &DialogKey) -> bool {
        self.list.contains(key)
    }

    pub fn clear(&mut self) {
        self.list.clear();
        self.index.clear();
    }

    /// Add an entry to the main list and all its first-letter index buckets.
    /// Upstream: `dialogs_indexed_list.cpp:L23-38`
    pub fn add_to_end(&mut self, entry: DialogEntryData) -> RowsByLetter {
        let key = DialogKey::History(entry.id);
        let letters = entry.first_letters();
        self.list.add_to_end(entry.clone());

        for &ch in &letters {
            let bucket = self.index
                .entry(ch)
                .or_insert_with(|| DialogsList::new(self.sort_mode, self.filter_id));
            bucket.add_to_end(entry.clone());
        }
        self.date_version += 1;
        RowsByLetter {
            main_key: key,
            letters,
        }
    }

    /// Add by alphabetical sort name.
    /// Upstream: `dialogs_indexed_list.cpp:L40-54`
    pub fn add_by_name(&mut self, entry: DialogEntryData) -> RowsByLetter {
        let key = DialogKey::History(entry.id);
        let letters = entry.first_letters();
        self.list.add_to_end(entry.clone());
        self.list.adjust_by_name(&key);

        for &ch in &letters {
            let bucket = self.index
                .entry(ch)
                .or_insert_with(|| DialogsList::new(self.sort_mode, self.filter_id));
            bucket.add_to_end(entry.clone());
            bucket.adjust_by_name(&key);
        }
        RowsByLetter {
            main_key: key,
            letters,
        }
    }

    /// Adjust entry position by date in main list.
    /// Upstream: `dialogs_indexed_list.cpp:L56-59`
    pub fn adjust_by_date(&mut self, key: &DialogKey) {
        if let Some(pos) = self.list.rows().iter().position(|r| &r.key == key) {
            self.list.adjust_by_date(pos);
            self.date_version += 1;
        }
    }

    /// Remove entry from main list and all buckets.
    /// Upstream: `dialogs_indexed_list.cpp:L205-213`
    pub fn remove(&mut self, key: &DialogKey) -> bool {
        let removed = self.list.remove(key);
        if removed {
            for bucket in self.index.values_mut() {
                bucket.remove(key);
            }
        }
        removed
    }

    /// Lookup filtered bucket by first letter with lazy date order sorting.
    /// Upstream: `dialogs_indexed_list.cpp:L220-232`
    pub fn filtered(&mut self, ch: char) -> Option<&DialogsList> {
        let upper = ch.to_uppercase().next().unwrap_or(ch);
        let date_ver = self.date_version;
        if let Some(bucket) = self.index.get_mut(&upper) {
            if self.sort_mode == SortMode::Date && bucket.date_order_version != date_ver {
                bucket.sort_by_date();
                bucket.date_order_version = date_ver;
            }
            Some(bucket)
        } else {
            None
        }
    }

    /// Multi-word prefix search with minimal bucket selection optimization.
    /// Upstream: `dialogs_indexed_list.cpp:L234-282`
    pub fn filtered_words(&mut self, words: &[String]) -> Vec<DialogRowItem> {
        if self.is_empty() || words.is_empty() {
            return Vec::new();
        }

        let non_empty_words: Vec<String> = words
            .iter()
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();

        if non_empty_words.is_empty() {
            return Vec::new();
        }

        // Minimal bucket selection: find smallest bucket by word[0]
        let mut minimal_char = None;
        let mut min_size = usize::MAX;

        for word in &non_empty_words {
            let ch = word.chars().next().unwrap();
            let upper = ch.to_uppercase().next().unwrap_or(ch);
            if let Some(bucket) = self.filtered(upper) {
                if bucket.is_empty() {
                    return Vec::new();
                }
                if bucket.size() < min_size {
                    min_size = bucket.size();
                    minimal_char = Some(upper);
                }
            } else {
                return Vec::new(); // Letter not in index -> 0 results
            }
        }

        let Some(min_ch) = minimal_char else {
            return Vec::new();
        };

        let bucket_rows = self.index.get(&min_ch).map(|b| b.rows().to_vec()).unwrap_or_default();
        let mut result = Vec::with_capacity(bucket_rows.len());

        for row in bucket_rows {
            let name_words = row.entry.name_words();
            let all_found = non_empty_words.iter().all(|search_word| {
                name_words.iter().any(|nw| nw.starts_with(search_word))
            });
            if all_found {
                result.push(row);
            }
        }

        result
    }

    pub fn freeze(&mut self) {
        self.list.freeze();
        for bucket in self.index.values_mut() {
            bucket.freeze();
        }
    }

    pub fn unfreeze(&mut self) {
        self.list.unfreeze();
        for bucket in self.index.values_mut() {
            bucket.unfreeze();
        }
    }
}
