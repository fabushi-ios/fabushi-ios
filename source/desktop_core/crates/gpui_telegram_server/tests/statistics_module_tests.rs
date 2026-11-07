use gpui_telegram_server::statistics::chart_lines_filter_controller::ChartLinesFilterController;
use gpui_telegram_server::statistics::chart_rulers_data::{self, ChartRulersData};
use gpui_telegram_server::statistics::chart_widget::ChartWidget;
use gpui_telegram_server::statistics::segment_tree::SegmentTree;
use gpui_telegram_server::statistics::statistics_common::ChartViewType;
use gpui_telegram_server::statistics::statistics_data_deserialize::{
    statistical_chart_from_json, StatisticsDataDeserialize,
};
use gpui_telegram_server::statistics::statistics_format_values::StatisticsFormatValues;
use gpui_telegram_server::statistics::statistics_xlsx::{Cell, Sheet, StatisticsXlsx};
use gpui_telegram_server::statistics::view::stack_linear_chart_common::StackLinearChartCommonView;

#[test]
fn test_segment_tree_small_and_large() {
    // 1. Small array (< 30) -> linear scan path
    let small_data = vec![10, 42, 5, 88, 12];
    let mut small_tree = SegmentTree::new(small_data);
    assert_eq!(small_tree.r_max_q(0, 4), 88);
    assert_eq!(small_tree.r_min_q(0, 4), 5);
    assert_eq!(small_tree.r_max_q(1, 2), 42);

    // 2. Large array (>= 30) -> binary segment tree heap path
    let mut large_data = Vec::new();
    for i in 0..64 {
        large_data.push(i * 3);
    }
    let mut large_tree = SegmentTree::new(large_data);
    assert_eq!(large_tree.r_max_q(0, 63), 63 * 3);
    assert_eq!(large_tree.r_min_q(10, 20), 10 * 3);
    assert_eq!(large_tree.r_max_q(10, 20), 20 * 3);
}

#[test]
fn test_chart_lines_filter_controller_animation() {
    let mut controller = ChartLinesFilterController::new();
    controller.set_enabled(1, false, 1000);
    assert!(!controller.is_finished());

    // Advance animation by 0.5
    controller.tick(0.5);
    let alpha_mid = controller.alpha(1);
    assert!(alpha_mid < 1.0 && alpha_mid >= 0.0);

    // Complete animation
    controller.tick(0.6);
    assert!(controller.alpha(1) <= 0.01);
    assert!(!controller.is_enabled(1));

    // Re-enable line
    controller.set_enabled(1, true, 2000);
    controller.tick(1.0);
    assert_eq!(controller.alpha(1), 1.0);
    assert!(controller.is_enabled(1));
    assert!(controller.is_finished());
}

#[test]
fn test_chart_rulers_data_math() {
    // LookupHeight tests
    assert_eq!(chart_rulers_data::round_chart_value(105), 110);
    assert_eq!(ChartRulersData::lookup_height(55), 55);
    assert_eq!(ChartRulersData::lookup_height(105), 110);

    // Rulers generation
    let mut rulers = ChartRulersData::new(100, 0, false, 1.0);
    assert!(!rulers.lines.is_empty());
    rulers.compute_relative(100.0, 0.0);
    let top_line = rulers.lines.last().unwrap();
    assert!(top_line.relative_value <= 1.0);
}

#[test]
fn test_statistics_data_deserialize() {
    let json_data = r##"{
        "columns": [
            ["x", 1609459200000.0, 1609545600000.0, 1609632000000.0],
            ["y0", 100.0, 200.0, 300.0],
            ["y1", 50.0, 80.0, 120.0]
        ],
        "colors": {
            "y0": "#FF0000",
            "y1": "#00FF00"
        },
        "names": {
            "y0": "Views",
            "y1": "Shares"
        },
        "hidden": ["y1"]
    }"##;

    let chart = StatisticsDataDeserialize::parse(json_data).expect("Must parse chart json");
    assert_eq!(chart.x.len(), 3);
    assert_eq!(chart.lines.len(), 2);
    assert_eq!(chart.lines[0].name, "Views");
    assert_eq!(chart.lines[0].color_hex, "#FF0000");
    assert!(!chart.lines[0].is_hidden_on_start);

    assert_eq!(chart.lines[1].name, "Shares");
    assert!(chart.lines[1].is_hidden_on_start);
    assert_eq!(chart.lines[1].y, vec![50, 80, 120]);
    assert_eq!(chart.max_value, 300);
}

#[test]
fn test_format_values() {
    let timestamp = 1609459200; // 2021-01-01 00:00:00 UTC
    let dmy = StatisticsFormatValues::day_month_year(timestamp);
    assert!(dmy.contains("Jan"));
    assert!(dmy.contains("2021"));

    let dm = StatisticsFormatValues::day_month(timestamp);
    assert!(dm.contains("Jan"));

    let detailed = StatisticsFormatValues::detailed_day_month(timestamp);
    assert!(detailed.contains("Fri"));
}

#[test]
fn test_pie_parts_percentage_invariant() {
    let sums = vec![33.3, 33.3, 33.4];
    let pie = StackLinearChartCommonView::parts_percentage(&sums, 100.0);
    assert_eq!(pie.parts.len(), 3);

    let total_rounded: f64 = pie.parts.iter().map(|p| p.rounded_percentage).sum();
    assert!((total_rounded - 1.0).abs() < 1e-6, "Total rounded must equal 100%");
}

#[test]
fn test_xlsx_serialization() {
    let sheet = Sheet {
        name: "TestSheet".to_string(),
        rows: vec![
            vec![Cell::header("Metric"), Cell::header("Count")],
            vec![Cell::text("Views"), Cell::number(4200.0)],
        ],
    };
    let serialized = StatisticsXlsx::serialize(&[sheet]);
    let xml_str = String::from_utf8(serialized).unwrap();
    assert!(xml_str.contains("<worksheet"));
    assert!(xml_str.contains("TestSheet"));
    assert!(xml_str.contains("Metric"));
    assert!(xml_str.contains("4200"));
}

#[test]
fn test_chart_widget_workflow() {
    let mut widget = ChartWidget::new();
    widget.set_title("Followers Growth");
    assert_eq!(widget.header.title(), "Followers Growth");

    let json_data = r##"{
        "columns": [
            ["x", 1000.0, 2000.0, 3000.0],
            ["y0", 10.0, 20.0, 30.0]
        ],
        "names": { "y0": "Followers" }
    }"##;
    let chart = statistical_chart_from_json(json_data).unwrap();
    widget.set_chart_data(chart, ChartViewType::Linear);

    assert_eq!(widget.rulers.rulers.len(), 1);
    assert_eq!(widget.filter_widget.buttons.len(), 1);

    // Mouse hover updates point details
    widget.on_mouse_move(200.0, 100.0);
    assert_eq!(widget.details_widget.lines.len(), 1);
    assert_eq!(widget.details_widget.lines[0].name, "Followers");
}
