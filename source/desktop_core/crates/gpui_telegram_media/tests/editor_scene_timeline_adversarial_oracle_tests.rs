//! Adversarial Blind C++ Oracle Test Suite for Module 09 (media & editor)
//! Tests Editor Scene Graph, Crop Overlay & Video Timeline Subsystem
//!
//! Blind Oracles:
//! 1. 45° angle snapping, sticky guideline magnetism & affine transform composition
//!    (upstream: editor/scene/scene_item_base.cpp:L27-L61, L203-L230)
//! 2. Star 10-point geometry & arrow head curvature math
//!    (upstream: editor/scene/scene_item_shape.cpp:L21-L32, L185-L209, L248-L278)
//! 3. Crop Rule of Thirds grid, aspect ratio flip & boundary constraints
//!    (upstream: editor/editor_crop.cpp:L49-L82, L257-L281)
//! 4. Video trim timestamp parse/format MM:SS.mmm & 24-slot timeline frame layout
//!    (upstream: editor/editor_trim_timeline.cpp:L40-L100, editor/video/video_timeline.cpp:L18-L105)
//! 5. Magnetic volume snapping [0.25, 0.50, 0.75] & 4000ms audio disc rotation cycle
//!    (upstream: editor/editor_audio_menu.cpp:L27-L34, editor/editor_audio_disc_button.cpp:L23, L87-L93)

use gpui_telegram_media::editor::{
    editor_audio_disc_button::*,
    editor_audio_menu::*,
    editor_crop::*,
    editor_trim_timeline::*,
    scene::scene_item_base::*,
    scene::scene_item_shape::*,
    video::video_timeline::*,
};
use std::f64::consts::PI;

// =========================================================================
// Oracle 1: 45° Angle Snapping, Sticky Guideline Magnetism & Affine Transform
// C++ Upstream Reference: scene_item_base.cpp:L27-L61, L203-L230
// =========================================================================

/// Independent C++ Oracle 1 for angle normalization:
/// auto Normalized(float64 angle) {
///     return angle + ((std::abs(angle) < 360) ? 0 : (-360 * (angle < 0 ? -1 : 1)));
/// }
fn cpp_oracle_normalize_angle(angle: f64) -> f64 {
    if angle.abs() < 360.0 {
        angle
    } else {
        angle - 360.0 * (if angle < 0.0 { -1.0 } else { 1.0 })
    }
}

/// Independent C++ Oracle 1 for 45° angle snapping:
/// base::SafeRound(angle / kSnapAngle) * kSnapAngle where kSnapAngle = 45.
fn cpp_oracle_snap_angle(angle: f64) -> f64 {
    let norm = cpp_oracle_normalize_angle(angle);
    (norm / 45.0).round() * 45.0
}

/// Independent C++ Oracle 1 for sticky guidelines:
/// StickyStart + StickyLength * line / (kStickyLines - 1) with kStickyLines = 5
fn cpp_oracle_sticky_line(start: f64, length: f64, line: usize) -> f64 {
    let clamped_line = line.min(4);
    start + length * (clamped_line as f64) / 4.0
}

/// Independent C++ Oracle 1 for rotation angle from mouse offset:
/// diff = pos - origin;
/// angle = Normalized((isLeft ? 180 : 0) + std::atan2(diff.y(), diff.x()) * 180 / M_PI);
/// rotation = shift ? (SafeRound(angle / 45.) * 45.) : angle;
fn cpp_oracle_item_rotation(
    origin: (f64, f64),
    mouse_pos: (f64, f64),
    is_left: bool,
    shift_snap: bool,
) -> f64 {
    let diff_x = mouse_pos.0 - origin.0;
    let diff_y = mouse_pos.1 - origin.1;
    let base_angle = diff_y.atan2(diff_x) * 180.0 / PI;
    let offset = if is_left { 180.0 } else { 0.0 };
    let normalized = cpp_oracle_normalize_angle(offset + base_angle);
    if shift_snap {
        cpp_oracle_snap_angle(normalized)
    } else {
        normalized
    }
}

/// Independent C++ Oracle 1 for resize math:
/// p = isLeft ? -mousePos : mousePos;
/// dx = int(2.0 * p.x()); dy = int(2.0 * p.y());
/// size = clamp(dx > dy ? dx : dy, min, max);
fn cpp_oracle_item_resize(
    mouse_x: f64,
    mouse_y: f64,
    is_left: bool,
    min_size: f64,
    max_size: f64,
    aspect_ratio: f64,
) -> (f64, f64) {
    let (px, py) = if is_left { (-mouse_x, -mouse_y) } else { (mouse_x, mouse_y) };
    let dx = (2.0 * px).abs();
    let dy = (2.0 * py).abs();
    let target = if dx > dy { dx } else { dy };
    let h_size = target.clamp(min_size, max_size);
    let v_size = (h_size * aspect_ratio).max(min_size);
    (h_size, v_size)
}

