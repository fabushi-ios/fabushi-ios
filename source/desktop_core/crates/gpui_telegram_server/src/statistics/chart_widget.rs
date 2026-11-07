//! Telegram Desktop 1:1 Submodule: `chart_widget`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::chart_lines_filter_controller::LinesFilterController;
use crate::statistics::statistics_common::{ChartViewType, Limits};
use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::view::chart_rulers_view::ChartRulersViewView;
use crate::statistics::widgets::chart_header_widget::Header;
use crate::statistics::widgets::chart_lines_filter_widget::ChartLinesFilterWidgetWidgets;
use crate::statistics::widgets::point_details_widget::PointDetailsWidgetWidgets;
use serde::{Deserialize, Serialize};

pub const K_HEIGHT_LIMITS_UPDATE_TIMEOUT_MS: i64 = 320;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BottomCaptionLineData {
    pub step: i32,
    pub step_max: i32,
    pub step_min: i32,
    pub step_min_fast: i32,
    pub step_raw: i32,
    pub alpha: f64,
    pub fixed_alpha: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartAnimationController {
    pub current_x_limits: Limits,
    pub target_x_limits: Limits,
    pub current_height_limits: Limits,
    pub target_height_limits: Limits,
    pub is_animating: bool,
    pub progress: f64,
}

impl Default for ChartAnimationController {
    fn default() -> Self {
        Self {
            current_x_limits: Limits::new(0.0, 1.0),
            target_x_limits: Limits::new(0.0, 1.0),
            current_height_limits: Limits::new(0.0, 100.0),
            target_height_limits: Limits::new(0.0, 100.0),
            is_animating: false,
            progress: 1.0,
        }
    }
}

impl ChartAnimationController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_x_percentage_limits(&mut self, limits: Limits) {
        self.target_x_limits = limits;
        self.is_animating = true;
        self.progress = 0.0;
    }

    pub fn set_height_limits(&mut self, limits: Limits) {
        self.target_height_limits = limits;
        self.is_animating = true;
        self.progress = 0.0;
    }

    pub fn tick(&mut self, dt_speed: f64) {
        if !self.is_animating {
            return;
        }
        self.progress = (self.progress + dt_speed).min(1.0);
        self.current_x_limits = Limits {
            min: self.current_x_limits.min + (self.target_x_limits.min - self.current_x_limits.min) * self.progress,
            max: self.current_x_limits.max + (self.target_x_limits.max - self.current_x_limits.max) * self.progress,
        };
        self.current_height_limits = Limits {
            min: self.current_height_limits.min + (self.target_height_limits.min - self.current_height_limits.min) * self.progress,
            max: self.current_height_limits.max + (self.target_height_limits.max - self.current_height_limits.max) * self.progress,
        };
        if self.progress >= 1.0 {
            self.is_animating = false;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DragArea {
    #[default]
    None,
    Left,
    Right,
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FooterState {
    pub range_limits: Limits,
    pub active_drag: DragArea,
    pub height: f64,
}

impl Default for FooterState {
    fn default() -> Self {
        Self {
            range_limits: Limits::new(0.0, 1.0),
            active_drag: DragArea::None,
            height: 42.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChartWidget {
    pub chart_data: Option<StatisticalChart>,
    pub view_type: ChartViewType,
    pub header: Header,
    pub rulers: ChartRulersViewView,
    pub lines_filter: LinesFilterController,
    pub filter_widget: ChartLinesFilterWidgetWidgets,
    pub details_widget: PointDetailsWidgetWidgets,
    pub anim_controller: ChartAnimationController,
    pub footer: FooterState,
    pub width: f64,
    pub height: f64,
}

impl ChartWidget {
    pub fn new() -> Self {
        Self {
            width: 400.0,
            height: 320.0,
            ..Default::default()
        }
    }

    pub fn set_chart_data(&mut self, data: StatisticalChart, view_type: ChartViewType) {
        self.view_type = view_type;
        self.rulers.set_chart_data(&data, view_type);
        self.rulers.add(Limits::new(data.min_value as f64, data.max_value as f64), false);

        let buttons_data: Vec<_> = data
            .lines
            .iter()
            .map(|l| crate::statistics::widgets::chart_lines_filter_widget::ButtonData {
                text: l.name.clone(),
                color_hex: l.color_hex.clone(),
                id: l.id,
                disabled: l.is_hidden_on_start,
            })
            .collect();
        self.filter_widget.fill_buttons(&buttons_data);

        for line in &data.lines {
            self.lines_filter.set_enabled(line.id, !line.is_hidden_on_start, 0);
        }

        self.anim_controller.set_height_limits(Limits::new(
            data.min_value as f64,
            data.max_value as f64,
        ));

        self.chart_data = Some(data);
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.header.set_title(title);
    }

    pub fn set_x_percentage_limits(&mut self, limits: Limits) {
        self.footer.range_limits = limits;
        self.anim_controller.set_x_percentage_limits(limits);
    }

    pub fn on_mouse_move(&mut self, x: f64, _y: f64) {
        if let Some(ref data) = self.chart_data {
            if data.x_percentage.is_empty() {
                return;
            }
            let span = (self.footer.range_limits.max - self.footer.range_limits.min).max(f64::EPSILON);
            let rel = (x / self.width).clamp(0.0, 1.0);
            let target_perc = self.footer.range_limits.min + rel * span;

            let closest_idx = data
                .x_percentage
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = (*a - target_perc).abs();
                    let db = (*b - target_perc).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
                .unwrap_or(0);

            self.details_widget.set_x_index(data, closest_idx);
        }
    }
}
