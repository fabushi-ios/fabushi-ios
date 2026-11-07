//! Integration & unit tests for Module 09 Slice 7: Editor Root & Theme Subsystem.
//! Verifies color picker math, brush geometry, crop rotation, grid math,
//! timeline stamp parsing, volume snapping, audio disc spin, and link pills.

use gpui_telegram_media::editor::{
    audio_track_timeline::*,
    color_picker::*,
    editor_audio_disc_button::*,
    editor_audio_menu::*,
    editor_audio_timeline::*,
    editor_crop::*,
    editor_keys_legend::*,
    editor_layer_widget::*,
    editor_link_box::*,
    editor_link_pill::*,
    editor_message_render::*,
    editor_message_source::*,
    editor_message_video::*,
    editor_paint::*,
    editor_trim_timeline::*,
    photo_editor::*,
    photo_editor_common::*,
    photo_editor_content::*,
    photo_editor_controls::*,
    photo_editor_inner_common::*,
    photo_editor_layer_widget::*,
    video_item_timeline::*,
};

#[test]
fn test_color_picker_palette_and_gradient() {
    // 10 default palette colors from color_picker.cpp:L266-L279
    assert_eq!(PALETTE_COLORS.len(), 10);
    assert_eq!(PALETTE_COLORS[0], [0, 0, 0, 255]); // Black
    assert_eq!(PALETTE_COLORS[1], [255, 255, 255, 255]); // White
    assert_eq!(PALETTE_COLORS[2], [234, 39, 57, 255]); // Red (#EA2739)
    assert_eq!(PALETTE_COLORS[7], [48, 81, 227, 255]); // Blue (#3051E3)

    // 7 rainbow wheel gradient stops from color_picker.cpp:L143-L151
    assert_eq!(WHEEL_GRADIENT_STOPS.len(), 7);
    assert_eq!(WHEEL_GRADIENT_STOPS[0], [0xEB, 0x4B, 0x4B]);
    assert_eq!(WHEEL_GRADIENT_STOPS[4], [0x00, 0xFF, 0xFF]);
}

#[test]
fn test_color_picker_tool_switching_and_fixed_colors() {
    let mut picker = ColorPicker::new();
    assert_eq!(picker.current_tool, BrushTool::Pen);
    assert_eq!(picker.current_brush.color, [234, 39, 57, 255]);

    // Switch to Eraser -> fixed color black
    picker.set_tool(BrushTool::Eraser);
    assert_eq!(picker.current_tool, BrushTool::Eraser);
    assert_eq!(picker.current_brush.color, [0, 0, 0, 255]);
    assert!(fixed_color_tool(BrushTool::Eraser));
    assert!(fixed_color_tool(BrushTool::Blur));
    assert!(!fixed_color_tool(BrushTool::Marker));

    // Attempting to set color on Eraser must be ignored
    picker.set_color([100, 200, 50, 255]);
    assert_eq!(picker.current_brush.color, [0, 0, 0, 255]);

    // Switch back to Pen -> restores Pen color
    picker.set_tool(BrushTool::Pen);
    assert_eq!(picker.current_brush.color, [234, 39, 57, 255]);
}

#[test]
fn test_color_picker_size_control_math() {
    let sc = SizeControl::new(100, 300);
    // At bottom (y = 300), ratio is 0.0
    assert_eq!(sc.ratio_from_y(300), 0.0);
    // At top (y = 100), ratio is 1.0
    assert_eq!(sc.ratio_from_y(100), 1.0);
    // At middle (y = 200), ratio is 0.5
    assert_eq!(sc.ratio_from_y(200), 0.5);

    // Inverse mapping
    assert_eq!(sc.y_from_ratio(0.0), 300);
    assert_eq!(sc.y_from_ratio(1.0), 100);
    assert_eq!(sc.y_from_ratio(0.5), 200);

    // Clamping checks
    assert_eq!(sc.ratio_from_y(50), 1.0);
    assert_eq!(sc.ratio_from_y(400), 0.0);

    // Brush width calculation from ratio
    let width_min = brush_width_from_ratio(0.0);
    let width_max = brush_width_from_ratio(1.0);
    assert_eq!(width_min, 1.0);
    assert_eq!(width_max, 25.0);
}

