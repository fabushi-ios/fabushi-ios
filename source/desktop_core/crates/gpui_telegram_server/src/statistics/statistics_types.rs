//! Telegram Desktop 1:1 Submodule: `statistics_types`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use serde::{Deserialize, Serialize};

/// Upstream 1:1 equivalent: `using ChartValue = int64;`
pub type ChartValue = i64;

/// Granularity of chart series points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeGranularity {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

/// Statistics type descriptors for Telegram channels and supergroups.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatisticsTypes {
    pub min_value: ChartValue,
    pub max_value: ChartValue,
    pub granularity: TimeGranularity,
    pub time_step_ms: i64,
}

impl Default for StatisticsTypes {
    fn default() -> Self {
        Self {
            min_value: 0,
            max_value: 100,
            granularity: TimeGranularity::Daily,
            time_step_ms: 86_400_000,
        }
    }
}

impl StatisticsTypes {
    pub fn new(min: ChartValue, max: ChartValue, step_ms: i64) -> Self {
        let granularity = if step_ms <= 3_600_000 {
            TimeGranularity::Hourly
        } else if step_ms <= 86_400_000 {
            TimeGranularity::Daily
        } else if step_ms <= 7 * 86_400_000 {
            TimeGranularity::Weekly
        } else {
            TimeGranularity::Monthly
        };
        Self {
            min_value: min,
            max_value: max,
            granularity,
            time_step_ms: step_ms,
        }
    }

    pub fn value_range(&self) -> ChartValue {
        self.max_value.saturating_sub(self.min_value).max(1)
    }
}
