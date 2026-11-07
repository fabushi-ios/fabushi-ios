//! Target Symbol: DataPeerCommon
//! Telegram Desktop 1:1 Submodule: `DataPeerCommon`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Channel / User Telegram Stars Rating details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct StarsRating {
    pub level: i32,
    pub stars: i32,
    pub this_level_stars: i32,
    pub next_level_stars: i32,
}

impl StarsRating {
    pub fn is_valid(&self) -> bool {
        self.level != 0 || self.this_level_stars != 0
    }
}

/// Pending stars rating update with timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StarsRatingPending {
    pub value: StarsRating,
    pub date: i64,
}

/// Target Symbol for Module 03 Ledger: DataPeerCommon coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerCommon {
    pub stars_rating: StarsRating,
}

impl DataPeerCommon {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_stars(level: i32, stars: i32) -> Self {
        Self {
            stars_rating: StarsRating {
                level,
                stars,
                this_level_stars: stars,
                next_level_stars: stars * 2,
            },
        }
    }
}
