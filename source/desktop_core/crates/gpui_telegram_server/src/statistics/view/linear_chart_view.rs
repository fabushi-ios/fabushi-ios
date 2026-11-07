//! Telegram Desktop 1:1 Submodule: `linear_chart_view`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::view::abstract_chart_view::{DoubleLineRatios, PaintContext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ChartPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CacheToken {
    pub x_indices: Limits,
    pub x_percentage_limits: Limits,
    pub height_limits: Limits,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinearChartViewView {
    pub is_double: bool,
    pub ratios: DoubleLineRatios,
    pub last_token: Option<CacheToken>,
}

impl Default for LinearChartViewView {
    fn default() -> Self {
        Self {
            is_double: false,
            ratios: DoubleLineRatios::new(false),
            last_token: None,
        }
    }
}

impl LinearChartViewView {
    pub fn new(is_double: bool) -> Self {
        Self {
            is_double,
            ratios: DoubleLineRatios::new(is_double),
            last_token: None,
        }
    }

    pub fn compute_polyline(
        &self,
        line_idx: usize,
        chart_data: &StatisticalChart,
        context: &PaintContext,
    ) -> Vec<ChartPoint> {
        let line = match chart_data.lines.get(line_idx) {
            Some(l) => l,
            None => return Vec::new(),
        };

        let ratio = self.ratios.ratio(line.id);
        let x_span = (context.x_percentage_limits.max - context.x_percentage_limits.min).max(f64::EPSILON);
        let y_span = (context.height_limits.max - context.height_limits.min).max(f64::EPSILON);

        let offset = 2.0;
        let start = (context.x_indices.min - offset).max(0.0) as usize;
        let end = ((context.x_indices.max + offset) as usize).min(chart_data.x_percentage.len().saturating_sub(1));

        let mut points = Vec::with_capacity(end.saturating_sub(start) + 1);

        for i in start..=end {
            if i >= line.y.len() || line.y[i] < 0 {
                continue;
            }
            let x_perc = chart_data.x_percentage.get(i).copied().unwrap_or(0.0);
            let x_point = context.rect_x + context.rect_width * ((x_perc - context.x_percentage_limits.min) / x_span);
            let y_perc = ((line.y[i] as f64 * ratio) - context.height_limits.min) / y_span;
            let y_point = context.rect_y + (1.0 - y_perc) * context.rect_height;
            points.push(ChartPoint { x: x_point, y: y_point });
        }

        points
    }

    pub fn find_x_index_by_position(
        &self,
        chart_data: &StatisticalChart,
        x_percentage_limits: &Limits,
        rect_width: f64,
        x: f64,
    ) -> usize {
        if chart_data.x_percentage.is_empty() || rect_width <= 0.0 {
            return 0;
        }

        let span = (x_percentage_limits.max - x_percentage_limits.min).max(f64::EPSILON);
        let relative_x = (x / rect_width).clamp(0.0, 1.0);
        let target_perc = x_percentage_limits.min + relative_x * span;

        match chart_data.x_percentage.binary_search_by(|p| p.partial_cmp(&target_perc).unwrap_or(std::cmp::Ordering::Equal)) {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    0
                } else if idx >= chart_data.x_percentage.len() {
                    chart_data.x_percentage.len() - 1
                } else {
                    let d_prev = (chart_data.x_percentage[idx - 1] - target_perc).abs();
                    let d_curr = (chart_data.x_percentage[idx] - target_perc).abs();
                    if d_prev < d_curr { idx - 1 } else { idx }
                }
            }
        }
    }
}