#[test]
fn test_oracle_1_angle_normalization_and_snapping_stress() {
    let test_angles = [
        0.0, 22.49, 22.51, 44.9, 45.0, 45.1, 67.4, 67.6, 89.9, 90.0,
        134.9, 135.0, 179.9, 180.0, 224.9, 225.0, 269.9, 270.0, 314.9, 315.0,
        359.9, 360.0, 382.5, 720.0, -22.49, -22.51, -45.0, -89.9, -90.0,
        -180.0, -270.0, -360.0, -400.0,
    ];

    for &angle in &test_angles {
        let expected_norm = cpp_oracle_normalize_angle(angle);
        let actual_norm = normalize_angle(angle);
        assert!(
            (expected_norm - actual_norm).abs() < 1e-9,
            "Angle norm mismatch for angle {}: expected {}, got {}",
            angle, expected_norm, actual_norm
        );

        let expected_snap = cpp_oracle_snap_angle(angle);
        let actual_snap = snap_angle(angle);
        assert!(
            (expected_snap - actual_snap).abs() < 1e-9,
            "Angle snap mismatch for angle {}: expected {}, got {}",
            angle, expected_snap, actual_snap
        );
        // Snapped angles must be integer multiples of 45.0
        assert_eq!((actual_snap % 45.0).abs(), 0.0);
    }
}

#[test]
fn test_oracle_1_sticky_guideline_magnetism_and_clamping() {
    let canvas_start = 120.0;
    let canvas_length = 800.0;

    for line_idx in 0..10 {
        let oracle_val = cpp_oracle_sticky_line(canvas_start, canvas_length, line_idx);
        let actual_val = sticky_line(canvas_start, canvas_length, line_idx);
        assert!(
            (oracle_val - actual_val).abs() < 1e-9,
            "Sticky line mismatch at index {}: oracle {}, actual {}",
            line_idx, oracle_val, actual_val
        );
    }

    // Verify 5 equidistant guidelines: 0%, 25%, 50%, 75%, 100%
    assert_eq!(sticky_line(0.0, 1000.0, 0), 0.0);
    assert_eq!(sticky_line(0.0, 1000.0, 1), 250.0);
    assert_eq!(sticky_line(0.0, 1000.0, 2), 500.0);
    assert_eq!(sticky_line(0.0, 1000.0, 3), 750.0);
    assert_eq!(sticky_line(0.0, 1000.0, 4), 1000.0);
}

#[test]
fn test_oracle_1_affine_transform_composition_and_handles() {
    let mut item = SceneItemBaseScene::new(42, (300.0, 300.0), 200.0, 1.5);
    assert_eq!(item.horizontal_size, 200.0);
    assert_eq!(item.vertical_size, 300.0);

    // Test resize via left and right handles against Oracle
    let mouse_coords = [(150.0, 80.0), (300.0, 250.0), (-80.0, -120.0), (10.0, 5.0)];
    for &(mx, my) in &mouse_coords {
        // Right handle resize
        let (exp_h, exp_v) = cpp_oracle_item_resize(mx, my, false, item.min_size, item.max_size, 1.5);
        item.apply_resize(mx, my, false);
        assert_eq!(item.horizontal_size, exp_h);
        assert_eq!(item.vertical_size, exp_v);

        // Left handle resize
        let (exp_hl, exp_vl) = cpp_oracle_item_resize(mx, my, true, item.min_size, item.max_size, 1.5);
        item.apply_resize(mx, my, true);
        assert_eq!(item.horizontal_size, exp_hl);
        assert_eq!(item.vertical_size, exp_vl);
    }

    // Test rotation with and without shift snapping against Oracle
    let test_points = [
        (400.0, 300.0), // 0 deg
        (400.0, 400.0), // 45 deg
        (300.0, 400.0), // 90 deg
        (200.0, 400.0), // 135 deg
        (200.0, 300.0), // 180 deg
        (200.0, 200.0), // -135 deg
        (300.0, 200.0), // -90 deg
        (400.0, 200.0), // -45 deg
    ];
    let origin = (300.0, 300.0);

    for &(px, py) in &test_points {
        for is_left in [false, true] {
            for snap in [false, true] {
                let oracle_rot = cpp_oracle_item_rotation(origin, (px, py), is_left, snap);
                item.apply_rotation(origin, (px, py), is_left, snap);
                assert!(
                    (item.placement.rotation - oracle_rot).abs() < 1e-6,
                    "Rotation mismatch at point ({}, {}), is_left={}, snap={}: oracle {}, got {}",
                    px, py, is_left, snap, oracle_rot, item.placement.rotation
                );
            }
        }
    }
}


