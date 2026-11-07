//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMessagesUiGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_messages_ui.h (L1-L204)
//!   - Telegram/SourceFiles/calls/group/calls_group_messages_ui.cpp (L1-L1943)

use super::calls_group_messages::CallChatMessage;
use serde::{Deserialize, Serialize};

/// Standard chat bubble background opacity.
/// Upstream: `calls_group_messages_ui.cpp:L65`
pub const K_MESSAGE_BG_OPACITY: f32 = 0.8;

/// Dark overlay opacity.
/// Upstream: `calls_group_messages_ui.cpp:L66`
pub const K_DARK_OVER_OPACITY: f32 = 0.25;

/// Colored highlight message background opacity.
/// Upstream: `calls_group_messages_ui.cpp:L67`
pub const K_COLORED_MESSAGE_BG_OPACITY: f32 = 0.65;

/// Admin badge text opacity.
/// Upstream: `calls_group_messages_ui.cpp:L68`
pub const K_ADMIN_BADGE_TEXT_OPACITY: f32 = 0.6;

/// Layout and styling metrics for an in-call message bubble.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BubbleLayout {
    pub width: f32,
    pub height: f32,
    pub bg_opacity: f32,
    pub is_donation: bool,
}

/// Parity component for `CallsGroupMessagesUiGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupMessagesUiGroup {
    pub pinned_message: Option<CallChatMessage>,
    pub max_visible_lines: usize,
    pub is_chat_hidden: bool,
}

impl CallsGroupMessagesUiGroup {
    pub fn new() -> Self {
        Self {
            pinned_message: None,
            max_visible_lines: 6,
            is_chat_hidden: false,
        }
    }

    pub fn pin_message(&mut self, msg: CallChatMessage) {
        self.pinned_message = Some(msg);
    }

    pub fn unpin_message(&mut self) {
        self.pinned_message = None;
    }

    pub fn toggle_chat_visibility(&mut self) -> bool {
        self.is_chat_hidden = !self.is_chat_hidden;
        !self.is_chat_hidden
    }

    /// Compute bubble layout metrics adhering to upstream opacities.
    pub fn layout_bubble(&self, msg: &CallChatMessage, available_width: f32) -> BubbleLayout {
        let max_w = (available_width * 0.75).clamp(160.0, 480.0);
        let approx_chars = msg.text.len();
        let estimated_lines = ((approx_chars as f32 / 32.0).ceil() as f32).clamp(1.0, 8.0);
        let height = 28.0 + estimated_lines * 18.0;

        let bg_opacity = if msg.stars_count > 0 {
            K_COLORED_MESSAGE_BG_OPACITY
        } else {
            K_MESSAGE_BG_OPACITY
        };

        BubbleLayout {
            width: max_w,
            height,
            bg_opacity,
            is_donation: msg.stars_count > 0,
        }
    }
}
