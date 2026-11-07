//! Telegram Desktop 1:1 Submodule: `statistics_data_deserialize`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::segment_tree::SegmentTree;
use crate::statistics::statistics_types::ChartValue;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatisticalCurrency {
    Ton,
    Credits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub id: i32,
    pub id_string: String,
    pub name: String,
    pub color_key: String,
    pub color_hex: String,
    pub is_hidden_on_start: bool,
    pub y: Vec<ChartValue>,
    pub min_value: ChartValue,
    pub max_value: ChartValue,
    #[serde(skip)]
    pub segment_tree: SegmentTree,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            id: 0,
            id_string: String::new(),
            name: String::new(),
            color_key: String::new(),
            color_hex: "#2AABEE".to_string(),
            is_hidden_on_start: false,
            y: Vec::new(),
            min_value: 0,
            max_value: 0,
            segment_tree: SegmentTree::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StatisticalChart {
    pub x: Vec<f64>,
    pub x_percentage: Vec<f64>,
    pub lines: Vec<Line>,
    pub time_step: f64,
    pub min_value: ChartValue,
    pub max_value: ChartValue,
    pub currency: Option<StatisticalCurrency>,
    pub currency_rate: f64,
    pub is_footer_hidden: bool,
    pub has_percentages: bool,
    pub week_format: bool,
    pub default_zoom_x_index: (usize, usize),
}

impl StatisticalChart {
    pub fn measure(&mut self) {
        if self.x.is_empty() {
            return;
        }
        let first = self.x[0];
        let span = (self.x.last().copied().unwrap_or(first) - first).max(1.0);
        self.x_percentage = self.x.iter().map(|&t| (t - first) / span).collect();

        let mut overall_min = ChartValue::MAX;
        let mut overall_max = ChartValue::MIN;

        for line in &mut self.lines {
            line.segment_tree = SegmentTree::new(line.y.clone());
            if let Some(&min) = line.y.iter().min() {
                line.min_value = min;
                overall_min = overall_min.min(min);
            }
            if let Some(&max) = line.y.iter().max() {
                line.max_value = max;
                overall_max = overall_max.max(max);
            }
        }

        if overall_min == ChartValue::MAX {
            overall_min = 0;
        }
        if overall_max == ChartValue::MIN {
            overall_max = 1;
        }
        if overall_min == overall_max {
            if overall_min != 0 {
                overall_min = 0;
            } else {
                overall_max = 1;
            }
        }
        self.min_value = overall_min;
        self.max_value = overall_max;
    }
}

pub fn statistical_chart_from_json(json_str: &str) -> Option<StatisticalChart> {
    let root: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let columns = root.get("columns")?.as_array()?;
    if columns.is_empty() {
        return None;
    }

    let mut chart = StatisticalChart::default();

    if let Some(fmt) = root.get("yTickFormatter").and_then(|v| v.as_str()) {
        if fmt.contains("TON") {
            chart.currency = Some(StatisticalCurrency::Ton);
        } else if fmt.contains("XTR") || fmt.contains("Credits") {
            chart.currency = Some(StatisticalCurrency::Credits);
        }
    }

    let hidden_ids: Vec<String> = root
        .get("hidden")
        .and_then(|h| h.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    let mut line_counter = 0;
    for col in columns {
        let items = col.as_array()?;
        if items.is_empty() {
            continue;
        }
        let col_id = items[0].as_str().unwrap_or_default();
        if col_id == "x" {
            chart.x = items[1..].iter().filter_map(|v| v.as_f64()).collect();
        } else {
            line_counter += 1;
            let values: Vec<ChartValue> = items[1..]
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f.round() as ChartValue))
                .collect();
            chart.lines.push(Line {
                id: line_counter,
                id_string: col_id.to_string(),
                name: col_id.replace('-', "—"),
                is_hidden_on_start: hidden_ids.iter().any(|h| h == col_id),
                y: values,
                ..Default::default()
            });
        }
    }

    if chart.x.len() > 1 {
        chart.time_step = (chart.x[1] - chart.x[0]).max(1.0);
    } else {
        chart.time_step = 86_400_000.0;
    }

    if let Some(subchart) = root.get("subchart").and_then(|s| s.as_object()) {
        if let Some(show) = subchart.get("show").and_then(|s| s.as_bool()) {
            chart.is_footer_hidden = !show;
        }
    }

    if let Some(colors) = root.get("colors").and_then(|c| c.as_object()) {
        for line in &mut chart.lines {
            if let Some(hex) = colors.get(&line.id_string).and_then(|v| v.as_str()) {
                line.color_hex = hex.to_string();
            }
        }
    }

    if let Some(names) = root.get("names").and_then(|n| n.as_object()) {
        for line in &mut chart.lines {
            if let Some(name) = names.get(&line.id_string).and_then(|v| v.as_str()) {
                line.name = name.replace('-', "—");
            }
        }
    }

    chart.measure();
    Some(chart)
}

/// Target symbol wrapper for compatibility
pub struct StatisticsDataDeserialize;

impl StatisticsDataDeserialize {
    pub fn parse(json: &str) -> Option<StatisticalChart> {
        statistical_chart_from_json(json)
    }
}
