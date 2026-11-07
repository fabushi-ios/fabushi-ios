//! Telegram Desktop 1:1 Parity Submodule: `CallsGroupStarsBoxGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_group_stars_box.h`
//! - `Telegram/SourceFiles/calls/group/calls_group_stars_box.cpp:L27-L140`

use serde::{Deserialize, Serialize};

/// Fallback maximum stars for video stream donations
/// calls_group_stars_box.cpp:L27-L28
pub const K_MAX_STARS_FALLBACK: i32 = 10_000;
pub const K_DEFAULT_STARS: i32 = 10;

/// Max stars count calculation clamped to minimum 2
/// calls_group_stars_box.cpp:L32-L39: MaxVideoStreamStarsCount
pub fn max_video_stream_stars_count(config_max: Option<i32>) -> i32 {
    let raw = config_max.unwrap_or(K_MAX_STARS_FALLBACK);
    raw.max(2)
}

/// Upstream PaidReactionTop representation
/// calls_group_stars_box.cpp:L69-L90
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaidReactionTopEntry {
    pub bare_peer_id: u64,
    pub name: String,
    pub count: i32,
    pub is_my: bool,
    pub is_anonymous: bool,
}

/// Sort top donors descending by star count, preserving stable tiebreaks
/// calls_group_stars_box.cpp:L115: ranges::stable_sort(..., ranges::greater(), count)
pub fn sort_top_donors(top: &mut [PaidReactionTopEntry]) {
    top.sort_by(|a, b| b.count.cmp(&a.count));
}

/// Video Stream Stars Box State & Controller
/// Target Symbol: `CallsGroupStarsBoxGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsGroupStarsBoxGroup {
    pub min_stars: i32,
    pub chosen_stars: i32,
    pub max_stars: i32,
    pub user_balance: i64,
    pub is_admin: bool,
    pub is_sending: bool,
    pub stream_name: String,
    pub top_donors: Vec<PaidReactionTopEntry>,
}

impl Default for CallsGroupStarsBoxGroup {
    fn default() -> Self {
        Self {
            min_stars: 1,
            chosen_stars: K_DEFAULT_STARS,
            max_stars: K_MAX_STARS_FALLBACK,
            user_balance: 0,
            is_admin: false,
            is_sending: false,
            stream_name: String::new(),
            top_donors: Vec::new(),
        }
    }
}

impl CallsGroupStarsBoxGroup {
    pub fn new(min_stars: i32, current_stars: i32, config_max: Option<i32>, is_admin: bool) -> Self {
        let max = max_video_stream_stars_count(config_max).max(min_stars);
        let chosen = if current_stars > 0 {
            current_stars
        } else {
            K_DEFAULT_STARS
        }.clamp(min_stars, max);

        Self {
            min_stars,
            chosen_stars: chosen,
            max_stars: max,
            user_balance: 0,
            is_admin,
            is_sending: false,
            stream_name: String::new(),
            top_donors: Vec::new(),
        }
    }

    pub fn set_chosen_stars(&mut self, amount: i32) {
        self.chosen_stars = amount.clamp(self.min_stars, self.max_stars);
    }

    pub fn add_or_update_donor(&mut self, bare_peer_id: u64, name: String, count: i32, is_my: bool, is_anonymous: bool) {
        if let Some(existing) = self.top_donors.iter_mut().find(|d| d.bare_peer_id == bare_peer_id) {
            existing.count = count;
            existing.is_my = is_my;
        } else {
            self.top_donors.push(PaidReactionTopEntry {
                bare_peer_id,
                name,
                count,
                is_my,
                is_anonymous,
            });
        }
        sort_top_donors(&mut self.top_donors);
    }

    pub fn can_submit(&self) -> bool {
        if self.is_admin {
            true
        } else {
            self.chosen_stars >= self.min_stars && self.user_balance >= (self.chosen_stars as i64)
        }
    }
}
