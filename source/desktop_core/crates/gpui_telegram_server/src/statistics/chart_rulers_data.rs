//! Telegram Desktop 1:1 Submodule: `chart_rulers_data`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_types::ChartValue;
use serde::{Deserialize, Serialize};

pub const K_MIN_LINES: i64 = 2;
pub const K_MAX_LINES: i64 = 6;
pub const K_STEP: f64 = 5.0;

/// Rounds chart values for clean decimal ruler markings.
#[inline]
pub fn round_chart_value(max_value: ChartValue) -> ChartValue {
    let k = max_value / (K_STEP as ChartValue);
    if k % 10 == 0 {
        max_value
    } else {
        (max_value / 10 + 1) * 10
    }
}

/// Upstream 1:1 struct: `ChartRulersData::Line`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Line {
    pub absolute_value: f64,
    pub relative_value: f64,
    pub caption: String,
    pub scaled_line_caption: String,
    pub right_caption_width: f64,
}

/// Upstream 1:1 struct: `ChartRulersData`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartRulersData {
    pub lines: Vec<Line>,
    pub alpha: f64,
    pub fixed_alpha: f64,
}

impl Default for ChartRulersData {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            alpha: 0.0,
            fixed_alpha: 1.0,
        }
    }
}

impl ChartRulersData {
    pub fn new(
        new_max_height: ChartValue,
        new_min_height: ChartValue,
        use_min_height: bool,
        right_ratio: f64,
    ) -> Self {
        let mut data = Self::default();

        if !use_min_height {
            let v = if new_max_height > 100 {
                round_chart_value(new_max_height)
            } else {
                new_max_height
            };

            let step = 1.max(((v as f64) / K_STEP).ceil() as ChartValue);
            let mut n = K_MAX_LINES;
            if v < K_MAX_LINES {
                n = 2.max(v + 1);
            } else if v / 2 < K_MAX_LINES {
                n = v / 2 + 1;
                if v % 2 != 0 {
                    n += 1;
                }
            }

            data.lines = (0..n)
                .map(|i| {
                    let absolute_value = (i * step) as f64;
                    Line {
                        absolute_value,
                        relative_value: 0.0,
                        caption: format_chart_count(absolute_value as ChartValue),
                        scaled_line_caption: String::new(),
                        right_caption_width: 0.0,
                    }
                })
                .collect();
        } else {
            let diff = new_max_height - new_min_height;
            let (n, step) = if diff == 0 {
                (K_MAX_LINES / 2, 1.0)
            } else if diff < K_MAX_LINES {
                (K_MIN_LINES.max(diff + 1), 1.0)
            } else if diff / 2 < K_MAX_LINES {
                (diff / 2 + diff % 2 + 1, 2.0)
            } else {
                let calculated_step = (diff as f64) / K_STEP;
                if calculated_step <= 0.0 {
                    (K_MIN_LINES.max(diff + 1), 1.0)
                } else {
                    (6, calculated_step)
                }
            };

            let diff_absolute = (n - 1) as f64 * step;
            data.lines = (0..n)
                .map(|i| {
                    let val = i as f64 * step;
                    let absolute_value = new_min_height as f64 + val;
                    let relative_value = if diff_absolute > 0.0 {
                        1.0 - (val / diff_absolute)
                    } else {
                        0.0
                    };
                    let caption = format_chart_count(absolute_value as ChartValue);
                    let scaled_line_caption = if right_ratio > 0.0 {
                        let v = (new_min_height as f64 + val) / right_ratio;
                        format_chart_count(v as ChartValue)
                    } else {
                        String::new()
                    };
                    Line {
                        absolute_value,
                        relative_value,
                        caption,
                        scaled_line_caption,
                        right_caption_width: 0.0,
                    }
                })
                .collect();
        }

        data
    }

    pub fn compute_relative(&mut self, new_max_height: f64, new_min_height: f64) {
        let span = (new_max_height - new_min_height).max(1.0);
        for line in &mut self.lines {
            line.relative_value = 1.0 - ((line.absolute_value - new_min_height) / span);
        }
    }

    pub fn lookup_height(max_value: ChartValue) -> ChartValue {
        let v = if max_value > 100 {
            round_chart_value(max_value)
        } else {
            max_value
        };
        let step = ((v as f64) / K_STEP).ceil() as ChartValue;
        step * (K_STEP as ChartValue)
    }
}

fn format_chart_count(val: ChartValue) -> String {
    if val >= 1_000_000 {
        format!("{:.1}M", val as f64 / 1_000_000.0)
    } else if val >= 10_000 {
        format!("{:.1}K", val as f64 / 1_000.0)
    } else {
        val.to_string()
    }
}
