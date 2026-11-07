//! Telegram Desktop 1:1 Submodule: `chart_rulers_view`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::chart_rulers_data::ChartRulersData;
use crate::statistics::statistics_common::{ChartViewType, Limits, K_RULER_LINE_ALPHA};
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::statistics_types::ChartValue;
use serde::{Deserialize, Serialize};

pub const K_ONE_STAR_IN_NANO: f64 = 1_000_000_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartRulerLineGeometry {
    pub y_pos: f64,
    pub relative_value: f64,
    pub alpha: f64,
    pub caption: String,
    pub scaled_caption: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChartRulersViewView {
    pub is_double: bool,
    pub is_left_line_scaled: bool,
    pub scaled_line_ratio: f64,
    pub rulers: Vec<ChartRulersData>,
    pub alpha: f64,
}

impl ChartRulersViewView {
    pub fn new() -> Self {
        Self {
            is_double: false,
            is_left_line_scaled: false,
            scaled_line_ratio: 1.0,
            rulers: Vec::new(),
            alpha: 1.0,
        }
    }

    pub fn set_chart_data(&mut self, chart_data: &StatisticalChart, view_type: ChartViewType) {
        self.rulers.clear();
        self.is_double = matches!(view_type, ChartViewType::DoubleLinear) || chart_data.currency_rate > 0.0;

        if self.is_double && chart_data.lines.len() == 2 {
            let first_max = chart_data.lines[0].max_value;
            let second_max = chart_data.lines[1].max_value;
            if first_max > second_max {
                self.is_left_line_scaled = false;
                self.scaled_line_ratio = first_max as f64 / (second_max as f64).max(1.0);
            } else {
                self.is_left_line_scaled = true;
                self.scaled_line_ratio = second_max as f64 / (first_max as f64).max(1.0);
            }
        }
    }

    pub fn add(&mut self, new_height: Limits, _animated: bool) {
        let max_val = new_height.max as ChartValue;
        let min_val = new_height.min as ChartValue;
        let mut ruler = ChartRulersData::new(max_val, min_val, false, self.scaled_line_ratio);
        ruler.alpha = 1.0;
        self.rulers.push(ruler);
    }

    pub fn compute_relative(&mut self, new_max_height: f64, new_min_height: f64) {
        for ruler in &mut self.rulers {
            ruler.compute_relative(new_max_height, new_min_height);
        }
    }

    pub fn set_alpha(&mut self, value: f64) {
        self.alpha = value;
    }

    pub fn layout_ruler_lines(&self, rect_y: f64, rect_height: f64) -> Vec<ChartRulerLineGeometry> {
        let mut result = Vec::new();
        for ruler in &self.rulers {
            let line_alpha = self.alpha * ruler.alpha * K_RULER_LINE_ALPHA;
            for line in &ruler.lines {
                let y = rect_y + rect_height * line.relative_value;
                result.push(ChartRulerLineGeometry {
                    y_pos: y,
                    relative_value: line.relative_value,
                    alpha: line_alpha,
                    caption: line.caption.clone(),
                    scaled_caption: line.scaled_line_caption.clone(),
                });
            }
        }
        result
    }
}
