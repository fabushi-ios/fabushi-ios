//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_common.h:L1-L171
//! - Telegram/SourceFiles/dialogs/dialogs_entry.h:L1-L240
//! - Telegram/SourceFiles/dialogs/dialogs_entry.cpp:L1-L532

use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::sync::atomic::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};

/// Global shift counter for date-based tie breaking.
/// Upstream: `dialogs_entry.cpp:L35` (`DialogsPosToTopShift`)
static DIALOGS_POS_TO_TOP_SHIFT: AtomicU64 = AtomicU64::new(0);

pub const K_ARCHIVE_FIX_ON_TOP_INDEX: u64 = 1;
pub const K_TOP_PROMOTION_FIX_ON_TOP_INDEX: u64 = 2;
pub const K_OPTION_DIALOGS_UNREAD_ON_TOP: &str = "dialogs-unread-on-top";

/// Generate a 64-bit sort position from a unix date and tie-breaker shift.
/// Upstream: `dialogs_entry.cpp:L37-42`
pub fn dialog_pos_from_date(date: i64) -> u64 {
    if date == 0 {
        return 0;
    }
    let shift = DIALOGS_POS_TO_TOP_SHIFT.fetch_add(1, Ordering::Relaxed) + 1;
    ((date as u64) << 32) | (shift & 0xFFFFFFFF)
}

/// Compute fixed on top position.
/// Upstream: `dialogs_entry.cpp:L44-46`
pub fn fixed_on_top_dialog_pos(index: u64) -> u64 {
    0xFFFFFFFFFFFF000F_u64.saturating_sub(index)
}

/// Compute pinned dialog position.
/// Upstream: `dialogs_entry.cpp:L48-50`
pub fn pinned_dialog_pos(pinned_index: u64) -> u64 {
    0xFFFFFFFF000000FF_u64.saturating_sub(pinned_index)
}

/// Compute unread on top dialog position.
/// Upstream: `dialogs_entry.cpp:L52-54`
pub fn unread_on_top_dialog_pos(sort_key_by_date: u64) -> u64 {
    sort_key_by_date | 0x8000000000000000_u64
}

/// Unread state vector for chats and folders.
/// Upstream: `dialogs_common.h:L38-80`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UnreadState {
    pub messages: i32,
    pub messages_muted: i32,
    pub chats: i32,
    pub chats_muted: i32,
    pub marks: i32,
    pub marks_muted: i32,
    pub reactions: i32,
    pub reactions_muted: i32,
    pub mentions: i32,
    pub polls: i32,
    pub polls_muted: i32,
    pub known: bool,
}

impl AddAssign for UnreadState {
    fn add_assign(&mut self, other: Self) {
        self.messages += other.messages;
        self.messages_muted += other.messages_muted;
        self.chats += other.chats;
        self.chats_muted += other.chats_muted;
        self.marks += other.marks;
        self.marks_muted += other.marks_muted;
        self.reactions += other.reactions;
        self.reactions_muted += other.reactions_muted;
        self.mentions += other.mentions;
        self.polls += other.polls;
        self.polls_muted += other.polls_muted;
    }
}

impl SubAssign for UnreadState {
    fn sub_assign(&mut self, other: Self) {
        self.messages -= other.messages;
        self.messages_muted -= other.messages_muted;
        self.chats -= other.chats;
        self.chats_muted -= other.chats_muted;
        self.marks -= other.marks;
        self.marks_muted -= other.marks_muted;
        self.reactions -= other.reactions;
        self.reactions_muted -= other.reactions_muted;
        self.mentions -= other.mentions;
        self.polls -= other.polls;
        self.polls_muted -= other.polls_muted;
    }
}

impl Add for UnreadState {
    type Output = Self;
    fn add(mut self, other: Self) -> Self {
        self += other;
        self
    }
}

impl Sub for UnreadState {
    type Output = Self;
    fn sub(mut self, other: Self) -> Self {
        self -= other;
        self
    }
}

/// Badges display state.
/// Upstream: `dialogs_common.h:L111-132`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BadgesState {
    pub unread_counter: i32,
    pub unread: bool,
    pub unread_muted: bool,
    pub mention: bool,
    pub mention_muted: bool,
    pub reaction: bool,
    pub reaction_muted: bool,
    pub poll: bool,
    pub poll_muted: bool,
}