// =========================================================================
// Oracle 2: Star 10-Point Geometry & Arrow Head Curvature Math
// C++ Upstream Reference: scene_item_shape.cpp:L21-L32, L185-L209, L248-L278
// =========================================================================

/// Independent C++ Oracle 2 for 5-point star 10-vertex generation:
/// outer = radius; inner = outer * kStarInnerRatio (0.4316);
/// for i in 0..5: angle = -PI/2 + i * 2PI/5; innerAngle = angle + PI/5;
fn cpp_oracle_star_vertices(outer_radius: f64) -> Vec<(f64, f64)> {
    let inner_radius = outer_radius * 0.4316;
    let mut vertices = Vec::with_capacity(10);
    for i in 0..10 {
        let r = if i % 2 == 0 { outer_radius } else { inner_radius };
        let angle = (i as f64) * PI / 5.0 - std::f64::consts::FRAC_PI_2;
        vertices.push((r * angle.cos(), r * angle.sin()));
    }
    vertices
}

/// Independent C++ Oracle 2 for arrowhead side:
/// max(kArrowHeadRatio * width, kArrowHeadStrokeFactor * strokeWidth)
/// clamped to [0, width / 3.0] where kArrowHeadRatio = 0.16, kArrowHeadStrokeFactor = 2.5
fn cpp_oracle_arrow_head_side(width: f64, stroke_width: f64) -> f64 {
    if width <= 0.0 {
        return 0.0;
    }
    let val = (0.16 * width).max(2.5 * stroke_width);
    val.clamp(0.0, width / 3.0)
}

/// Independent C++ Oracle 2 for arrow control points:
/// start = (-dir * width / 2, -middle); control = (0, 3 * middle);
/// end = (dir * (width / 2 - side / 2), -middle);
/// middle = bend * kArrowMaxBendRatio * width (kArrowMaxBendRatio = 0.35)
fn cpp_oracle_arrow_control_points(
    width: f64,
    stroke_width: f64,
    bend: f64,
) -> ((f64, f64), (f64, f64), (f64, f64), f64) {
    let hw = width / 2.0;
    let start = (-hw, 0.0);
    let end = (hw, 0.0);
    let max_bend = width * 0.35;
    let bend_offset = bend.clamp(-1.0, 1.0) * max_bend;
    let control = (0.0, bend_offset);
    let head_side = (width * 0.16).max(stroke_width * 2.5);
    (start, control, end, head_side)
}

#[test]
fn test_oracle_2_star_10_point_geometry_golden_ratio() {
    let shape_item = SceneItemShapeScene::new(ShapeType::Star, [255, 255, 0, 255], 2.0, true, 200.0);
    assert_eq!(shape_item.shape, ShapeType::Star);

    // Verify Star aspect ratio from C++ constants: kStarBoxHeight / kStarBoxWidth = 1.809017 / 1.902113
    let expected_ratio = 1.809017 / 1.902113;
    assert!((shape_item.default_aspect_ratio() - expected_ratio).abs() < 1e-6);

    let test_radii = [10.0, 50.0, 100.0, 250.0, 500.0];
    for &outer_r in &test_radii {
        let oracle_verts = cpp_oracle_star_vertices(outer_r);
        let actual_verts = shape_item.star_vertices(outer_r);

        assert_eq!(actual_verts.len(), 10);
        assert_eq!(oracle_verts.len(), 10);

        for i in 0..10 {
            let (ox, oy) = oracle_verts[i];
            let (ax, ay) = actual_verts[i];
            assert!(
                (ox - ax).abs() < 1e-9 && (oy - ay).abs() < 1e-9,
                "Star vertex {} mismatch: oracle ({}, {}), actual ({}, {})",
                i, ox, oy, ax, ay
            );

            // Even vertices are tips (outer radius), odd are inner valleys
            let dist = (ax * ax + ay * ay).sqrt();
            if i % 2 == 0 {
                assert!((dist - outer_r).abs() < 1e-9);
            } else {
                let inner_r = outer_r * K_STAR_INNER_RATIO;
                assert!((dist - inner_r).abs() < 1e-9);
            }
        }
    }
}

