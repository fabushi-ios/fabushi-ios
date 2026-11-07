//! Telegram Desktop 1:1 Parity Submodule: `EditorMessageSource`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_message_source.h:L1-L59`, `Telegram/SourceFiles/editor/editor_message_source.cpp:L1-L136`

use super::editor_link_pill::LinkPreview;
use serde::{Deserialize, Serialize};

/// Returns true if a history item is eligible for story/editor rendering.
/// Upstream: `editor_message_source.cpp:L23-L27`
pub fn can_render_message(is_service: bool, is_empty: bool) -> bool {
    !is_service && !is_empty
}

/// Checks if content protection or self-destruct flags forbid visual rendering.
/// Upstream: `editor_message_source.cpp:L29-L38`
pub fn message_forbids_render(is_protected: bool, has_ttl: bool) -> bool {
    is_protected || has_ttl
}

/// Target symbol EditorMessageSource representing message data underlying a story element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorMessageSource {
    pub message_id: i64,
    pub peer_id: i64,
    pub link: Option<LinkPreview>,
    pub has_video: bool,
    pub is_removed: bool,
}

pub type MessageSource = EditorMessageSource;

impl Default for EditorMessageSource {
    fn default() -> Self {
        Self {
            message_id: 0,
            peer_id: 0,
            link: None,
            has_video: false,
            is_removed: false,
        }
    }
}

impl EditorMessageSource {
    pub fn new(message_id: i64, peer_id: i64) -> Self {
        Self {
            message_id,
            peer_id,
            ..Default::default()
        }
    }

    pub fn with_link(link: LinkPreview) -> Self {
        Self {
            link: Some(link),
            ..Default::default()
        }
    }

    pub fn mark_removed(&mut self) {
        self.is_removed = true;
    }
}