impl BadgesState {
    pub fn is_empty(&self) -> bool {
        !self.unread && !self.mention && !self.reaction && !self.poll
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CountInBadge {
    #[default]
    Default,
    Chats,
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IncludeInBadge {
    #[default]
    Default,
    Unmuted,
    All,
    UnmutedOrAll,
}

/// Badges for unread calculation matching upstream C++ exactly.
/// Upstream: `dialogs_entry.cpp:L67-98`
pub fn badges_for_unread(
    state: &UnreadState,
    count: CountInBadge,
    include: IncludeInBadge,
    count_unread_messages: bool,
    include_muted_counter: bool,
) -> BadgesState {
    let count_messages = (count == CountInBadge::Messages)
        || ((count == CountInBadge::Default) && count_unread_messages);

    let counter_full = state.marks + if count_messages { state.messages } else { state.chats };
    let counter_muted = state.marks_muted + if count_messages { state.messages_muted } else { state.chats_muted };
    let unread_muted = counter_full <= counter_muted;

    let include_muted = (include == IncludeInBadge::All)
        || (include == IncludeInBadge::UnmutedOrAll && unread_muted)
        || ((include == IncludeInBadge::Default) && include_muted_counter);

    let marks = state.marks - if include_muted { 0 } else { state.marks_muted };
    let counter = counter_full - if include_muted { 0 } else { counter_muted };
    let mark = (counter == 1) && (marks == 1);

    BadgesState {
        unread_counter: if mark { 0 } else { counter },
        unread: counter > 0,
        unread_muted: include_muted && (counter <= counter_muted),
        mention: state.mentions > 0,
        mention_muted: false,
        reaction: state.reactions > 0,
        reaction_muted: state.reactions <= state.reactions_muted,
        poll: state.polls > 0,
        poll_muted: state.polls <= state.polls_muted,
    }
}

/// Cache for formatted date string and width.
/// Upstream: `dialogs_entry.h:L57-62`
#[derive(Debug, Clone, Default)]
pub struct DateTextCache {
    pub text: String,
    pub message_time_id: i64,
    pub today_serial: i32,
    pub width: i32,
}

impl DateTextCache {
    pub fn resolve_date_text(&mut self, date: i64, now: i64) -> (&str, i32) {
        let today_serial = (now / 86400) as i32;
        if self.message_time_id != date || self.today_serial != today_serial {
            self.text = format_dialog_date(date, now);
            self.width = (self.text.len() * 7) as i32; // Standard font metric approximation
            self.message_time_id = date;
            self.today_serial = today_serial;
        }
        (&self.text, self.width)
    }
}

/// Format unix timestamp into dialog list short date string.
pub fn format_dialog_date(date: i64, now: i64) -> String {
    if date <= 0 {
        return String::new();
    }
    let diff = now.saturating_sub(date);
    if diff < 86400 {
        let dt = chrono::DateTime::from_timestamp(date, 0)
            .unwrap_or_else(|| chrono::Utc::now());
        dt.format("%H:%M").to_string()
    } else if diff < 7 * 86400 {
        let dt = chrono::DateTime::from_timestamp(date, 0)
            .unwrap_or_else(|| chrono::Utc::now());
        dt.format("%a").to_string()
    } else {
        let dt = chrono::DateTime::from_timestamp(date, 0)
            .unwrap_or_else(|| chrono::Utc::now());
        dt.format("%d.%m.%y").to_string()
    }
}

/// Pure data model representing a Dialog Entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogEntryData {
    pub id: i64,
    pub title: String,
    pub sort_key_name: String,
    pub time_id: i64,
    pub pinned_index: i32,
    pub fixed_on_top_index: i32,
    pub unread_state: UnreadState,
    pub is_forum: bool,
    pub is_channel: bool,
    pub is_user: bool,
    pub has_filter_tags: bool,
    pub filter_color_index: Option<u8>,
}

impl Default for DialogEntryData {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            sort_key_name: String::new(),
            time_id: 0,
            pinned_index: 0,
            fixed_on_top_index: 0,
            unread_state: UnreadState::default(),
            is_forum: false,
            is_channel: false,
            is_user: false,
            has_filter_tags: false,
            filter_color_index: None,
        }
    }
}

impl DialogEntryData {
    pub fn new(id: i64, title: impl Into<String>) -> Self {
        let title_str = title.into();
        let sort_key_name = title_str.to_lowercase();
        Self {
            id,
            title: title_str,
            sort_key_name,
            ..Default::default()
        }
    }

    pub fn compute_sort_position(&self, unread_on_top: bool) -> u64 {
        if self.fixed_on_top_index > 0 {
            return fixed_on_top_dialog_pos(self.fixed_on_top_index as u64);
        }
        if self.pinned_index > 0 {
            return pinned_dialog_pos(self.pinned_index as u64);
        }
        let date_pos = dialog_pos_from_date(self.time_id);
        if unread_on_top && self.has_unread_unmuted_for_sort() {
            unread_on_top_dialog_pos(date_pos)
        } else {
            date_pos
        }
    }

    pub fn has_unread_unmuted_for_sort(&self) -> bool {
        let s = &self.unread_state;
        (s.messages > s.messages_muted)
            || (s.marks > s.marks_muted)
            || (s.reactions > s.reactions_muted)
            || (s.mentions > 0)
    }

    pub fn first_letters(&self) -> Vec<char> {
        self.title
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .collect()
    }

    pub fn name_words(&self) -> Vec<String> {
        self.title
            .split_whitespace()
            .map(|w| w.to_lowercase())
            .collect()
    }
}
