//! Telegram Desktop 1:1 Submodule: `abstract_chart_view`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use std::collections::HashMap;
use crate::statistics::chart_lines_filter_controller::LinesFilterController;
use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::statistics_types::ChartValue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaintContext {
    pub x_indices: Limits,
    pub x_percentage_limits: Limits,
    pub height_limits: Limits,
    pub rect_x: f64,
    pub rect_y: f64,
    pub rect_width: f64,
    pub rect_height: f64,
    pub footer: bool,
}

impl Default for PaintContext {
    fn default() -> Self {
        Self {
            x_indices: Limits::new(0.0, 10.0),
            x_percentage_limits: Limits::new(0.0, 1.0),
            height_limits: Limits::new(0.0, 100.0),
            rect_x: 0.0,
            rect_y: 0.0,
            rect_width: 300.0,
            rect_height: 200.0,
            footer: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CachedSelectedPoints {
    pub last_x_index: i32,
    pub last_height_limits: Limits,
    pub last_x_limits: Limits,
    pub points: HashMap<i32, (f64, f64)>,
}

impl CachedSelectedPoints {
    pub fn is_same(&self, x: i32, c: &PaintContext) -> bool {
        self.last_x_index == x
            && self.last_height_limits == c.height_limits
            && self.last_x_limits == c.x_percentage_limits
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoubleLineRatios {
    pub first: f64,
    pub second: f64,
}

impl Default for DoubleLineRatios {
    fn default() -> Self {
        Self { first: 1.0, second: 1.0 }
    }
}

impl DoubleLineRatios {
    pub fn new(is_double: bool) -> Self {
        if is_double {
            Self { first: 0.0, second: 0.0 }
        } else {
            Self { first: 1.0, second: 1.0 }
        }
    }

    pub fn init(&mut self, chart_data: &StatisticalChart) {
        if chart_data.lines.len() != 2 {
            self.first = 1.0;
            self.second = 1.0;
        } else {
            let first_max = chart_data.lines[0].max_value;
            let second_max = chart_data.lines[1].max_value;
            if first_max > second_max {
                self.first = 1.0;
                self.second = first_max as f64 / (second_max as f64).max(1.0);
            } else {
                self.first = second_max as f64 / (first_max as f64).max(1.0);
                self.second = 1.0;
            }
        }
    }

    pub fn ratio(&self, line_id: i32) -> f64 {
        if line_id == 1 { self.first } else { self.second }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct HeightLimits {
    pub full: Limits,
    pub ranged: Limits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocalZoomType {
    #[default]
    Prepare,
    SkipCalculation,
    CheckAvailability,
    Process,
    SaveZoomFromFooter,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LocalZoomResult {
    pub has_zoom: bool,
    pub limit_indices: Limits,
    pub range: Limits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalZoomArgs {
    pub zoom_type: LocalZoomType,
    pub progress: f64,
    pub x_index: i32,
}

pub fn default_height_limits(
    ratios: &DoubleLineRatios,
    lines_filter: &LinesFilterController,
    chart_data: &mut StatisticalChart,
    x_indices: Limits,
) -> HeightLimits {
    let mut min_val = ChartValue::MAX;
    let mut max_val = 0;
    let mut min_full = ChartValue::MAX;
    let mut max_full = 0;

    let from = x_indices.min.max(0.0) as usize;
    let to = x_indices.max.max(0.0) as usize;

    for line in &mut chart_data.lines {
        if !lines_filter.is_enabled(line.id) {
            continue;
        }
        let r = ratios.ratio(line.id);
        let line_max = (line.segment_tree.r_max_q(from, to) as f64 * r) as ChartValue;
        let line_min = (line.segment_tree.r_min_q(from, to) as f64 * r) as ChartValue;

        max_val = max_val.max(line_max);
        min_val = min_val.min(line_min);

        max_full = max_full.max((line.max_value as f64 * r) as ChartValue);
        min_full = min_full.min((line.min_value as f64 * r) as ChartValue);
    }

    if max_val == min_val {
        max_val = chart_data.max_value;
        min_val = chart_data.min_value;
    }

    HeightLimits {
        full: Limits::new(min_full as f64, max_full as f64),
        ranged: Limits::new(min_val as f64, max_val as f64),
    }
}

/// Target symbol wrapper for compatibility
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AbstractChartViewView {
    pub ratios: DoubleLineRatios,
    pub selected_points: CachedSelectedPoints,
}

impl AbstractChartViewView {
    pub fn new() -> Self {
        Self::default()
    }
}
