//! Telegram Desktop 1:1 Submodule: `point_details_widget`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::statistics_format_values::{lang_day_month, lang_day_month_year};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Line {
    pub id: i32,
    pub name: String,
    pub value_str: String,
    pub percentage_str: String,
    pub color_hex: String,
    pub alpha: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PointDetailsWidgetWidgets {
    pub x_index: i32,
    pub alpha: f64,
    pub zoom_enabled: bool,
    pub header_text: String,
    pub lines: Vec<Line>,
    pub width: f64,
    pub height: f64,
}

impl PointDetailsWidgetWidgets {
    pub fn new(zoom_enabled: bool) -> Self {
        Self {
            x_index: -1,
            alpha: 1.0,
            zoom_enabled,
            header_text: String::new(),
            lines: Vec::new(),
            width: 160.0,
            height: 80.0,
        }
    }

    pub fn set_x_index(&mut self, chart_data: &StatisticalChart, x_index: usize) {
        if x_index >= chart_data.x.len() {
            return;
        }
        self.x_index = x_index as i32;

        let timestamp_ms = chart_data.x[x_index];
        let seconds = (timestamp_ms / 1000.0) as i64;
        self.header_text = if chart_data.week_format {
            let next_week_sec = seconds + (7 * 86400);
            format!("{} — {}", lang_day_month(seconds), lang_day_month_year(next_week_sec))
        } else {
            lang_day_month_year(seconds)
        };

        self.lines = chart_data
            .lines
            .iter()
            .map(|l| {
                let val = l.y.get(x_index).copied().unwrap_or(0);
                Line {
                    id: l.id,
                    name: l.name.clone(),
                    value_str: val.to_string(),
                    percentage_str: String::new(),
                    color_hex: l.color_hex.clone(),
                    alpha: 1.0,
                }
            })
            .collect();

        self.height = 30.0 + (self.lines.len() as f64 * 22.0) + 12.0;
        self.width = 180.0;
    }

    pub fn set_alpha(&mut self, alpha: f64) {
        self.alpha = alpha;
    }

    pub fn set_line_alpha(&mut self, line_id: i32, alpha: f64) {
        if let Some(line) = self.lines.iter_mut().find(|l| l.id == line_id) {
            line.alpha = alpha;
        }
    }
}