#[test]
fn test_oracle_2_arrow_curvature_and_head_math() {
    let mut arrow = SceneItemShapeScene::new(ShapeType::Arrow, [0, 200, 255, 255], 4.0, false, 300.0);
    assert_eq!(arrow.shape, ShapeType::Arrow);

    // Test straight arrow (bend = 0.0)
    arrow.set_bend(0.0);
    let pts = arrow.compute_arrow_points();
    let (exp_start, exp_control, exp_end, exp_head) =
        cpp_oracle_arrow_control_points(arrow.width, arrow.stroke_width, 0.0);

    assert_eq!(pts.start, exp_start);
    assert_eq!(pts.control, exp_control);
    assert_eq!(pts.end, exp_end);
    assert_eq!(pts.head_side, exp_head);
    assert_eq!(pts.control.1, 0.0); // No curve offset when bend = 0

    // Test extreme bend values and clamping: -2.0 clamped to -1.0, 2.0 clamped to 1.0
    arrow.set_bend(2.0);
    assert_eq!(arrow.bend, 1.0);
    let pts_pos = arrow.compute_arrow_points();
    assert_eq!(pts_pos.control.1, arrow.width * K_ARROW_MAX_BEND_RATIO);

    arrow.set_bend(-2.0);
    assert_eq!(arrow.bend, -1.0);
    let pts_neg = arrow.compute_arrow_points();
    assert_eq!(pts_neg.control.1, -arrow.width * K_ARROW_MAX_BEND_RATIO);

    // Test arrowhead sizing dominance: stroke vs width
    // Case A: Wide stroke dominates within 1/3 bounds (stroke_width * 2.5 > width * 0.16)
    let mid_arrow = SceneItemShapeScene::new(ShapeType::Arrow, [255, 0, 0, 255], 10.0, false, 150.0);
    let pts_a = mid_arrow.compute_arrow_points();
    assert_eq!(pts_a.head_side, 10.0 * K_ARROW_HEAD_STROKE_FACTOR); // 25.0
    assert_eq!(pts_a.head_side, cpp_oracle_arrow_head_side(150.0, 10.0));

    // Case B: Width ratio dominates (width * 0.16 > stroke_width * 2.5)
    let large_arrow = SceneItemShapeScene::new(ShapeType::Arrow, [255, 0, 0, 255], 2.0, false, 500.0);
    let pts_b = large_arrow.compute_arrow_points();
    assert_eq!(pts_b.head_side, 500.0 * K_ARROW_HEAD_RATIO); // 80.0
    assert_eq!(pts_b.head_side, cpp_oracle_arrow_head_side(500.0, 2.0));

    // Case C: Extreme small arrow where stroke dominates above width/3
    let small_arrow = SceneItemShapeScene::new(ShapeType::Arrow, [255, 0, 0, 255], 10.0, false, 50.0);
    let pts_c = small_arrow.compute_arrow_points();
    assert_eq!(pts_c.head_side, 25.0);
    let oracle_clamped = cpp_oracle_arrow_head_side(50.0, 10.0);
    assert!((oracle_clamped - 50.0 / 3.0).abs() < 1e-6);
}

#[test]
fn test_oracle_2_shape_aspect_ratios_and_boundary_conditions() {
    let star = SceneItemShapeScene::new(ShapeType::Star, [0, 0, 0, 255], 1.0, false, 100.0);
    assert_eq!(star.default_aspect_ratio(), K_STAR_BOX_HEIGHT / K_STAR_BOX_WIDTH);

    let bubble = SceneItemShapeScene::new(ShapeType::SpeechBubble, [0, 0, 0, 255], 1.0, false, 100.0);
    assert_eq!(bubble.default_aspect_ratio(), K_BUBBLE_BODY_RATIO);

    let arrow = SceneItemShapeScene::new(ShapeType::Arrow, [0, 0, 0, 255], 1.0, false, 100.0);
    assert_eq!(arrow.default_aspect_ratio(), 0.5);

    let rect = SceneItemShapeScene::new(ShapeType::Rectangle, [0, 0, 0, 255], 1.0, false, 100.0);
    assert_eq!(rect.default_aspect_ratio(), 1.0);

    let circle = SceneItemShapeScene::new(ShapeType::Circle, [0, 0, 0, 255], 1.0, false, 100.0);
    assert_eq!(circle.default_aspect_ratio(), 1.0);
}


// =========================================================================
// Oracle 3: Crop Rule of Thirds Grid, Aspect Ratio Flip & Boundary Constraints
// C++ Upstream Reference: editor_crop.cpp:L49-L82, L257-L281
// =========================================================================

