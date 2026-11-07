//! Target Symbol: DataMessageReactions
//! Telegram Desktop 1:1 Submodule: `data/data_message_reactions`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use super::data_message_reaction_id::DataMessageReactionId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionCount {
    pub reaction: DataMessageReactionId,
    pub count: u32,
    pub is_chosen_by_me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentReactor {
    pub peer_id: u64,
    pub reaction: DataMessageReactionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataMessageReactions {
    pub message_id: i32,
    pub reactions: Vec<ReactionCount>,
    pub recent_reactors: Vec<RecentReactor>,
    pub can_see_list: bool,
}

impl DataMessageReactions {
    pub fn new(message_id: i32) -> Self {
        Self {
            message_id,
            reactions: Vec::new(),
            recent_reactors: Vec::new(),
            can_see_list: true,
        }
    }

    pub fn total_reactions(&self) -> u32 {
        self.reactions.iter().map(|r| r.count).sum()
    }

    pub fn toggle_reaction(&mut self, reaction: DataMessageReactionId) -> bool {
        if let Some(entry) = self.reactions.iter_mut().find(|r| r.reaction == reaction) {
            if entry.is_chosen_by_me {
                entry.is_chosen_by_me = false;
                entry.count = entry.count.saturating_sub(1);
                false
            } else {
                entry.is_chosen_by_me = true;
                entry.count = entry.count.saturating_add(1);
                true
            }
        } else {
            self.reactions.push(ReactionCount {
                reaction,
                count: 1,
                is_chosen_by_me: true,
            });
            true
        }
    }

    pub fn my_reactions(&self) -> Vec<&DataMessageReactionId> {
        self.reactions
            .iter()
            .filter(|r| r.is_chosen_by_me)
            .map(|r| &r.reaction)
            .collect()
    }
}
