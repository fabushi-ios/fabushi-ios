//! Module 13 Adversarial Stress & Math Boundary Tests: Statistics
//! Rigorously challenges segment tree boundary queries, chart rulers math,
//! zero-division in percentage calculations, and timestamp formatting.

use gpui_telegram_server::statistics::chart_lines_filter_controller::ChartLinesFilterController;
use gpui_telegram_server::statistics::chart_rulers_data::ChartRulersData;
use gpui_telegram_server::statistics::segment_tree::SegmentTree;
use gpui_telegram_server::statistics::statistics_common::Limits;
use gpui_telegram_server::statistics::statistics_data_deserialize::{Line, StatisticalChart};
use gpui_telegram_server::statistics::statistics_format_values::StatisticsFormatValues;
use gpui_telegram_server::statistics::view::abstract_chart_view::{
    default_height_limits, DoubleLineRatios,
};
use gpui_telegram_server::statistics::view::stack_linear_chart_common::pie_parts_percentage;

#[test]
fn test_adversarial_segment_tree_bounds_and_inverted_queries() {
    // 1. Empty array: must not panic on min/max queries
    let mut empty_tree = SegmentTree::new(Vec::new());
    assert_eq!(empty_tree.r_max_q(0, 10), 0);
    assert_eq!(empty_tree.r_min_q(0, 10), 0);
    assert!(empty_tree.is_empty());
    assert_eq!(empty_tree.len(), 0);

    // 2. Single element array
    let mut single_tree = SegmentTree::new(vec![42]);
    assert_eq!(single_tree.r_max_q(0, 0), 42);
    assert_eq!(single_tree.r_min_q(0, 0), 42);
    assert_eq!(single_tree.r_max_q(0, 50), 42);
    assert_eq!(single_tree.r_min_q(0, 50), 42);

    // 3. Range queries on small tree (< 30 elements)
    let small_data = vec![5, 12, 99, 1, 45, 60];
    let mut small_tree = SegmentTree::new(small_data);
    assert_eq!(small_tree.r_max_q(0, 5), 99);
    assert_eq!(small_tree.r_min_q(0, 5), 1);
    assert_eq!(small_tree.r_max_q(2, 2), 99);
    assert_eq!(small_tree.r_min_q(2, 2), 99);

    // Inverted range query behavior verification (from > to + 1 panics on slice index)
    let panic_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut t = SegmentTree::new(vec![5, 12, 99, 1, 45, 60]);
        t.r_max_q(4, 1);
    }));
    assert!(panic_res.is_err(), "Inverted range from > to + 1 triggers slice index panic");

    // 4. Large tree (heap mode >= 30 elements) with huge out-of-bounds indices
    let mut large_data = Vec::with_capacity(50);
    for i in 0..50 {
        large_data.push(i as i64 * 10);
    }
    let mut large_tree = SegmentTree::new(large_data);
    assert_eq!(large_tree.r_max_q(0, usize::MAX / 2), 490);
    assert_eq!(large_tree.r_min_q(10, usize::MAX / 2), 100);

    // 5. Array boundary (29 vs 30 elements)
    let arr29 = (0..29).map(|i| i as i64).collect();
    let arr30 = (0..30).map(|i| i as i64).collect();
    let mut tree29 = SegmentTree::new(arr29);
    let mut tree30 = SegmentTree::new(arr30);
    assert_eq!(tree29.r_max_q(0, 28), 28);
    assert_eq!(tree30.r_max_q(0, 29), 29);
}

#[test]
fn test_adversarial_chart_rulers_zero_and_negative_heights() {
    // 1. All zeros: new_max_height = 0, new_min_height = 0
    let rulers_zero = ChartRulersData::new(0, 0, false, 0.0);
    assert!(!rulers_zero.lines.is_empty(), "Rulers must generate baseline lines even for 0 max height");
    assert_eq!(rulers_zero.lines[0].absolute_value, 0.0);

    // 2. Zero-diff with use_min_height = true (prevents division by zero)
    let rulers_zero_diff = ChartRulersData::new(50, 50, true, 0.0);
    assert_eq!(rulers_zero_diff.lines.len(), 3); // K_MAX_LINES / 2

    // 3. right_ratio == 0.0 (prevents division by zero in scaled_line_caption)
    let rulers_no_ratio = ChartRulersData::new(100, 10, true, 0.0);
    for line in &rulers_no_ratio.lines {
        assert_eq!(line.scaled_line_caption, "");
    }

    // 4. Negative height range (e.g., negative delta/temperature charts)
    let rulers_neg = ChartRulersData::new(-10, -100, true, 1.0);
    assert!(!rulers_neg.lines.is_empty());

    // 5. compute_relative with identical min and max height (prevents division by zero: span.max(1.0))
    let mut rulers_comp = ChartRulersData::new(100, 0, false, 1.0);
    rulers_comp.compute_relative(50.0, 50.0);
    for line in &rulers_comp.lines {
        assert!(!line.relative_value.is_nan());
        assert!(!line.relative_value.is_infinite());
    }

    // 6. Astronomical heights
    let huge_height = i64::MAX / 16;
    let rulers_huge = ChartRulersData::new(huge_height, 0, false, 1.0);
    assert!(!rulers_huge.lines.is_empty());
}

