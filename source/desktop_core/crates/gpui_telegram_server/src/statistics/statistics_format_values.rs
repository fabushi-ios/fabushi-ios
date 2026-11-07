//! Telegram Desktop 1:1 Submodule: `statistics_format_values`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use chrono::{DateTime, Datelike, Timelike, Utc};

const MONTH_NAMES_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const WEEKDAY_NAMES_SHORT: [&str; 7] = [
    "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun",
];

fn parse_seconds(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap())
}

/// Upstream 1:1: `QString LangDayMonthYear(crl::time seconds)`
pub fn lang_day_month_year(seconds: i64) -> String {
    let dt = parse_seconds(seconds);
    let month_idx = (dt.month() as usize).saturating_sub(1).min(11);
    format!("{} {} {}", dt.day(), MONTH_NAMES_SHORT[month_idx], dt.year())
}

/// Upstream 1:1: `QString LangDayMonth(crl::time seconds)`
pub fn lang_day_month(seconds: i64) -> String {
    let dt = parse_seconds(seconds);
    let month_idx = (dt.month() as usize).saturating_sub(1).min(11);
    format!("{} {}", dt.day(), MONTH_NAMES_SHORT[month_idx])
}

/// Upstream 1:1: `QString LangDetailedDayMonth(crl::time seconds)`
pub fn lang_detailed_day_month(seconds: i64) -> String {
    let dt = parse_seconds(seconds);
    let month_idx = (dt.month() as usize).saturating_sub(1).min(11);
    let weekday_idx = (dt.weekday().number_from_monday() as usize).saturating_sub(1).min(6);

    if dt.hour() != 0 || dt.minute() != 0 {
        format!(
            "{}, {} {}, {:02}:{:02}",
            WEEKDAY_NAMES_SHORT[weekday_idx],
            dt.day(),
            MONTH_NAMES_SHORT[month_idx],
            dt.hour(),
            dt.minute()
        )
    } else {
        format!(
            "{}, {} {} {}",
            WEEKDAY_NAMES_SHORT[weekday_idx],
            dt.day(),
            MONTH_NAMES_SHORT[month_idx],
            dt.year()
        )
    }
}

/// Target symbol wrapper for compatibility
pub struct StatisticsFormatValues;

impl StatisticsFormatValues {
    pub fn day_month_year(seconds: i64) -> String {
        lang_day_month_year(seconds)
    }
    pub fn day_month(seconds: i64) -> String {
        lang_day_month(seconds)
    }
    pub fn detailed_day_month(seconds: i64) -> String {
        lang_detailed_day_month(seconds)
    }
}
