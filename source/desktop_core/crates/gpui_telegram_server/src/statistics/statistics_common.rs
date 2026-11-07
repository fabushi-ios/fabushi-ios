//! Telegram Desktop 1:1 Submodule: `statistics_common`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use serde::{Deserialize, Serialize};

/// Upstream 1:1 constant: `constexpr auto kRulerLineAlpha = 0.06;`
pub const K_RULER_LINE_ALPHA: f64 = 0.06;

/// Upstream 1:1 struct: `struct Limits { float64 min = 0; float64 max = 0; };`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Limits {
    pub min: f64,
    pub max: f64,
}

impl Limits {
    pub fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    pub fn span(&self) -> f64 {
        (self.max - self.min).abs().max(f64::EPSILON)
    }

    pub fn contains(&self, val: f64) -> bool {
        val >= self.min && val <= self.max
    }

    pub fn clamp(&self, val: f64) -> f64 {
        val.clamp(self.min, self.max)
    }

    pub fn interpolate(&self, progress: f64) -> f64 {
        self.min + (self.max - self.min) * progress
    }
}

/// Upstream 1:1 enum: `enum class ChartViewType { Linear, Bar, StackBar, DoubleLinear, StackLinear };`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChartViewType {
    #[default]
    Linear,
    Bar,
    StackBar,
    DoubleLinear,
    StackLinear,
}

/// Common context container for chart metrics and limits configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatisticsCommon {
    pub view_type: ChartViewType,
    pub x_percentage_limits: Limits,
    pub height_limits: Limits,
    pub ruler_alpha: f64,
}

impl Default for StatisticsCommon {
    fn default() -> Self {
        Self {
            view_type: ChartViewType::Linear,
            x_percentage_limits: Limits::new(0.0, 1.0),
            height_limits: Limits::new(0.0, 100.0),
            ruler_alpha: K_RULER_LINE_ALPHA,
        }
    }
}

impl StatisticsCommon {
    pub fn new(view_type: ChartViewType) -> Self {
        Self {
            view_type,
            ..Default::default()
        }
    }

    pub fn is_stacked(&self) -> bool {
        matches!(self.view_type, ChartViewType::StackBar | ChartViewType::StackLinear)
    }

    pub fn is_double(&self) -> bool {
        matches!(self.view_type, ChartViewType::DoubleLinear)
    }
}
