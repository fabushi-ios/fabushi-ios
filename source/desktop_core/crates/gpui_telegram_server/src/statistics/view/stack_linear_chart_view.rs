//! Telegram Desktop 1:1 Submodule: `stack_linear_chart_view`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_common::Limits;
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::view::abstract_chart_view::{LocalZoomArgs, LocalZoomResult, PaintContext};
use crate::statistics::view::stack_linear_chart_common::{pie_parts_percentage_by_indices, PiePartData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransitionStep {
    PrepareToZoomIn,
    PrepareToZoomOut,
    #[default]
    ZoomedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TransitionLine {
    pub start_x: f64,
    pub start_y: f64,
    pub end_x: f64,
    pub end_y: f64,
    pub angle: f64,
    pub sum: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Transition {
    pub lines: Vec<TransitionLine>,
    pub progress: f64,
    pub pending_prepare_to_zoom_in: bool,
    pub zoomed_out_x_indices: Limits,
    pub zoomed_in_limit_x_indices: Limits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StackLinearChartViewView {
    pub step: TransitionStep,
    pub transition: Transition,
    pub pie_data: PiePartData,
    pub is_pie_active: bool,
}

impl StackLinearChartViewView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prepare_zoom(&mut self, chart_data: &StatisticalChart, context: &PaintContext, step: TransitionStep) {
        self.step = step;
        match step {
            TransitionStep::PrepareToZoomIn => {
                self.pie_data = pie_parts_percentage_by_indices(chart_data, None, &context.x_indices);
                self.transition.pending_prepare_to_zoom_in = true;
                self.transition.progress = 0.0;
                self.is_pie_active = true;
            }
            TransitionStep::PrepareToZoomOut => {
                self.transition.progress = 1.0;
                self.is_pie_active = false;
            }
            TransitionStep::ZoomedOut => {
                self.transition.progress = 0.0;
                self.is_pie_active = false;
            }
        }
    }

    pub fn maybe_local_zoom(&mut self, _args: &LocalZoomArgs) -> LocalZoomResult {
        if self.is_pie_active {
            LocalZoomResult {
                has_zoom: true,
                limit_indices: Limits::new(0.0, 10.0),
                range: Limits::new(0.0, 1.0),
            }
        } else {
            LocalZoomResult {
                has_zoom: false,
                limit_indices: Limits::new(0.0, 0.0),
                range: Limits::new(0.0, 0.0),
            }
        }
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
