//! Telegram Stories Interactive Reaction Stickers & Overlays
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_reactions.h:L36-L150`
//! - `Telegram/SourceFiles/media/stories/media_stories_reactions.cpp:L50-L1480`

use serde::{Deserialize, Serialize};

/// Scale target for reaction fly-out animation.
/// Upstream: `media_stories_reactions.cpp:L52`
pub const K_REACTION_SCALE_OUT_TARGET: f32 = 0.7;

/// Animation durations in milliseconds.
/// Upstream: `media_stories_reactions.cpp:L53-L54, L64`
pub const K_REACTION_SCALE_OUT_DURATION_MS: u64 = 1000;
pub const K_MESSAGE_REACTION_SCALE_OUT_DURATION_MS: u64 = 400;
pub const K_STOPPING_FADE_DURATION_MS: u64 = 150;

/// Geometric coefficients for suggested reaction sticker tail bubbles.
/// Upstream: `media_stories_reactions.cpp:L55-L63`
pub const K_SUGGESTED_BUBBLE_SIZE: f32 = 1.0;
pub const K_SUGGESTED_TAIL_BIG_SIZE: f32 = 0.264;
pub const K_SUGGESTED_TAIL_BIG_OFFSET: f32 = 0.464;
pub const K_SUGGESTED_TAIL_SMALL_SIZE: f32 = 0.110;
pub const K_SUGGESTED_TAIL_SMALL_OFFSET: f32 = 0.697;
pub const K_SUGGESTED_TAIL_BIG_ROTATION: f32 = -42.29;
pub const K_SUGGESTED_TAIL_SMALL_ROTATION: f32 = -40.87;
pub const K_SUGGESTED_REACTION_SIZE: f32 = 0.7;
pub const K_SUGGESTED_WITH_COUNT_SIZE: f32 = 0.55;

/// Interactive reaction sticker metrics.
/// Upstream: `media_stories_reactions.cpp:L55-L63`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReactionStickerGeometry {
    pub base_size: f32,
    pub tail_big_radius: f32,
    pub tail_big_offset: f32,
    pub tail_big_deg: f32,
    pub tail_small_radius: f32,
    pub tail_small_offset: f32,
    pub tail_small_deg: f32,
    pub icon_size: f32,
}

impl ReactionStickerGeometry {
    pub fn for_size(base_size: f32, with_count: bool) -> Self {
        let icon_factor = if with_count {
            K_SUGGESTED_WITH_COUNT_SIZE
        } else {
            K_SUGGESTED_REACTION_SIZE
        };

        Self {
            base_size,
            tail_big_radius: base_size * K_SUGGESTED_TAIL_BIG_SIZE,
            tail_big_offset: base_size * K_SUGGESTED_TAIL_BIG_OFFSET,
            tail_big_deg: K_SUGGESTED_TAIL_BIG_ROTATION,
            tail_small_radius: base_size * K_SUGGESTED_TAIL_SMALL_SIZE,
            tail_small_offset: base_size * K_SUGGESTED_TAIL_SMALL_OFFSET,
            tail_small_deg: K_SUGGESTED_TAIL_SMALL_ROTATION,
            icon_size: base_size * icon_factor,
        }
    }
}

/// Stories reaction overlay manager.
/// Upstream: `media_stories_reactions.h:L56-L148`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesReactionsStories {
    pub current_reaction: Option<String>,
    pub reaction_count: i32,
    pub is_active: bool,
}

impl MediaStoriesReactionsStories {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_liked(&mut self, reaction: Option<String>) {
        if self.current_reaction != reaction {
            if reaction.is_some() {
                self.reaction_count += 1;
            } else if self.reaction_count > 0 {
                self.reaction_count -= 1;
            }
            self.current_reaction = reaction;
        }
    }

    pub fn is_liked(&self) -> bool {
        self.current_reaction.is_some()
    }

    pub fn toggle_like(&mut self, default_reaction: &str) -> bool {
        if self.is_liked() {
            self.set_liked(None);
            false
        } else {
            self.set_liked(Some(default_reaction.to_string()));
            true
        }
    }
}
