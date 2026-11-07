//! Target Symbol: DataBirthday
//! Telegram Desktop 1:1 Submodule: `DataBirthday`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// User birthday container with day, month, and optional birth year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct Birthday {
    value: i32,
}

impl Birthday {
    pub const YEAR_MIN: i32 = 1875;
    pub const YEAR_MAX: i32 = 2100;

    pub fn new(day: i32, month: i32, year: i32) -> Self {
        if Self::validate(day, month, year) {
            Self {
                value: day + month * 100 + year * 10000,
            }
        } else {
            Self { value: 0 }
        }
    }

    pub fn validate(day: i32, month: i32, year: i32) -> bool {
        if year != 0 && (year < Self::YEAR_MIN || year > Self::YEAR_MAX) {
            return false;
        }
        if day < 1 || month < 1 || month > 12 {
            return false;
        }
        let max_days = match month {
            2 => {
                if day == 29 {
                    year == 0 || (year % 4 == 0 && (year % 100 != 0 || year % 400 == 0))
                } else {
                    day <= 28
                }
            }
            4 | 6 | 9 | 11 => day <= 30,
            _ => day <= 31,
        };
        max_days
    }

    pub fn from_serialized(value: i32) -> Self {
        let day = value % 100;
        let month = (value / 100) % 100;
        let year = value / 10000;
        Self::new(day, month, year)
    }

    pub fn serialize(&self) -> i32 {
        self.value
    }

    pub fn is_valid(&self) -> bool {
        self.value != 0
    }

    pub fn day(&self) -> i32 {
        self.value % 100
    }

    pub fn month(&self) -> i32 {
        (self.value / 100) % 100
    }

    pub fn year(&self) -> i32 {
        self.value / 10000
    }

    pub fn format_text(&self) -> String {
        let months = [
            "January", "February", "March", "April", "May", "June",
            "July", "August", "September", "October", "November", "December"
        ];
        let m = self.month() as usize;
        let m_str = if m >= 1 && m <= 12 { months[m - 1] } else { "" };
        if self.year() > 0 {
            format!("{} {}, {}", m_str, self.day(), self.year())
        } else {
            format!("{} {}", m_str, self.day())
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataBirthday coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataBirthday {
    pub birthday: Birthday,
}

impl DataBirthday {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_birthday(birthday: Birthday) -> Self {
        Self { birthday }
    }

    pub fn parse(day: i32, month: i32, year: i32) -> Birthday {
        Birthday::new(day, month, year)
    }
}
