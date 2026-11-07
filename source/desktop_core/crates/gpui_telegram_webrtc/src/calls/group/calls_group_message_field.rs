//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMessageFieldGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_message_field.h (L1-L89)
//!   - Telegram/SourceFiles/calls/group/calls_group_message_field.cpp (L1-L689)

use serde::{Deserialize, Serialize};

/// Maximum message error limit threshold.
/// Upstream: `calls_group_message_field.cpp:L42`
pub const K_ERROR_LIMIT: usize = 99;

/// Maximum characters allowed per in-call message.
pub const K_MAX_MESSAGE_LENGTH: usize = 4096;

/// Quick reaction option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickReaction {
    pub emoji: String,
    pub count: u32,
    pub is_selected: bool,
}

/// Parity component for `CallsGroupMessageFieldGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupMessageFieldGroup {
    pub text: String,
    pub is_focused: bool,
    pub is_emoji_panel_open: bool,
    pub quick_reactions: Vec<QuickReaction>,
}

impl CallsGroupMessageFieldGroup {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            is_focused: false,
            is_emoji_panel_open: false,
            quick_reactions: vec![
                QuickReaction { emoji: "👍".to_string(), count: 0, is_selected: false },
                QuickReaction { emoji: "❤️".to_string(), count: 0, is_selected: false },
                QuickReaction { emoji: "🔥".to_string(), count: 0, is_selected: false },
                QuickReaction { emoji: "👏".to_string(), count: 0, is_selected: false },
            ],
        }
    }

    pub fn set_text(&mut self, text: &str) -> bool {
        if text.len() <= K_MAX_MESSAGE_LENGTH {
            self.text = text.to_string();
            true
        } else {
            false
        }
    }

    pub fn append_emoji(&mut self, emoji: &str) {
        if self.text.len() + emoji.len() <= K_MAX_MESSAGE_LENGTH {
            self.text.push_str(emoji);
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
    }

    pub fn can_send(&self) -> bool {
        !self.text.trim().is_empty() && self.text.len() <= K_MAX_MESSAGE_LENGTH
    }

    pub fn take_message_to_send(&mut self) -> Option<String> {
        if self.can_send() {
            let msg = self.text.trim().to_string();
            self.text.clear();
            Some(msg)
        } else {
            None
        }
    }

    pub fn toggle_reaction(&mut self, emoji: &str) {
        for r in &mut self.quick_reactions {
            if r.emoji == emoji {
                if r.is_selected {
                    r.is_selected = false;
                    r.count = r.count.saturating_sub(1);
                } else {
                    r.is_selected = true;
                    r.count = r.count.saturating_add(1);
                }
            }
        }
    }
}
