//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupViewportTileGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_tile.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_viewport_tile.cpp:L23-L140`

use serde::{Deserialize, Serialize};

/// Upstream square dimension for paused video avatar fallback (90x90)
/// calls_group_viewport_tile.cpp:L23
pub const K_PAUSED_VIDEO_SIZE: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoQuality {
    Thumbnail,
    Medium,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoTileItem {
    pub endpoint_id: u64,
    pub peer_id: u64,
    pub is_self: bool,
    pub is_camera: bool,
    pub is_screencast: bool,
    pub is_pinned: bool,
    pub is_paused: bool,
    pub is_hidden: bool,
    pub current_quality: Option<VideoQuality>,
}

impl VideoTileItem {
    pub fn new(endpoint_id: u64, peer_id: u64, is_self: bool, is_camera: bool) -> Self {
        Self {
            endpoint_id,
            peer_id,
            is_self,
            is_camera,
            is_screencast: !is_camera,
            is_pinned: false,
            is_paused: false,
            is_hidden: false,
            current_quality: Some(VideoQuality::Medium),
        }
    }

    /// Front camera feeds for self are mirrored; screen shares and peer feeds are not
    /// calls_group_viewport_tile.cpp:L54-L56: mirror
    pub fn mirror(&self) -> bool {
        self.is_self && self.is_camera
    }

    /// Returns active video size or 90x90 paused fallback
    /// calls_group_viewport_tile.cpp:L81-L90: trackOrUserpicSize
    pub fn track_or_userpic_size(&self, width: u32, height: u32) -> (u32, u32) {
        if width > 0 && height > 0 && !self.is_paused {
            (width, height)
        } else if self.is_paused {
            (K_PAUSED_VIDEO_SIZE, K_PAUSED_VIDEO_SIZE)
        } else {
            (0, 0)
        }
    }

    /// Adapts stream quality; drops allocation when tile is hidden
    /// calls_group_viewport_tile.cpp:L122-L131: updateRequestedQuality
    pub fn update_requested_quality(&mut self, quality: VideoQuality) -> bool {
        if self.is_hidden {
            let changed = self.current_quality.is_some();
            self.current_quality = None;
            changed
        } else if self.current_quality != Some(quality) {
            self.current_quality = Some(quality);
            true
        } else {
            false
        }
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.is_pinned = pinned;
    }
}

/// Group Call Video Tiles Manager
/// Target Symbol: `CallsGroupViewportTileGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CallsGroupViewportTileGroup {
    pub tiles: Vec<VideoTileItem>,
}

impl CallsGroupViewportTileGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_tile(&mut self, endpoint_id: u64, peer_id: u64, is_self: bool, is_camera: bool) {
        if !self.tiles.iter().any(|t| t.endpoint_id == endpoint_id) {
            self.tiles.push(VideoTileItem::new(endpoint_id, peer_id, is_self, is_camera));
        }
    }

    pub fn remove_tile(&mut self, endpoint_id: u64) {
        self.tiles.retain(|t| t.endpoint_id != endpoint_id);
    }

    pub fn find_tile(&self, endpoint_id: u64) -> Option<&VideoTileItem> {
        self.tiles.iter().find(|t| t.endpoint_id == endpoint_id)
    }

    pub fn find_tile_mut(&mut self, endpoint_id: u64) -> Option<&mut VideoTileItem> {
        self.tiles.iter_mut().find(|t| t.endpoint_id == endpoint_id)
    }

    pub fn paused_video_size() -> (u32, u32) {
        (K_PAUSED_VIDEO_SIZE, K_PAUSED_VIDEO_SIZE)
    }
}
