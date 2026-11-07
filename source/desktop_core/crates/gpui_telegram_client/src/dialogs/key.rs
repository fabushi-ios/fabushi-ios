//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_key.h:L1-L170
//! - Telegram/SourceFiles/dialogs/dialogs_key.cpp:L1-L116
//! - Telegram/SourceFiles/dialogs/ui/chat_search_in.h:L1-L110
//! - Telegram/SourceFiles/dialogs/ui/chat_search_in.cpp:L1-L478
//! - Telegram/SourceFiles/dialogs/ui/chat_search_empty.h:L1-L45
//! - Telegram/SourceFiles/dialogs/ui/posts_search_intro.h:L1-L60
//! - Telegram/SourceFiles/dialogs/ui/restore_windows_offer.h:L1-L61

use serde::{Deserialize, Serialize};

/// Dialog Key identifying an entry in chat lists.
/// Upstream: `dialogs_key.h:L30-65`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DialogKey {
    History(i64),
    Folder(i32),
    ForumTopic { peer_id: i64, topic_root_id: i32 },
    Thread { peer_id: i64, thread_root_id: i32 },
    SavedSublist(i64),
}

impl Default for DialogKey {
    fn default() -> Self {
        DialogKey::History(0)
    }
}

impl DialogKey {
    pub fn peer_id(&self) -> Option<i64> {
        match *self {
            DialogKey::History(peer_id) => Some(peer_id),
            DialogKey::ForumTopic { peer_id, .. } => Some(peer_id),
            DialogKey::Thread { peer_id, .. } => Some(peer_id),
            DialogKey::SavedSublist(peer_id) => Some(peer_id),
            DialogKey::Folder(_) => None,
        }
    }

    pub fn folder_id(&self) -> Option<i32> {
        match *self {
            DialogKey::Folder(id) => Some(id),
            _ => None,
        }
    }

    pub fn topic_id(&self) -> Option<i32> {
        match *self {
            DialogKey::ForumTopic { topic_root_id, .. } => Some(topic_root_id),
            _ => None,
        }
    }

    pub fn is_history(&self) -> bool {
        matches!(self, DialogKey::History(_))
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, DialogKey::Folder(_))
    }

    pub fn is_topic(&self) -> bool {
        matches!(self, DialogKey::ForumTopic { .. })
    }
}

/// Message descriptor within a row.
/// Upstream: `dialogs_key.h:L67-106`
#[derive(Debug, Clone, Copy, Eq, Serialize, Deserialize)]
pub struct RowDescriptor {
    pub key: DialogKey,
    pub full_msg_id: (i64, i32), // (peer_id, msg_id)
}

impl PartialEq for RowDescriptor {
    fn eq(&self, other: &Self) -> bool {
        // Upstream C++ logic: `(a.key == b.key) && ((a.fullId == b.fullId) || (!a.fullId.msg && !b.fullId.msg))`
        self.key == other.key
            && ((self.full_msg_id == other.full_msg_id)
                || (self.full_msg_id.1 == 0 && other.full_msg_id.1 == 0))
    }
}

impl PartialOrd for RowDescriptor {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RowDescriptor {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.key.cmp(&other.key) {
            std::cmp::Ordering::Equal => self.full_msg_id.cmp(&other.full_msg_id),
            ord => ord,
        }
    }
}

/// Navigation section inside an entry.
/// Upstream: `dialogs_key.h:L107-134`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntrySection {
    History,
    Profile,
    ChatsList,
    Scheduled,
    Pinned,
    Replies,
    SavedSublist,
    ContextMenu,
    SubsectionTabsMenu,
    ShortcutMessages,
    WelcomeMessages,
}

impl Default for EntrySection {
    fn default() -> Self {
        EntrySection::History
    }
}

/// State of an active entry in navigation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EntryState {
    pub key: DialogKey,
    pub section: EntrySection,
    pub filter_id: i32,
}

/// Chat type filter.
/// Upstream: `dialogs_key.h:L136-141`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChatTypeFilter {
    #[default]
    All,
    Private,
    Groups,
    Channels,
}

/// Search tab enumeration.
/// Upstream: `dialogs/ui/chat_search_in.h:L22-29`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChatSearchTab {
    #[default]
    MyMessages,
    ThisTopic,
    ThisPeer,
    PublicPosts,
    Archive,
    ThisCommunity,
}

/// Search peer tab category.
/// Upstream: `dialogs/ui/chat_search_in.h:L31-35`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChatSearchPeerTabType {
    #[default]
    Chat,
    Channel,
    Group,
}