#[test]
fn test_brush_width_and_arrowhead_geometry() {
    assert_eq!(brush_size(0.0), 1.0);
    assert_eq!(brush_size(1.0), 25.0);
    assert_eq!(brush_size(0.5), 13.0);

    let start = (0.0, 0.0);
    let end = (100.0, 0.0);
    let (b1, b2) = arrow_head_points(start, end, 10.0);

    // Arrowhead endpoints should point backward (x < 100.0) symmetrically around y=0
    assert!(b1.0 < 100.0);
    assert!(b2.0 < 100.0);
    assert!((b1.1 + b2.1).abs() < 1e-4); // symmetrical y offset
    assert!(b1.1.abs() > 0.0);
}

#[test]
fn test_crop_rotation_and_original_aspect_ratio() {
    let (w0, h0) = flip_size_by_rotation(1920.0, 1080.0, 0);
    assert_eq!((w0, h0), (1920.0, 1080.0));

    let (w90, h90) = flip_size_by_rotation(1920.0, 1080.0, 90);
    assert_eq!((w90, h90), (1080.0, 1920.0));

    let (w180, h180) = flip_size_by_rotation(1920.0, 1080.0, 180);
    assert_eq!((w180, h180), (1920.0, 1080.0));

    let (w270, h270) = flip_size_by_rotation(1920.0, 1080.0, 270);
    assert_eq!((w270, h270), (1080.0, 1920.0));

    // OriginalCrop aspect ratio fitting
    let (cx, cy, cw, ch) = original_crop(1000.0, 1000.0, 800.0, 400.0);
    assert_eq!(cw, 1000.0);
    assert_eq!(ch, 500.0);
    assert_eq!(cx, 0.0);
    assert_eq!(cy, 250.0); // centered vertically
}

#[test]
fn test_crop_rule_of_thirds_grid() {
    let crop = EditorCrop::new(300.0, 300.0);
    let (h_lines, v_lines) = crop.grid_lines_3x3();

    // Horizontal lines at y = 100, y = 200
    assert_eq!(h_lines[0].1, 100.0);
    assert_eq!(h_lines[0].3, 100.0);
    assert_eq!(h_lines[1].1, 200.0);
    assert_eq!(h_lines[1].3, 200.0);

    // Vertical lines at x = 100, x = 200
    assert_eq!(v_lines[0].0, 100.0);
    assert_eq!(v_lines[0].2, 100.0);
    assert_eq!(v_lines[1].0, 200.0);
    assert_eq!(v_lines[1].2, 200.0);
}

#[test]
fn test_crop_rounded_corners_multiplier() {
    assert_eq!(RoundedCornersLevel::Circle.multiplier(), 0.50);
    assert_eq!(RoundedCornersLevel::Large.multiplier(), 0.44);
    assert_eq!(RoundedCornersLevel::Medium.multiplier(), 0.20);
    assert_eq!(RoundedCornersLevel::Small.multiplier(), 0.12);
    assert_eq!(RoundedCornersLevel::None.multiplier(), 0.00);

    let radius = PhotoEditorCommon::calculate_shape_radius(100.0, 200.0, RoundedCornersLevel::Large);
    assert_eq!(radius, 44.0); // min(100, 200) * 0.44
}

#[test]
fn test_trim_timeline_timestamp_parser_and_formatter() {
    // Parser checks
    assert_eq!(parse_stamp("12.500"), Some(12500));
    assert_eq!(parse_stamp("01:23.450"), Some(83450));
    assert_eq!(parse_stamp("02:05:10.125"), Some(7510125));
    assert_eq!(parse_stamp("invalid"), None);
    assert_eq!(parse_stamp(""), None);

    // Formatter checks
    assert_eq!(format_stamp(83450, 1), "01:23.4");
    assert_eq!(format_stamp(83450, 3), "01:23.450");
    assert_eq!(format_stamp(500, 2), "00:00.50");
}

