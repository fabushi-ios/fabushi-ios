//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupMessagesGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_messages.h (L1-L199)
//!   - Telegram/SourceFiles/calls/group/calls_group_messages.cpp (L1-L731)

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Maximum number of messages kept in memory for live stream / group call chat.
/// Upstream: `calls_group_messages.cpp:L34`
pub const K_MAX_SHOWN_VIDEO_STREAM_MESSAGES: usize = 100;

/// Short poll interval for Telegram Stars donation statistics.
/// Upstream: `calls_group_messages.cpp:L35`
pub const K_STARS_STATS_SHORT_POLL_DELAY_MS: u64 = 30_000;

/// An individual in-call text chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallChatMessage {
    pub id: u64,
    pub sender_id: u64,
    pub sender_name: String,
    pub text: String,
    pub timestamp: i64,
    pub stars_count: u32,
}

/// Star gift donor leaderboard entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsDonor {
    pub user_id: u64,
    pub name: String,
    pub total_stars: u64,
}

/// Parity component for `CallsGroupMessagesGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsGroupMessagesGroup {
    pub messages: VecDeque<CallChatMessage>,
    pub donors: Vec<StarsDonor>,
    pub next_msg_id: u64,
}

impl CallsGroupMessagesGroup {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::with_capacity(K_MAX_SHOWN_VIDEO_STREAM_MESSAGES),
            donors: Vec::new(),
            next_msg_id: 1,
        }
    }

    pub fn push_message(&mut self, sender_id: u64, sender_name: &str, text: &str) -> u64 {
        let id = self.next_msg_id;
        self.next_msg_id += 1;

        if self.messages.len() >= K_MAX_SHOWN_VIDEO_STREAM_MESSAGES {
            self.messages.pop_front();
        }

        self.messages.push_back(CallChatMessage {
            id,
            sender_id,
            sender_name: sender_name.to_string(),
            text: text.to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            stars_count: 0,
        });

        id
    }

    pub fn push_star_donation(&mut self, sender_id: u64, sender_name: &str, stars: u32) -> u64 {
        let id = self.next_msg_id;
        self.next_msg_id += 1;

        if self.messages.len() >= K_MAX_SHOWN_VIDEO_STREAM_MESSAGES {
            self.messages.pop_front();
        }

        self.messages.push_back(CallChatMessage {
            id,
            sender_id,
            sender_name: sender_name.to_string(),
            text: format!("Sent {} Telegram Stars!", stars),
            timestamp: chrono::Utc::now().timestamp(),
            stars_count: stars,
        });

        // Update donor leaderboard
        if let Some(d) = self.donors.iter_mut().find(|d| d.user_id == sender_id) {
            d.total_stars += stars as u64;
        } else {
            self.donors.push(StarsDonor {
                user_id: sender_id,
                name: sender_name.to_string(),
                total_stars: stars as u64,
            });
        }
        self.donors.sort_by(|a, b| b.total_stars.cmp(&a.total_stars));

        id
    }

    pub fn delete_message(&mut self, message_id: u64) -> bool {
        if let Some(pos) = self.messages.iter().position(|m| m.id == message_id) {
            self.messages.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_messages(&self) -> Vec<CallChatMessage> {
        self.messages.iter().cloned().collect()
    }

    pub fn top_donors(&self, limit: usize) -> Vec<StarsDonor> {
        self.donors.iter().take(limit).cloned().collect()
    }
}
