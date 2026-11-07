//! Target Symbol: DataMessageReactionId
//! Telegram Desktop 1:1 Submodule: `data/data_message_reaction_id`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReactionIdentifier {
    Emoji(String),
    CustomEmoji(u64),
    PaidStars,
}

impl Default for ReactionIdentifier {
    fn default() -> Self {
        Self::Emoji("👍".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct DataMessageReactionId {
    pub raw: ReactionIdentifier,
}

impl DataMessageReactionId {
    pub fn emoji(emoji: impl Into<String>) -> Self {
        Self {
            raw: ReactionIdentifier::Emoji(emoji.into()),
        }
    }

    pub fn custom(document_id: u64) -> Self {
        Self {
            raw: ReactionIdentifier::CustomEmoji(document_id),
        }
    }

    pub fn paid_stars() -> Self {
        Self {
            raw: ReactionIdentifier::PaidStars,
        }
    }

    pub fn is_custom(&self) -> bool {
        matches!(self.raw, ReactionIdentifier::CustomEmoji(_))
    }

    pub fn is_emoji(&self) -> bool {
        matches!(self.raw, ReactionIdentifier::Emoji(_))
    }
}
