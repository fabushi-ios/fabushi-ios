//! Telegram Stories Recent Views & Reaction Counts Sheet
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_recent_views.h:L34-L137`
//! - `Telegram/SourceFiles/media/stories/media_stories_recent_views.cpp:L33-L670`

use serde::{Deserialize, Serialize};

/// Batch page size when loading viewer list.
/// Upstream: `media_stories_recent_views.cpp:L35`
pub const K_ADD_PER_PAGE: usize = 50;

/// Number of initial pages loaded.
/// Upstream: `media_stories_recent_views.cpp:L36`
pub const K_LOAD_VIEWS_PAGES: usize = 2;

/// Classification of the story view context.
/// Upstream: `media_stories_recent_views.h:L37-L42`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecentViewsType {
    #[default]
    Other,
    SelfStory,
    Channel,
    Changelog,
}

/// Determines the recent views mode based on peer properties.
/// Upstream: `media_stories_recent_views.cpp:L140-L152`
pub fn recent_views_type_for(
    is_video_stream: bool,
    is_self: bool,
    is_broadcast: bool,
    is_service_user: bool,
) -> RecentViewsType {
    if is_video_stream {
        RecentViewsType::Other
    } else if is_self {
        RecentViewsType::SelfStory
    } else if is_broadcast {
        RecentViewsType::Channel
    } else if is_service_user {
        RecentViewsType::Changelog
    } else {
        RecentViewsType::Other
    }
}

/// Checks if current user is allowed to view detailed reactions for a peer.
/// Upstream: `media_stories_recent_views.cpp:L154-L159`
pub fn can_view_reactions_for(is_channel: bool, is_creator: bool, has_admin_rights: bool) -> bool {
    if is_channel {
        is_creator || has_admin_rights
    } else {
        false
    }
}

/// Aggregated view and reaction metrics.
/// Upstream: `media_stories_recent_views.h:L44-L59`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecentViewsData {
    pub views_count: i32,
    pub reactions_count: i32,
    pub forwards_count: i32,
    pub total_count: i32,
    pub view_type: RecentViewsType,
    pub can_view_reactions: bool,
}

impl RecentViewsData {
    pub fn should_show_panel(&self) -> bool {
        self.view_type == RecentViewsType::SelfStory
            || (self.can_view_reactions && (self.reactions_count > 0 || self.forwards_count > 0))
    }
}

/// Recent views controller.
/// Upstream: `media_stories_recent_views.h:L66-L136`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesRecentViewsStories {
    pub data: RecentViewsData,
    pub current_page: usize,
    pub has_more: bool,
}

impl MediaStoriesRecentViewsStories {
    pub fn new(data: RecentViewsData) -> Self {
        let has_more = data.total_count > K_ADD_PER_PAGE as i32;
        Self {
            data,
            current_page: 0,
            has_more,
        }
    }

    pub fn load_next_page(&mut self) -> usize {
        self.current_page += 1;
        let loaded = self.current_page * K_ADD_PER_PAGE;
        if loaded >= self.data.total_count as usize {
            self.has_more = false;
        }
        loaded
    }
}
