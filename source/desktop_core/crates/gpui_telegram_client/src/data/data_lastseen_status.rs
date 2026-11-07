//! Target Symbol: DataLastseenStatus
//! Telegram Desktop 1:1 Submodule: `DataLastseenStatus`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub const LIFE_START_DATE: i64 = 1375315200; // 2013-08-01 UTC
pub const RECENTLY_VALUE: u32 = 0;
pub const WITHIN_WEEK_VALUE: u32 = 1;
pub const WITHIN_MONTH_VALUE: u32 = 2;
pub const LONG_AGO_VALUE: u32 = 3;
pub const SPECIAL_VALUE_SKIP: u32 = 4;

/// Last seen and online presence status representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LastseenStatus {
    value: u32,
    available: bool,
    hidden_by_me: bool,
}

impl Default for LastseenStatus {
    fn default() -> Self {
        Self {
            value: 0,
            available: false,
            hidden_by_me: false,
        }
    }
}

impl LastseenStatus {
    pub const fn new(value: u32, available: bool, hidden_by_me: bool) -> Self {
        Self {
            value,
            available,
            hidden_by_me,
        }
    }

    pub const fn recently(by_me: bool) -> Self {
        Self::new(RECENTLY_VALUE, false, by_me)
    }

    pub const fn within_week(by_me: bool) -> Self {
        Self::new(WITHIN_WEEK_VALUE, false, by_me)
    }

    pub const fn within_month(by_me: bool) -> Self {
        Self::new(WITHIN_MONTH_VALUE, false, by_me)
    }

    pub const fn long_ago(by_me: bool) -> Self {
        Self::new(LONG_AGO_VALUE, false, by_me)
    }

    pub fn online_till(till: i64, local: bool, hidden_by_me: bool) -> Self {
        if till >= LIFE_START_DATE + SPECIAL_VALUE_SKIP as i64 {
            Self::new((till - LIFE_START_DATE) as u32, !local, hidden_by_me)
        } else {
            Self::long_ago(hidden_by_me)
        }
    }

    pub const fn is_hidden(&self) -> bool {
        !self.available
    }

    pub const fn is_recently(&self) -> bool {
        !self.available && self.value == RECENTLY_VALUE
    }

    pub const fn is_within_week(&self) -> bool {
        !self.available && self.value == WITHIN_WEEK_VALUE
    }

    pub const fn is_within_month(&self) -> bool {
        !self.available && self.value == WITHIN_MONTH_VALUE
    }

    pub const fn is_long_ago(&self) -> bool {
        !self.available && self.value == LONG_AGO_VALUE
    }

    pub const fn is_hidden_by_me(&self) -> bool {
        self.hidden_by_me
    }

    pub fn is_online(&self, now: i64) -> bool {
        self.value >= SPECIAL_VALUE_SKIP && (LIFE_START_DATE + self.value as i64) > now
    }

    pub fn online_till_timestamp(&self) -> i64 {
        if self.value >= SPECIAL_VALUE_SKIP {
            LIFE_START_DATE + self.value as i64
        } else {
            0
        }
    }

    pub fn serialize(&self) -> u32 {
        (self.value & 0x3FFF_FFFF)
            | ((self.available as u32) << 30)
            | ((self.hidden_by_me as u32) << 31)
    }

    pub fn from_serialized(value: u32) -> Self {
        Self {
            value: value & 0x3FFF_FFFF,
            available: ((value >> 30) & 1) != 0,
            hidden_by_me: ((value >> 31) & 1) != 0,
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataLastseenStatus coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataLastseenStatus {
    pub current_status: LastseenStatus,
}

impl DataLastseenStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_status(status: LastseenStatus) -> Self {
        Self {
            current_status: status,
        }
    }

    pub fn is_online(&self, now: i64) -> bool {
        self.current_status.is_online(now)
    }
}
