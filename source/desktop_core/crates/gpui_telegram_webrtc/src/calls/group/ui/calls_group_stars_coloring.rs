//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupStarsColoringGroupUi`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_stars_coloring.h`
//! - `Telegram/SourceFiles/calls/group/ui/calls_group_stars_coloring.cpp:L20-L117`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarsColoringTier {
    pub bg_light: u32,
    pub bg_dark: u32,
    pub from_stars: i32,
    pub seconds_pin: u32,
    pub characters_max: usize,
    pub emoji_limit: usize,
}

/// Upstream lower-bound range search for star count tier
/// calls_group_stars_coloring.cpp:L20-L30: StarsColoringForCount
pub fn stars_coloring_for_count(tiers: &[StarsColoringTier], stars: i32) -> Option<StarsColoringTier> {
    if tiers.is_empty() {
        return None;
    }
    for (idx, tier) in tiers.iter().enumerate() {
        if tier.from_stars > stars {
            return if idx > 0 {
                Some(tiers[idx - 1].clone())
            } else {
                Some(tiers[0].clone())
            };
        }
    }
    tiers.last().cloned()
}

/// Determine minimum stars needed to send message within character and emoji bounds
/// calls_group_stars_coloring.cpp:L32-L55: StarsRequiredForMessage
pub fn stars_required_for_message(tiers: &[StarsColoringTier], char_len: usize, emoji_count: usize) -> i32 {
    if tiers.is_empty() {
        return 1;
    }
    for tier in tiers {
        if emoji_count <= tier.emoji_limit && char_len <= tier.characters_max {
            return tier.from_stars;
        }
    }
    tiers.last().map(|t| t.from_stars + 1).unwrap_or(1)
}

/// Format pin duration in hours, minutes, or seconds
/// calls_group_stars_coloring.cpp:L74-L80
pub fn format_pin_duration(seconds: u32) -> String {
    if seconds >= 3600 {
        format!("{}h", seconds / 3600)
    } else if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}s", seconds)
    }
}

/// Group Call Stars Level & Limits Controller
/// Target Symbol: `CallsGroupStarsColoringGroupUi`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupStarsColoringGroupUi {
    pub tiers: Vec<StarsColoringTier>,
    pub current_stars: i32,
    pub current_tier: Option<StarsColoringTier>,
}

impl Default for CallsGroupStarsColoringGroupUi {
    fn default() -> Self {
        let default_tiers = vec![
            StarsColoringTier { bg_light: 0xFFE1F5FE, bg_dark: 0xFF0D47A1, from_stars: 1, seconds_pin: 60, characters_max: 64, emoji_limit: 1 },
            StarsColoringTier { bg_light: 0xFFEDE7F6, bg_dark: 0xFF4A148C, from_stars: 10, seconds_pin: 300, characters_max: 128, emoji_limit: 3 },
            StarsColoringTier { bg_light: 0xFFFFF3E0, bg_dark: 0xFFE65100, from_stars: 50, seconds_pin: 900, characters_max: 256, emoji_limit: 5 },
            StarsColoringTier { bg_light: 0xFFFFEBEE, bg_dark: 0xFFB71C1C, from_stars: 200, seconds_pin: 3600, characters_max: 512, emoji_limit: 10 },
        ];
        let current_tier = default_tiers.first().cloned();
        Self {
            tiers: default_tiers,
            current_stars: 1,
            current_tier,
        }
    }
}

impl CallsGroupStarsColoringGroupUi {
    pub fn new(tiers: Vec<StarsColoringTier>) -> Self {
        let first = tiers.first().cloned();
        Self {
            tiers,
            current_stars: 1,
            current_tier: first,
        }
    }

    pub fn set_stars(&mut self, stars: i32) -> Option<&StarsColoringTier> {
        self.current_stars = stars;
        self.current_tier = stars_coloring_for_count(&self.tiers, stars);
        self.current_tier.as_ref()
    }

    pub fn stars_needed(&self, char_len: usize, emoji_count: usize) -> i32 {
        stars_required_for_message(&self.tiers, char_len, emoji_count)
    }

    pub fn pin_duration_text(&self) -> String {
        self.current_tier
            .as_ref()
            .map(|t| format_pin_duration(t.seconds_pin))
            .unwrap_or_else(|| "0s".to_string())
    }
}