/// Dialog Search State.
/// Upstream: `dialogs_key.h:L143-167`, `dialogs_key.cpp:L89-114`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DialogSearchState {
    pub in_chat: Option<DialogKey>,
    pub from_peer_id: Option<i64>,
    pub community_id: Option<i64>,
    pub is_community: bool,
    pub tags: Vec<String>,
    pub tab: ChatSearchTab,
    pub filter: ChatTypeFilter,
    pub from_archive: bool,
    pub query: String,
}

impl DialogSearchState {
    pub fn is_empty(&self) -> bool {
        self.in_chat.is_none() && self.tags.is_empty() && self.query.trim().is_empty()
    }

    /// Default tab determination.
    /// Upstream: `dialogs_key.cpp:L95-107`
    pub fn default_tab_for_me(&self) -> ChatSearchTab {
        match self.in_chat {
            Some(DialogKey::ForumTopic { .. }) => ChatSearchTab::ThisTopic,
            Some(_key) if self.is_community => ChatSearchTab::ThisCommunity,
            Some(DialogKey::History(_)) | Some(DialogKey::SavedSublist(_)) => {
                ChatSearchTab::ThisPeer
            }
            Some(DialogKey::Folder(_)) => ChatSearchTab::Archive,
            _ => ChatSearchTab::MyMessages,
        }
    }

    /// Chats list filter active check.
    /// Upstream: `dialogs_key.cpp:L109-113`
    pub fn filter_chats_list(&self) -> bool {
        self.in_chat.is_none()
            && (self.tab == ChatSearchTab::MyMessages || self.tab == ChatSearchTab::ThisPeer)
    }
}

/// Hashtag vs Cashtag classifier.
/// Upstream: `dialogs/ui/chat_search_in.h:L92-96`, `dialogs/ui/chat_search_in.cpp:L241-259`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HashOrCashtag {
    #[default]
    None,
    Hashtag,
    Cashtag,
}

pub fn is_hash_or_cashtag_search_query(query: &str) -> HashOrCashtag {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return HashOrCashtag::None;
    }
    let first = trimmed.chars().next().unwrap();
    if first == '#' {
        if trimmed.chars().any(|c| c.is_whitespace()) {
            HashOrCashtag::None
        } else {
            HashOrCashtag::Hashtag
        }
    } else if first == '$' {
        let rest = &trimmed[1..];
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_uppercase()) {
            HashOrCashtag::Cashtag
        } else {
            HashOrCashtag::None
        }
    } else {
        HashOrCashtag::None
    }
}

/// Fixed query and cursor position for hashtags/cashtags.
/// Upstream: `dialogs/ui/chat_search_in.h:L98-105`, `dialogs/ui/chat_search_in.cpp:L207-239`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedHashtagSearchQuery {
    pub text: String,
    pub cursor_position: usize,
}

pub fn fix_hashtag_search_query(
    query: &str,
    mut cursor_position: usize,
    tag: HashOrCashtag,
) -> FixedHashtagSearchQuery {
    let trimmed = query.trim();
    let hash = if trimmed.is_empty() {
        query.len()
    } else {
        query.find(trimmed).unwrap_or(0)
    };
    let start = cursor_position.min(hash);
    let first = if tag == HashOrCashtag::Cashtag {
        '$'
    } else {
        '#'
    };

    let mut result = query[..start].to_string();
    for ch in query[start..].chars() {
        if ch.is_whitespace() {
            if cursor_position > result.len() {
                cursor_position = cursor_position.saturating_sub(1);
            }
            continue;
        } else if result.len() == start {
            result.push(first);
            if ch != first {
                cursor_position += 1;
            }
        }
        if ch != first {
            result.push(ch);
        }
    }
    if result.len() == start {
        result.push(first);
        cursor_position += 1;
    }
    FixedHashtagSearchQuery {
        text: result,
        cursor_position,
    }
}

/// Icon for search empty state.
/// Upstream: `dialogs/ui/chat_search_empty.h:L14-17`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SearchEmptyIcon {
    #[default]
    Search,
    NoResults,
}

/// State for channel post search paywall / free searches left.
/// Upstream: `dialogs/ui/posts_search_intro.h:L20-31`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PostsSearchIntroState {
    pub query: String,
    pub free_searches_per_day: i32,
    pub free_searches_left: i32,
    pub next_free_search_time: i64,
    pub stars_per_paid_search: u32,
    pub needs_premium: bool,
}

/// Session crash / restore windows choice.
/// Upstream: `dialogs/ui/restore_windows_offer.h:L21-27`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RestoreWindowsChoice {
    #[default]
    Always,
    Once,
    Never,
    Dismiss,
    Detached,
}
