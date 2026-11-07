//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_main_list.h:L1-L89
//! - Telegram/SourceFiles/dialogs/dialogs_main_list.cpp:L1-L242

use crate::dialogs::entry::{DialogEntryData, UnreadState};
use crate::dialogs::indexed_list::{DialogsIndexedList, RowsByLetter};
use crate::dialogs::key::DialogKey;
use crate::dialogs::list::SortMode;
use crate::dialogs::pinned_list::DialogsPinnedList;

/// Main composite dialog list holding indexed entries, pinned items, and unread counters.
/// Upstream: `dialogs_main_list.h:L24-77`, `dialogs_main_list.cpp:L19-240`
#[derive(Debug, Clone, Default)]
pub struct DialogsMainList {
    pub filter_id: i32,
    pub all: DialogsIndexedList,
    pub pinned: DialogsPinnedList,
    unread_state: UnreadState,
    cloud_unread_state: UnreadState,
    pub full_list_size: usize,
    cloud_list_size: usize,
    pub loaded: bool,
    pub all_are_muted: bool,
}

impl DialogsMainList {
    pub fn new(filter_id: i32, pinned_limit: usize) -> Self {
        let mut unread_state = UnreadState::default();
        unread_state.known = true;
        let mut cloud_unread_state = UnreadState::default();
        cloud_unread_state.known = false;

        Self {
            filter_id,
            all: DialogsIndexedList::new(SortMode::Date, filter_id),
            pinned: DialogsPinnedList::new(filter_id, pinned_limit.max(1)),
            unread_state,
            cloud_unread_state,
            full_list_size: 0,
            cloud_list_size: 0,
            loaded: false,
            all_are_muted: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn set_loaded(&mut self, loaded: bool) {
        if self.loaded != loaded {
            self.loaded = loaded;
            self.recompute_full_list_size();
        }
    }

    pub fn set_all_are_muted(&mut self, all_muted: bool) {
        self.all_are_muted = all_muted;
    }

    pub fn set_cloud_list_size(&mut self, size: usize) {
        if self.cloud_list_size != size {
            self.cloud_list_size = size;
            self.recompute_full_list_size();
        }
    }

    pub fn clear(&mut self) {
        self.pinned.clear();
        self.all.clear();
        self.unread_state = UnreadState { known: true, ..Default::default() };
        self.cloud_unread_state = UnreadState { known: true, ..Default::default() };
        self.cloud_list_size = 0;
        self.recompute_full_list_size();
    }

    /// Add an entry to the main list and update unread delta.
    /// Upstream: `dialogs_main_list.cpp:L93-101`
    pub fn add_entry(&mut self, entry: DialogEntryData) -> RowsByLetter {
        let unread = entry.unread_state;
        let links = self.all.add_to_end(entry);
        self.unread_entry_changed(&unread, true);
        self.recompute_full_list_size();
        links
    }

    /// Remove an entry from the main list and update unread delta.
    /// Upstream: `dialogs_main_list.cpp:L103-109`
    pub fn remove_entry(&mut self, key: &DialogKey) {
        if let Some(row) = self.all.get_row(key) {
            let unread = row.entry.unread_state;
            self.all.remove(key);
            self.unread_entry_changed(&unread, false);
            self.recompute_full_list_size();
        }
    }

    /// Recompute full list size accounting for cloud size before list fully loaded.
    /// Upstream: `dialogs_main_list.cpp:L111-113`
    pub fn recompute_full_list_size(&mut self) {
        self.full_list_size = self.all.size().max(if self.loaded { 0 } else { self.cloud_list_size });
    }

    /// Track unread state change delta for an existing entry.
    /// Upstream: `dialogs_main_list.cpp:L115-137`
    pub fn unread_state_changed(&mut self, was: &UnreadState, now: &UnreadState) {
        let update_cloud = self.cloud_unread_state.known && was.known && now.known;
        self.unread_state += *now - *was;
        if update_cloud {
            self.cloud_unread_state += *now - *was;
            self.finalize_cloud_unread();
        }
    }

    /// Track addition or removal of an unread entry.
    /// Upstream: `dialogs_main_list.cpp:L139-169`
    pub fn unread_entry_changed(&mut self, state: &UnreadState, added: bool) {
        if state.messages == 0
            && state.chats == 0
            && state.marks == 0
            && state.mentions == 0
            && state.reactions == 0
        {
            return;
        }
        let update_cloud = self.cloud_unread_state.known && state.known;
        if added {
            self.unread_state += *state;
        } else {
            self.unread_state -= *state;
        }
        if update_cloud {
            if added {
                self.cloud_unread_state += *state;
            } else {
                self.cloud_unread_state -= *state;
            }
            self.finalize_cloud_unread();
        }
    }

    /// Update cloud unread counters from folder metadata.
    /// Upstream: `dialogs_main_list.cpp:L171-181`
    pub fn update_cloud_unread(
        &mut self,
        unread_muted_messages: i32,
        unread_unmuted_messages: i32,
        unread_muted_peers: i32,
        unread_unmuted_peers: i32,
    ) {
        self.cloud_unread_state.messages = unread_muted_messages + unread_unmuted_messages;
        self.cloud_unread_state.chats = unread_muted_peers + unread_unmuted_peers;
        self.finalize_cloud_unread();
        self.cloud_unread_state.known = true;
    }

    /// Cloud unread state for archive counts all as muted, zero marks.
    /// Upstream: `dialogs_main_list.cpp:L187-194`
    pub fn finalize_cloud_unread(&mut self) {
        self.cloud_unread_state.messages_muted = self.cloud_unread_state.messages;
        self.cloud_unread_state.chats_muted = self.cloud_unread_state.chats;
        self.cloud_unread_state.marks = 0;
        self.cloud_unread_state.marks_muted = 0;
    }

    /// Effective unread state aggregating local or cloud counters.
    /// Upstream: `dialogs_main_list.cpp:L196-219`
    pub fn unread_state(&self) -> UnreadState {
        let use_cloud = self.cloud_unread_state.known && !self.loaded;
        let mut result = if use_cloud {
            self.cloud_unread_state
        } else {
            self.unread_state
        };

        if use_cloud {
            result.marks = self.unread_state.marks;
            result.marks_muted = self.unread_state.marks_muted;
        }
        if self.all_are_muted {
            result.messages_muted = result.messages;
            result.chats_muted = result.chats;
            result.marks_muted = result.marks;
        }
        result
    }
}