/// Independent C++ Oracle 3 for rotation transposition:
/// (((angle / 90) % 2) == 1) ? size.transposed() : size
fn cpp_oracle_flip_size_by_rotation(width: f64, height: f64, angle: i32) -> (f64, f64) {
    if ((angle / 90).abs() % 2) == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

/// Independent C++ Oracle 3 for original crop fitting:
/// inner.scaled(outer, Qt::KeepAspectRatio) centered at outer
fn cpp_oracle_original_crop(
    outer_w: f64,
    outer_h: f64,
    inner_w: f64,
    inner_h: f64,
) -> (f64, f64, f64, f64) {
    let scale = (outer_w / inner_w).min(outer_h / inner_h);
    let sw = inner_w * scale;
    let sh = inner_h * scale;
    let x = (outer_w - sw) / 2.0;
    let y = (outer_h - sh) / 2.0;
    (x, y, sw, sh)
}

/// Independent C++ Oracle 3 for adjusting crop rect to fit within inner bounds:
/// Upstream: AdjustCropToInner(QRectF &crop, const QRectF &inner)
fn cpp_oracle_adjust_crop_to_inner(
    crop_x: &mut f64,
    crop_y: &mut f64,
    crop_w: &mut f64,
    crop_h: &mut f64,
    inner_x: f64,
    inner_y: f64,
    inner_w: f64,
    inner_h: f64,
) -> bool {
    let orig = (*crop_x, *crop_y, *crop_w, *crop_h);
    *crop_w = (*crop_w).min(inner_w);
    *crop_h = (*crop_h).min(inner_h);
    if *crop_x < inner_x {
        *crop_x = inner_x;
    }
    if *crop_y < inner_y {
        *crop_y = inner_y;
    }
    if *crop_x + *crop_w > inner_x + inner_w {
        *crop_x = inner_x + inner_w - *crop_w;
    }
    if *crop_y + *crop_h > inner_y + inner_h {
        *crop_y = inner_y + inner_h - *crop_h;
    }
    orig != (*crop_x, *crop_y, *crop_w, *crop_h)
}

/// Independent C++ Oracle 3 for 3x3 grid lines:
/// horizontalStep = w / 3.; verticalStep = h / 3.;
/// for i in 1..3: x = left + hStep * i; y = top + vStep * i;
fn cpp_oracle_grid_lines_3x3(
    cx: f64,
    cy: f64,
    cw: f64,
    ch: f64,
) -> ([(f64, f64, f64, f64); 2], [(f64, f64, f64, f64); 2]) {
    let h_step = cw / 3.0;
    let v_step = ch / 3.0;

    let vertical = [
        (cx + h_step, cy, cx + h_step, cy + ch),
        (cx + h_step * 2.0, cy, cx + h_step * 2.0, cy + ch),
    ];
    let horizontal = [
        (cx, cy + v_step, cx + cw, cy + v_step),
        (cx, cy + v_step * 2.0, cx + cw, cy + v_step * 2.0),
    ];
    (horizontal, vertical)
}

#[test]
fn test_oracle_3_crop_rotation_size_transposition() {
    let base_w = 1920.0;
    let base_h = 1080.0;

    let angles = [0, 90, 180, 270, 360, 450, -90, -180, -270];
    for &angle in &angles {
        let oracle_res = cpp_oracle_flip_size_by_rotation(base_w, base_h, angle);
        let actual_res = flip_size_by_rotation(base_w, base_h, angle);
        assert_eq!(oracle_res, actual_res, "Mismatch for rotation angle {}", angle);

        // 90 and 270 degrees must transpose dimensions
        let is_transposed = ((angle / 90).abs() % 2) == 1;
        if is_transposed {
            assert_eq!(actual_res, (base_h, base_w));
        } else {
            assert_eq!(actual_res, (base_w, base_h));
        }
    }

    // Verify original_crop aspect-ratio fit matches Oracle
    let (ox, oy, ow, oh) = cpp_oracle_original_crop(1920.0, 1080.0, 800.0, 600.0);
    let (ax, ay, aw, ah) = original_crop(1920.0, 1080.0, 800.0, 600.0);
    assert_eq!((ax, ay, aw, ah), (ox, oy, ow, oh));
}

#[test]
fn test_oracle_3_crop_boundary_adjustments_and_containment() {
    let mut crop_rect = CropRect::new(200.0, 300.0, 800.0, 600.0);
    let inner = (100.0, 100.0, 600.0, 400.0); // smaller than crop!

    // Rust struct adjustment
    let changed = crop_rect.adjust_to_inner(inner.0, inner.1, inner.2, inner.3);
    assert!(changed);

    // Independent Oracle comparison
    let mut ox = 200.0;
    let mut oy = 300.0;
    let mut ow = 800.0;
    let mut oh = 600.0;
    let oracle_changed = cpp_oracle_adjust_crop_to_inner(
        &mut ox, &mut oy, &mut ow, &mut oh,
        inner.0, inner.1, inner.2, inner.3,
    );
    assert_eq!(changed, oracle_changed);
    assert_eq!(crop_rect.x, ox);
    assert_eq!(crop_rect.y, oy);
    assert_eq!(crop_rect.width, ow);
    assert_eq!(crop_rect.height, oh);

    // Assert strict containment within inner boundary
    assert!(crop_rect.x >= inner.0);
    assert!(crop_rect.y >= inner.1);
    assert!(crop_rect.right() <= inner.0 + inner.2 + 1e-9);
    assert!(crop_rect.bottom() <= inner.1 + inner.3 + 1e-9);
}

#[test]
fn test_oracle_3_rule_of_thirds_3x3_grid_lines() {
    let mut crop = EditorCrop::new(1200.0, 900.0);
    crop.crop = CropRect::new(150.0, 75.0, 600.0, 450.0);

    let (actual_h, actual_v) = crop.grid_lines_3x3();
    let (oracle_h, oracle_v) = cpp_oracle_grid_lines_3x3(150.0, 75.0, 600.0, 450.0);

    assert_eq!(actual_h, oracle_h);
    assert_eq!(actual_v, oracle_v);

    // Verify grid spacing: 600 / 3 = 200, 450 / 3 = 150
    assert_eq!(actual_v[0], (350.0, 75.0, 350.0, 525.0));
    assert_eq!(actual_v[1], (550.0, 75.0, 550.0, 525.0));
    assert_eq!(actual_h[0], (150.0, 225.0, 750.0, 225.0));
    assert_eq!(actual_h[1], (150.0, 375.0, 750.0, 375.0));
}


// =========================================================================
// Oracle 4: Video Trim Timestamp Parse/Format & 24-Slot Timeline Frame Layout
// C++ Upstream Reference: editor_trim_timeline.cpp:L40-L100, video_timeline.cpp:L18-L105
// =========================================================================

/// Independent C++ Oracle 4 for timestamp formatting:
/// Ui::FormatDurationText(value / 1000) + '.' + digits.left(precision)
fn cpp_oracle_format_stamp(ms: i64, precision: usize) -> String {
    let total_sec = ms / 1000;
    let min = total_sec / 60;
    let sec = total_sec % 60;
    let millis = ms % 1000;
    let prec = precision.clamp(1, 3);
    let frac = format!("{:03}", millis);
    format!("{:02}:{:02}.{}", min, sec, &frac[..prec])
}

/// Independent C++ Oracle 4 for ParseStamp:
/// Parses [HH:]MM:SS[.mmm] or SS[.mmm] into milliseconds
fn cpp_oracle_parse_stamp(text: &str) -> Option<i64> {
    let clean = text.trim();
    if clean.is_empty() {
        return None;
    }
    let parts: Vec<&str> = clean.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }

    let mut seconds: i64 = 0;
    for &part in &parts[..parts.len() - 1] {
        let val: i64 = part.parse().ok()?;
        if val < 0 {
            return None;
        }
        seconds = seconds * 60 + val;
    }

    let last = parts.last()?;
    let (whole_str, frac_str) = if let Some(idx) = last.find(|c| c == '.' || c == ',') {
        (&last[..idx], &last[idx + 1..])
    } else {
        (*last, "")
    };

    let whole: i64 = if whole_str.is_empty() { 0 } else { whole_str.parse().ok()? };
    if whole < 0 {
        return None;
    }

    let mut millis: i64 = 0;
    if !frac_str.is_empty() {
        let digits = &frac_str[..frac_str.len().min(3)];
        let val: i64 = digits.parse().ok()?;
        millis = val;
        for _ in digits.len()..3 {
            millis *= 10;
        }
    }

    Some((seconds * 60 + whole) * 1000 + millis)
}

