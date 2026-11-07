//! Telegram Desktop 1:1 Submodule: `bar_chart_view`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::chart_lines_filter_controller::LinesFilterController;
use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::view::abstract_chart_view::PaintContext;
use crate::statistics::view::stack_chart_common::compute_left_start_and_step;
use serde::{Deserialize, Serialize};

pub const K_SELECTED_ALPHA: f64 = 0.5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarColumnGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub line_id: i32,
    pub color_hex: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarChartViewView {
    pub is_stack: bool,
    pub last_selected_x_index: i32,
}

impl Default for BarChartViewView {
    fn default() -> Self {
        Self {
            is_stack: false,
            last_selected_x_index: -1,
        }
    }
}

impl BarChartViewView {
    pub fn new(is_stack: bool) -> Self {
        Self {
            is_stack,
            last_selected_x_index: -1,
        }
    }

    pub fn compute_columns(
        &self,
        chart_data: &StatisticalChart,
        context: &PaintContext,
        lines_filter: &LinesFilterController,
    ) -> Vec<BarColumnGeometry> {
        let n = chart_data.x_percentage.len();
        if n == 0 {
            return Vec::new();
        }

        let local_start = (context.x_indices.min - 2.0).max(0.0) as usize;
        let local_end = ((context.x_indices.max + 2.0) as usize).min(n - 1);

        if local_start > local_end {
            return Vec::new();
        }

        let step_info = compute_left_start_and_step(
            chart_data,
            &context.x_percentage_limits,
            context.rect_x,
            context.rect_width,
            local_start,
        );

        let count = local_end - local_start + 1;
        let mut bottoms = vec![0.0f64; count];
        let y_span = (context.height_limits.max - context.height_limits.min).max(1.0);
        let mut columns = Vec::new();

        for line in &chart_data.lines {
            let alpha = lines_filter.alpha(line.id);
            if alpha <= 0.0 {
                continue;
            }

            for x in local_start..=local_end {
                if x >= line.y.len() {
                    continue;
                }
                let val = line.y[x];
                if val <= 0 && self.is_stack {
                    continue;
                }

                let y_percentage = (val as f64 - context.height_limits.min) / y_span;
                let y_point = y_percentage * context.rect_height * alpha;
                let bottom_idx = x - local_start;

                let col_x = step_info.start + (bottom_idx as f64) * step_info.step;
                let col_y = context.rect_y + context.rect_height - bottoms[bottom_idx] - y_point;

                columns.push(BarColumnGeometry {
                    x: col_x,
                    y: col_y,
                    width: step_info.step,
                    height: y_point,
                    line_id: line.id,
                    color_hex: line.color_hex.clone(),
                });

                if self.is_stack {
                    bottoms[bottom_idx] += y_point;
                }
            }
        }

        columns
    }

    pub fn find_x_index_by_position(
        &self,
        chart_data: &StatisticalChart,
        x_percentage_limits: &Limits,
        rect_width: f64,
        x: f64,
    ) -> usize {
        let n = chart_data.x_percentage.len();
        if n == 0 || rect_width <= 0.0 {
            return 0;
        }

        let span = (x_percentage_limits.max - x_percentage_limits.min).max(f64::EPSILON);
        let rel = (x / rect_width).clamp(0.0, 1.0);
        let target = x_percentage_limits.min + rel * span;

        for (i, &p) in chart_data.x_percentage.iter().enumerate() {
            if p >= target {
                return i;
            }
        }
        n - 1
    }
}
