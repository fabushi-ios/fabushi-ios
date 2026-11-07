//! Target Symbol: DataStatisticsChart
//! Telegram Desktop 1:1 Submodule: `data/data_statistics_chart`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartLine {
    pub name: String,
    pub values: Vec<f64>,
    pub color_rgb: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataStatisticsChart {
    pub chart_id: String,
    pub chart_title: String,
    pub timestamps: Vec<i64>,
    pub lines: Vec<ChartLine>,
    pub is_percentage: bool,
    pub is_stacked: bool,
}

impl Default for DataStatisticsChart {
    fn default() -> Self {
        Self {
            chart_id: String::new(),
            chart_title: String::new(),
            timestamps: Vec::new(),
            lines: Vec::new(),
            is_percentage: false,
            is_stacked: false,
        }
    }
}

impl DataStatisticsChart {
    pub fn new(chart_id: impl Into<String>, chart_title: impl Into<String>) -> Self {
        Self {
            chart_id: chart_id.into(),
            chart_title: chart_title.into(),
            ..Default::default()
        }
    }

    pub fn add_line(&mut self, name: impl Into<String>, values: Vec<f64>, color_rgb: u32) {
        self.lines.push(ChartLine {
            name: name.into(),
            values,
            color_rgb,
        });
    }

    pub fn set_timestamps(&mut self, ts: Vec<i64>) {
        self.timestamps = ts;
    }
}