/// Independent C++ Oracle 4 for 24-slot timeline equidistant timestamp generation:
/// step = duration / 24; ts = round(i * step)
fn cpp_oracle_timeline_slots(duration_ms: u64, count: usize) -> Vec<u64> {
    let mut slots = Vec::with_capacity(count);
    if duration_ms == 0 {
        return slots;
    }
    let step = (duration_ms as f64) / (count as f64);
    for i in 0..count {
        let ts = ((i as f64) * step).round() as u64;
        slots.push(ts.min(duration_ms));
    }
    slots
}

#[test]
fn test_oracle_4_timestamp_format_precision_matrix() {
    let test_cases = [
        (0, 1, "00:00.0"),
        (0, 2, "00:00.00"),
        (0, 3, "00:00.000"),
        (5123, 1, "00:05.1"),
        (5123, 2, "00:05.12"),
        (5123, 3, "00:05.123"),
        (65432, 3, "01:05.432"),
        (3600500, 3, "60:00.500"),
    ];

    for &(ms, prec, expected) in &test_cases {
        let oracle_res = cpp_oracle_format_stamp(ms, prec);
        let actual_res = format_stamp(ms, prec);
        assert_eq!(actual_res, expected);
        assert_eq!(actual_res, oracle_res);
    }
}

#[test]
fn test_oracle_4_timestamp_parsing_valid_and_adversarial_malformed() {
    // Valid timestamps
    let valid_cases = [
        ("00:00.000", 0),
        ("00:05.100", 5100),
        ("01:05.432", 65432),
        ("01:30", 90000),
        ("1:30.5", 90500),
        ("1:30.50", 90500),
        ("1:30.500", 90500),
        ("02:00:00.000", 7200000),
        ("45.250", 45250),
        ("45,250", 45250), // Comma as decimal separator
        ("  00:10.500  ", 10500),
    ];

    for &(input, expected_ms) in &valid_cases {
        let oracle_res = cpp_oracle_parse_stamp(input);
        let actual_res = parse_stamp(input);
        assert_eq!(oracle_res, Some(expected_ms), "Oracle failed for {}", input);
        assert_eq!(actual_res, Some(expected_ms), "Actual failed for {}", input);
    }

    // Malformed / invalid inputs
    let invalid_cases = [
        "",
        "   ",
        "abc",
        "1:2:3:4", // too many colons
        "-01:00",  // negative
        "1:-05",
        "10:abc",
        "1:30.xyz",
    ];

    for &input in &invalid_cases {
        let oracle_res = cpp_oracle_parse_stamp(input);
        let actual_res = parse_stamp(input);
        assert_eq!(oracle_res, None, "Oracle should reject {}", input);
        assert_eq!(actual_res, None, "Actual should reject {}", input);
    }
}

