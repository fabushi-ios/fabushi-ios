//! Target Symbol: DataBusinessCommonBusiness
//! Telegram Desktop 1:1 Submodule: `data/business/data_business_common`
//! Subsystem: `data`, Feature ID: `TCG-N27`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkingInterval {
    pub start_minute: u16, // 0..1439
    pub end_minute: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyWorkingHours {
    pub day_of_week: u8, // 1 = Monday, 7 = Sunday
    pub intervals: Vec<WorkingInterval>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataBusinessCommonBusiness {
    pub timezone_id: String,
    pub weekly_hours: Vec<DailyWorkingHours>,
    pub away_message_text: Option<String>,
    pub greeting_message_text: Option<String>,
    pub is_away_active: bool,
}

impl DataBusinessCommonBusiness {
    pub fn new(timezone_id: impl Into<String>) -> Self {
        Self {
            timezone_id: timezone_id.into(),
            weekly_hours: Vec::new(),
            away_message_text: None,
            greeting_message_text: None,
            is_away_active: false,
        }
    }

    pub fn set_away_message(&mut self, text: impl Into<String>, active: bool) {
        self.away_message_text = Some(text.into());
        self.is_away_active = active;
    }

    pub fn set_greeting_message(&mut self, text: impl Into<String>) {
        self.greeting_message_text = Some(text.into());
    }

    pub fn add_daily_hours(&mut self, day: u8, intervals: Vec<WorkingInterval>) {
        self.weekly_hours.retain(|d| d.day_of_week != day);
        self.weekly_hours.push(DailyWorkingHours {
            day_of_week: day,
            intervals,
        });
    }
}
