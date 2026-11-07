//! Target Symbol: DataBoosts
//! Telegram Desktop 1:1 Submodule: `DataBoosts`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BoostsOverview {
    pub is_group: bool,
    pub mine: i32,
    pub level: i32,
    pub boost_count: i32,
    pub current_level_boost_count: i32,
    pub next_level_boost_count: i32,
    pub premium_member_count: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Boost {
    pub id: String,
    pub user_id: u64,
    pub multiplier: i32,
    pub expires_at: i64,
}

/// Target Symbol for Module 03 Ledger: DataBoosts coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataBoosts {
    pub overview: BoostsOverview,
    pub boosts: Vec<Boost>,
}

impl DataBoosts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_overview(overview: BoostsOverview) -> Self {
        Self {
            overview,
            boosts: Vec::new(),
        }
    }

    pub fn level(&self) -> i32 {
        self.overview.level
    }

    pub fn can_unrestrict(&self) -> bool {
        self.overview.level >= 1
    }
}
