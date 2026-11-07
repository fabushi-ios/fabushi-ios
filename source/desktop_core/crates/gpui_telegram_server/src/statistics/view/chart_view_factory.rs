//! Telegram Desktop 1:1 Submodule: `chart_view_factory`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_common::ChartViewType;
use crate::statistics::view::bar_chart_view::BarChartViewView;
use crate::statistics::view::linear_chart_view::LinearChartViewView;
use crate::statistics::view::stack_linear_chart_view::StackLinearChartViewView;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConcreteChartView {
    Linear(LinearChartViewView),
    Bar(BarChartViewView),
    StackBar(BarChartViewView),
    DoubleLinear(LinearChartViewView),
    StackLinear(StackLinearChartViewView),
}

/// Upstream 1:1: `CreateChartView(ChartViewType type)`
pub fn create_chart_view(view_type: ChartViewType) -> ConcreteChartView {
    match view_type {
        ChartViewType::Linear => ConcreteChartView::Linear(LinearChartViewView::new(false)),
        ChartViewType::Bar => ConcreteChartView::Bar(BarChartViewView::new(false)),
        ChartViewType::StackBar => ConcreteChartView::StackBar(BarChartViewView::new(true)),
        ChartViewType::DoubleLinear => ConcreteChartView::DoubleLinear(LinearChartViewView::new(true)),
        ChartViewType::StackLinear => ConcreteChartView::StackLinear(StackLinearChartViewView::new()),
    }
}

/// Target symbol wrapper for compatibility
pub struct ChartViewFactoryView;

impl ChartViewFactoryView {
    pub fn create(view_type: ChartViewType) -> ConcreteChartView {
        create_chart_view(view_type)
    }
}
