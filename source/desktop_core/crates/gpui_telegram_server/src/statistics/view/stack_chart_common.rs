//! Telegram Desktop 1:1 Submodule: `stack_chart_common`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LeftStartAndStep {
    pub start: f64,
    pub step: f64,
}

pub fn compute_left_start_and_step(
    chart_data: &StatisticalChart,
    x_percentage_limits: &Limits,
    rect_x: f64,
    rect_width: f64,
    x_index_start: usize,
) -> LeftStartAndStep {
    let span = (x_percentage_limits.max - x_percentage_limits.min).max(f64::EPSILON);
    let full_width = rect_width / span;
    let offset = full_width * x_percentage_limits.min;

    let p = if chart_data.x_percentage.len() < 2 {
        1.0
    } else {
        chart_data.x_percentage[1] * full_width
    };

    let w = if chart_data.x_percentage.len() < 2 {
        1.0
    } else {
        chart_data.x_percentage[1] * (full_width - p)
    };

    let x_perc = chart_data
        .x_percentage
        .get(x_index_start)
        .copied()
        .unwrap_or(0.0);
    let left_start = rect_x + x_perc * (full_width - p) - offset;

    LeftStartAndStep {
        start: left_start,
        step: w,
    }
}

pub fn find_stack_x_indices_from_raw_x_percentages(
    chart_data: &StatisticalChart,
    raw_x_percentage_limits: &Limits,
    zoomed_in_limit_x_indices: &Limits,
) -> Limits {
    let n = chart_data.x_percentage.len();
    if n == 0 {
        return Limits::new(0.0, 0.0);
    }

    let min_idx = (zoomed_in_limit_x_indices.min.max(0.0) as usize).min(n - 1);
    let max_idx = (zoomed_in_limit_x_indices.max.max(0.0) as usize).min(n - 1);

    let zoom_limit_min = chart_data.x_percentage[min_idx];
    let zoom_limit_max = chart_data.x_percentage[max_idx];

    let offset = if zoom_limit_max == 1.0 { 0.0 } else { -1.0 };
    let right_shrink = if raw_x_percentage_limits.max == 1.0 {
        if zoom_limit_max == 1.0 { 0.0 } else { 1.0 }
    } else {
        0.0
    };

    let zoomed_in_min = zoom_limit_min + (zoom_limit_max - zoom_limit_min) * raw_x_percentage_limits.min;
    let zoomed_in_max = zoom_limit_min + (zoom_limit_max - zoom_limit_min) * raw_x_percentage_limits.max;

    let mut min_it = None;
    let mut max_it = None;

    for (i, &perc) in chart_data.x_percentage.iter().enumerate() {
        if min_it.is_none() && perc > zoomed_in_min {
            min_it = Some(i);
        }
        if max_it.is_none() && perc > zoomed_in_max {
            max_it = Some(i);
        }
    }

    let min_i = min_it.unwrap_or(0) as f64 + offset;
    let max_i = max_it.unwrap_or(n - 1) as f64 + offset;

    Limits {
        min: min_i.clamp(
            zoomed_in_limit_x_indices.min,
            zoomed_in_limit_x_indices.max - right_shrink,
        ),
        max: max_i.clamp(
            zoomed_in_limit_x_indices.min,
            zoomed_in_limit_x_indices.max - right_shrink,
        ),
    }
}

/// Target symbol wrapper for compatibility
pub struct StackChartCommonView;

impl StackChartCommonView {
    pub fn compute_start_and_step(
        chart_data: &StatisticalChart,
        x_percentage_limits: &Limits,
        rect_x: f64,
        rect_width: f64,
        x_index_start: usize,
    ) -> LeftStartAndStep {
        compute_left_start_and_step(
            chart_data,
            x_percentage_limits,
            rect_x,
            rect_width,
            x_index_start,
        )
    }
}
