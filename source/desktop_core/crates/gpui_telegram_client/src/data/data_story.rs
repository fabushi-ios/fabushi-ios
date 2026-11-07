//! Target Symbol: DataStory
//! Telegram Desktop 1:1 Submodule: `data/data_story`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StoryPrivacy {
    #[default]
    Public,
    CloseFriends,
    Contacts,
    SelectedContacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoryMediaAttachment {
    Photo { photo_id: u64 },
    Video { document_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StoryViews {
    pub views_count: u32,
    pub reactions_count: u32,
    pub forwards_count: u32,
    pub recent_viewer_peer_ids: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StoryArea {
    Location { latitude: f64, longitude: f64, title: String },
    Reaction { emoji: String },
    Url { url: String },
    ChannelPost { channel_id: u64, msg_id: i32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataStory {
    pub id: i32,
    pub peer_id: u64,
    pub date: i64,
    pub expires: i64,
    pub caption: String,
    pub privacy: StoryPrivacy,
    pub is_pinned: bool,
    pub is_edited: bool,
    pub media: Option<StoryMediaAttachment>,
    pub views: StoryViews,
    pub areas: Vec<StoryArea>,
}

impl Default for DataStory {
    fn default() -> Self {
        Self {
            id: 0,
            peer_id: 0,
            date: 0,
            expires: 0,
            caption: String::new(),
            privacy: StoryPrivacy::Public,
            is_pinned: false,
            is_edited: false,
            media: None,
            views: StoryViews::default(),
            areas: Vec::new(),
        }
    }
}

impl DataStory {
    pub fn new(id: i32, peer_id: u64, date: i64, duration_hours: u32) -> Self {
        let expires = date + (duration_hours as i64 * 3600);
        Self {
            id,
            peer_id,
            date,
            expires,
            ..Default::default()
        }
    }

    pub fn is_expired(&self, now: i64) -> bool {
        now >= self.expires
    }

    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = caption.into();
        self
    }

    pub fn with_privacy(mut self, privacy: StoryPrivacy) -> Self {
        self.privacy = privacy;
        self
    }

    pub fn with_photo(mut self, photo_id: u64) -> Self {
        self.media = Some(StoryMediaAttachment::Photo { photo_id });
        self
    }

    pub fn with_video(mut self, document_id: u64) -> Self {
        self.media = Some(StoryMediaAttachment::Video { document_id });
        self
    }

    pub fn add_area(&mut self, area: StoryArea) {
        self.areas.push(area);
    }
}