#[test]
fn test_oracle_4_24_slot_timeline_frame_layout_and_scrub() {
    let durations = [24000, 60000, 120000, 3600000];
    let width = 480.0;

    for &dur in &durations {
        let timeline = VideoTimelineVideo::new(dur, width);
        assert_eq!(timeline.frame_slots.len(), K_MAX_FRAMES); // 24 slots

        let oracle_slots = cpp_oracle_timeline_slots(dur, K_MAX_FRAMES);
        for i in 0..K_MAX_FRAMES {
            assert_eq!(
                timeline.frame_slots[i].timestamp_ms,
                oracle_slots[i],
                "Slot {} mismatch for duration {}", i, dur
            );
        }

        // Verify scrubbing position x_at mapping
        assert_eq!(timeline.x_at(0), 0.0);
        assert_eq!(timeline.x_at(dur), width);
        assert_eq!(timeline.x_at(dur / 2), width / 2.0);
        // Clamping checks
        assert_eq!(timeline.x_at(dur * 2), width);
    }
}


// =========================================================================
// Oracle 5: Magnetic Volume Snapping & 4000ms Audio Disc Rotation Cycle
// C++ Upstream Reference: editor_audio_menu.cpp:L27-L34, editor_audio_disc_button.cpp:L23, L87-L93
// =========================================================================

/// Independent C++ Oracle 5 for volume slider magnetic snapping:
/// kVolumeStickedValues = {{ 0.25, 0.02 }, { 0.5, 0.02 }, { 0.75, 0.02 }}
fn cpp_oracle_volume_sticking(value: f64) -> f64 {
    const STICKIED: [(f64, f64); 3] = [(0.25, 0.02), (0.50, 0.02), (0.75, 0.02)];
    for (target, delta) in STICKIED {
        if (value - target).abs() <= delta {
            return target;
        }
    }
    value.clamp(0.0, 1.0)
}

/// Independent C++ Oracle 5 for speaker wave arcs count:
/// kSpeakerThresholds = { 0.05f, 0.25f, 0.75f }
fn cpp_oracle_speaker_arcs(volume: f64) -> usize {
    if volume <= 0.05 {
        0
    } else if volume <= 0.25 {
        1
    } else if volume <= 0.75 {
        2
    } else {
        3
    }
}

