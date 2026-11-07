//! Telegram Desktop 1:1 Submodule: `stack_linear_chart_common`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::chart_lines_filter_controller::LinesFilterController;
use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Part {
    pub rounded_percentage: f64,
    pub stacked_angle: f64,
    pub percentage_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PiePartData {
    pub parts: Vec<Part>,
    pub pie_has_single_part: bool,
}

pub fn pie_parts_percentage(sums: &[f64], total_sum: f64, round: bool) -> PiePartData {
    let mut result = PiePartData {
        parts: Vec::with_capacity(sums.len()),
        pie_has_single_part: false,
    };

    let mut stacked_percentage = 0.0;
    let mut sum_perc_diffs = 0.0;
    let mut max_perc_diff = 0.0;
    let mut min_perc_diff = 0.0;
    let mut max_perc_diff_index: Option<usize> = None;
    let mut min_perc_diff_index: Option<usize> = None;
    let mut rounded_percentages_sum = 0.0;

    for (k, &s) in sums.iter().enumerate() {
        let raw_percentage = if total_sum > 0.0 { s / total_sum } else { 0.0 };
        let rounded = if round {
            0.01 * (raw_percentage * 100.0).round()
        } else {
            raw_percentage
        };
        rounded_percentages_sum += rounded;
        let diff = raw_percentage - rounded;
        sum_perc_diffs += diff;
        let diff_abs = diff.abs();

        if max_perc_diff < diff_abs {
            max_perc_diff = diff_abs;
            max_perc_diff_index = Some(k);
        }
        if min_perc_diff < diff_abs {
            min_perc_diff = diff_abs;
            min_perc_diff_index = Some(k);
        }

        stacked_percentage += rounded;
        result.parts.push(Part {
            rounded_percentage: rounded,
            stacked_angle: stacked_percentage * 360.0 - 180.0,
            percentage_text: format!("{}%", (rounded * 100.0) as i64),
        });
        if rounded >= 1.0 {
            result.pie_has_single_part = true;
        }
    }

    if round && !result.parts.is_empty() {
        let target_idx = if rounded_percentages_sum > 1.0 {
            max_perc_diff_index.unwrap_or(0)
        } else {
            min_perc_diff_index.unwrap_or(0)
        };

        if target_idx < result.parts.len() {
            result.parts[target_idx].rounded_percentage += sum_perc_diffs;
            result.parts[target_idx].percentage_text = format!(
                "{}%",
                (result.parts[target_idx].rounded_percentage * 100.0) as i64
            );
            let angle_shrink = sum_perc_diffs * 360.0;
            for part in &mut result.parts {
                part.stacked_angle += angle_shrink;
            }
        }
    }

    result
}

pub fn pie_parts_percentage_by_indices(
    chart_data: &StatisticalChart,
    lines_filter: Option<&LinesFilterController>,
    x_indices: &Limits,
) -> PiePartData {
    let mut sums = Vec::with_capacity(chart_data.lines.len());
    let mut total_sum = 0.0;

    let from = x_indices.min.max(0.0) as usize;
    let to = x_indices.max.max(0.0) as usize;

    for line in &chart_data.lines {
        let mut sum = 0.0;
        let line_len = line.y.len();
        if line_len > 0 {
            let start = from.min(line_len - 1);
            let end = to.min(line_len - 1);
            for &val in &line.y[start..=end] {
                sum += val as f64;
            }
        }
        if let Some(filter) = lines_filter {
            sum *= filter.alpha(line.id);
        }
        total_sum += sum;
        sums.push(sum);
    }

    pie_parts_percentage(&sums, total_sum, true)
}

/// Target symbol wrapper for compatibility
pub struct StackLinearChartCommonView;

impl StackLinearChartCommonView {
    pub fn parts_percentage(sums: &[f64], total_sum: f64) -> PiePartData {
        pie_parts_percentage(sums, total_sum, true)
    }
}