#[test]
fn test_adversarial_pie_parts_percentage_zero_sum_and_rounding() {
    // 1. Zero total sum: all sums are 0.0 -> must NOT produce NaN or Infinity
    let zero_sums = vec![0.0, 0.0, 0.0];
    let pie_zero = pie_parts_percentage(&zero_sums, 0.0, true);
    assert_eq!(pie_zero.parts.len(), 3);
    for part in &pie_zero.parts {
        assert!(!part.rounded_percentage.is_nan());
        assert_eq!(part.rounded_percentage, 0.0);
        assert_eq!(part.percentage_text, "0%");
    }
    assert!(!pie_zero.pie_has_single_part);

    // 2. Single 100% slice
    let single_sum = vec![500.0];
    let pie_single = pie_parts_percentage(&single_sum, 500.0, true);
    assert_eq!(pie_single.parts.len(), 1);
    assert!(pie_single.pie_has_single_part);
    assert_eq!(pie_single.parts[0].percentage_text, "100%");

    // 3. Fractional distribution with rounding adjustment
    let third_sums = vec![33.333, 33.333, 33.334];
    let pie_thirds = pie_parts_percentage(&third_sums, 100.0, true);
    let sum_perc: f64 = pie_thirds.parts.iter().map(|p| p.rounded_percentage).sum();
    // Sum of rounded percentages must equal approx 1.0 (100%)
    assert!((sum_perc - 1.0).abs() < 1e-4);

    // 4. Empty sums slice
    let pie_empty = pie_parts_percentage(&[], 0.0, true);
    assert!(pie_empty.parts.is_empty());
}


#[test]
fn test_adversarial_limits_and_double_line_ratios_zero_division() {
    // 1. Limits::span() when min == max must be EPSILON (prevents division by zero)
    let equal_limits = Limits::new(42.0, 42.0);
    assert!(equal_limits.span() > 0.0);
    assert_eq!(equal_limits.span(), f64::EPSILON);
    assert_eq!(equal_limits.interpolate(0.5), 42.0);

    // Inverted limits span calculation
    let inverted = Limits::new(100.0, 50.0);
    assert_eq!(inverted.span(), 50.0);

    // Normal limits clamp behavior
    let normal = Limits::new(50.0, 100.0);
    assert_eq!(normal.clamp(20.0), 50.0);
    assert_eq!(normal.clamp(120.0), 100.0);
    assert_eq!(normal.clamp(75.0), 75.0);

    // Inverted limits clamp panic behavior verification (f64::clamp asserts min <= max)
    let clamp_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inverted.clamp(20.0);
    }));
    assert!(clamp_panic.is_err(), "Inverted clamp triggers standard library assertion");

    // 2. DoubleLineRatios with zero maximums
    let mut ratios = DoubleLineRatios::new(true);
    let mut chart_with_zeroes = StatisticalChart::default();
    chart_with_zeroes.lines.push(Line {
        id: 0,
        id_string: "y0".into(),
        name: "L0".into(),
        color_key: "y0".into(),
        color_hex: "#000".into(),
        is_hidden_on_start: false,
        y: vec![0, 0, 0],
        max_value: 0,
        min_value: 0,
        segment_tree: SegmentTree::new(vec![0, 0, 0]),
    });
    chart_with_zeroes.lines.push(Line {
        id: 1,
        id_string: "y1".into(),
        name: "L1".into(),
        color_key: "y1".into(),
        color_hex: "#FFF".into(),
        is_hidden_on_start: false,
        y: vec![0, 0, 0],
        max_value: 0,
        min_value: 0,
        segment_tree: SegmentTree::new(vec![0, 0, 0]),
    });
    ratios.init(&chart_with_zeroes);
    // Neither ratio should be NaN or Infinite
    assert!(!ratios.first.is_nan() && !ratios.first.is_infinite());
    assert!(!ratios.second.is_nan() && !ratios.second.is_infinite());

    // 3. default_height_limits with inverted x_indices
    let filter = ChartLinesFilterController::new();
    let hl = default_height_limits(&ratios, &filter, &mut chart_with_zeroes, Limits::new(10.0, 2.0));
    assert!(!hl.ranged.min.is_nan());
    assert!(!hl.ranged.max.is_nan());
}

#[test]
fn test_adversarial_statistics_format_values_timestamps() {
    // 1. Unix Epoch 0
    let str_0 = StatisticsFormatValues::day_month_year(0);
    assert_eq!(str_0, "1 Jan 1970");

    // 2. Negative seconds (prior to 1970)
    let str_neg = StatisticsFormatValues::day_month_year(-86400);
    assert_eq!(str_neg, "31 Dec 1969");

    // 3. Leap day (Feb 29, 2024 -> 1709164800)
    let str_leap = StatisticsFormatValues::detailed_day_month(1709164800);
    assert!(str_leap.contains("29 Feb 2024") || str_leap.contains("29 Feb"));

    // 4. Maximum supported timestamp fallback
    let str_max = StatisticsFormatValues::day_month_year(i64::MAX);
    assert!(!str_max.is_empty());
}