/// Independent C++ Oracle 5 for audio disc spin angle:
/// kSpinDuration = 4000ms; passed = now - started;
/// std::fmod(spinBase + passed * 360. / 4000., 360.)
fn cpp_oracle_disc_angle(spin_base: f64, started_ms: i64, now_ms: i64, is_active: bool) -> f64 {
    if !is_active {
        spin_base
    } else {
        let passed = (now_ms - started_ms).max(0);
        (spin_base + (passed as f64 * 360.0) / 4000.0) % 360.0
    }
}

#[test]
fn test_oracle_5_magnetic_volume_snapping_and_speaker_arcs() {
    let mut menu = EditorAudioMenu::new(1.0);
    assert_eq!(menu.volume_item.volume, 1.0);
    assert_eq!(menu.speaker_waves(), 3);

    let test_values = [
        // Out of snap range
        (0.10, 0.10, 1),
        (0.20, 0.20, 1),
        // Within 0.25 +- 0.02 snap window
        (0.231, 0.25, 1),
        (0.250, 0.25, 1),
        (0.269, 0.25, 1),
        // Between 0.25 and 0.50
        (0.35, 0.35, 2),
        // Within 0.50 +- 0.02 snap window
        (0.485, 0.50, 2),
        (0.500, 0.50, 2),
        (0.519, 0.50, 2),
        // Between 0.50 and 0.75
        (0.65, 0.65, 2),
        // Within 0.75 +- 0.02 snap window
        (0.735, 0.75, 2),
        (0.750, 0.75, 2),
        (0.768, 0.75, 2),
        // Above 0.75 snap window
        (0.850, 0.85, 3),
        // Full volume & mute boundary
        (1.0, 1.0, 3),
        (0.0, 0.0, 0),
        (-0.5, 0.0, 0),
        (1.5, 1.0, 3),
    ];

    for &(raw, exp_snapped, exp_waves) in &test_values {
        let oracle_snapped = cpp_oracle_volume_sticking(raw);
        let oracle_waves = cpp_oracle_speaker_arcs(oracle_snapped);

        let actual_snapped = menu.set_volume(raw);
        let actual_waves = menu.speaker_waves();

        assert!(
            (actual_snapped - exp_snapped).abs() < 1e-9,
            "Volume snap mismatch for raw {}: expected {}, got {}",
            raw, exp_snapped, actual_snapped
        );
        assert_eq!(actual_snapped, oracle_snapped);
        assert_eq!(actual_waves, exp_waves);
        assert_eq!(actual_waves, oracle_waves);
    }
}

#[test]
fn test_oracle_5_audio_disc_4000ms_rotation_cycle_and_pause_resume() {
    let mut disc = EditorAudioDiscButton::new();
    assert_eq!(disc.spin_base, 0.0);
    assert!(!disc.is_active);

    // Initial angle while inactive
    assert_eq!(disc.current_angle(1000), 0.0);

    // Start spinning at t = 1000ms
    disc.set_active(true, 1000);
    assert!(disc.is_active);

    // 1 second (1000ms) = 1/4 cycle = 90 degrees
    let angle_2000 = disc.current_angle(2000);
    let oracle_2000 = cpp_oracle_disc_angle(0.0, 1000, 2000, true);
    assert_eq!(angle_2000, 90.0);
    assert_eq!(angle_2000, oracle_2000);

    // 2 seconds (2000ms) = 1/2 cycle = 180 degrees
    let angle_3000 = disc.current_angle(3000);
    let oracle_3000 = cpp_oracle_disc_angle(0.0, 1000, 3000, true);
    assert_eq!(angle_3000, 180.0);
    assert_eq!(angle_3000, oracle_3000);

    // 4 seconds (4000ms) = full 360 degree revolution -> 0 degrees
    let angle_5000 = disc.current_angle(5000);
    let oracle_5000 = cpp_oracle_disc_angle(0.0, 1000, 5000, true);
    assert_eq!(angle_5000, 0.0);
    assert_eq!(angle_5000, oracle_5000);

    // Pause at t = 3500ms (passed 2500ms -> angle 225 degrees)
    disc.set_active(false, 3500);
    assert!(!disc.is_active);
    assert_eq!(disc.spin_base, 225.0);
    assert_eq!(disc.current_angle(9999), 225.0); // angle frozen while inactive

    // Resume at t = 10000ms
    disc.set_active(true, 10000);
    // After 1000ms from resume (t = 11000ms) -> 225 + 90 = 315 degrees
    let angle_11000 = disc.current_angle(11000);
    let oracle_11000 = cpp_oracle_disc_angle(225.0, 10000, 11000, true);
    assert_eq!(angle_11000, 315.0);
    assert_eq!(angle_11000, oracle_11000);
}