#[test]
fn test_audio_menu_volume_magnetic_snapping() {
    // Within delta 0.02 of 0.25 -> snaps to 0.25
    assert_eq!(apply_volume_sticking(0.24), 0.25);
    assert_eq!(apply_volume_sticking(0.26), 0.25);

    // Within delta 0.02 of 0.50 -> snaps to 0.50
    assert_eq!(apply_volume_sticking(0.51), 0.50);

    // Far from snap points -> remains exact
    assert_eq!(apply_volume_sticking(0.40), 0.40);
    assert_eq!(apply_volume_sticking(0.90), 0.90);

    // Sound wave arcs
    assert_eq!(speaker_arc_count(0.02), 0);
    assert_eq!(speaker_arc_count(0.15), 1);
    assert_eq!(speaker_arc_count(0.50), 2);
    assert_eq!(speaker_arc_count(0.85), 3);
}

#[test]
fn test_audio_disc_spinning_angle() {
    // Inactive -> returns base angle
    assert_eq!(angle_at_time(45.0, 1000, 2000, false), 45.0);

    // Active: at passed = 1000ms (1/4 of 4000ms spin duration)
    let angle_quarter = angle_at_time(0.0, 1000, 2000, true);
    assert!((angle_quarter - 90.0).abs() < 1e-4);

    // Full spin (4000ms) -> wraps to 0
    let angle_full = angle_at_time(0.0, 1000, 5000, true);
    assert!((angle_full - 0.0).abs() < 1e-4);
}

#[test]
fn test_link_pill_density_and_colors() {
    assert_eq!(EditorLinkPill::density_for(720), 2.0);
    assert_eq!(EditorLinkPill::density_for(360), 1.0);

    assert_eq!(without_scheme("https://telegram.org/blog/"), "telegram.org/blog");
    assert_eq!(without_scheme("http://example.com"), "example.com");

    let mut pill = EditorLinkPill::new(
        LinkPreview {
            url: "https://t.me/durov".to_string(),
            name: "DUROV".to_string(),
            ..Default::default()
        },
        1.0,
        300,
    );

    // Default style Framed
    assert_eq!(pill.background_color(), [255, 255, 255, 255]);
    assert_eq!(pill.foreground_color(), [0x33, 0x91, 0xD4, 255]);

    // SemiTransparent
    pill.set_style(LinkStyle::SemiTransparent);
    assert_eq!(pill.background_color(), [0, 0, 0, 0x4C]);
    assert_eq!(pill.foreground_color(), [255, 255, 255, 255]);

    // Opaque
    pill.set_style(LinkStyle::Opaque);
    assert_eq!(pill.background_color(), [0, 0, 0, 255]);
    assert_eq!(pill.foreground_color(), [255, 255, 255, 255]);
}

#[test]
fn test_opaque_bounds_alpha_scanning() {
    let width = 4;
    let height = 4;
    // 4x4 image with a 2x2 non-zero center square at (1,1) to (2,2)
    let mut alpha = vec![0u8; 16];
    alpha[1 * 4 + 1] = 255;
    alpha[1 * 4 + 2] = 255;
    alpha[2 * 4 + 1] = 255;
    alpha[2 * 4 + 2] = 255;

    let bounds = opaque_bounds(width, height, &alpha);
    assert_eq!(bounds, Some((1, 1, 2, 2)));

    // Completely transparent image returns None
    let empty_alpha = vec![0u8; 16];
    assert_eq!(opaque_bounds(width, height, &empty_alpha), None);
}

#[test]
fn test_photo_editor_integration_flow() {
    let mut editor = PhotoEditor::new(
        1920,
        1080,
        PhotoModifications::default(),
        EditorData::default(),
    );

    // Switch to paint mode
    editor.set_mode(EditorMode::Paint);
    assert!(editor.color_picker.is_visible);
    assert_eq!(editor.controls.current_mode.mode, EditorMode::Paint);

    // Switch tool to Marker
    editor.set_brush_tool(BrushTool::Marker);
    assert_eq!(editor.color_picker.current_tool, BrushTool::Marker);

    // Rotate 90 degrees
    editor.content.crop.rotate_cw_90();
    assert_eq!(editor.content.crop.angle, 90);
    assert_eq!(editor.content.crop.crop.width, 1080.0);
    assert_eq!(editor.content.crop.crop.height, 1920.0);

    // Save modifications
    let mods = editor.save();
    assert!(editor.is_done);
    assert_eq!(mods.angle, 90);
    assert_eq!(mods.crop_width, 1080);
    assert_eq!(mods.crop_height, 1920);
}
