//! Comprehensive Subsystem Tests for Module 09 Slice 6:
//! Editor Scene Graph, Controllers & Video Timeline Subsystem
//! Covering Mandates #15, #17, #18, #19

use gpui_telegram_media::editor::scene::scene_item_base::{
    normalize_angle, snap_angle, sticky_line, HandleType, ItemPlacement, ItemStatus, SceneItemBaseScene,
    K_MIN_SIZE_RATIO, K_SNAP_ANGLE, K_STICKY_LINES,
};
use gpui_telegram_media::editor::scene::scene_item_shape::{
    ArrowPoints, SceneItemShapeScene, ShapeType, K_ARROW_HEAD_RATIO, K_ARROW_MAX_BEND_RATIO,
    K_BUBBLE_BODY_RATIO, K_STAR_BOX_HEIGHT, K_STAR_BOX_WIDTH, K_STAR_INNER_RATIO,
};
use gpui_telegram_media::editor::scene::scene_item_canvas::{
    point_distance, BrushTool, SceneItemCanvasScene, K_HALF_STRENGTH, K_INV_STRENGTH,
    K_MIN_POINT_DISTANCE_BASE, K_SMOOTHING_STRENGTH,
};
use gpui_telegram_media::editor::scene::scene_item_text::{
    effective_text_color, SceneItemTextScene, TextAlignment, TextStyle, TextTypeface,
    K_BRIGHTNESS_FRAMED_THRESHOLD, K_PADDING_FACTOR, K_SEMI_TRANSPARENT_ALPHA,
};
use gpui_telegram_media::editor::scene::scene_text_editing::{
    SceneTextEditingScene, K_AUTO_SHRINK_HEIGHT_FACTOR, K_AUTO_SHRINK_STEP, K_MIN_FONT_SIZE_FACTOR,
};
use gpui_telegram_media::editor::scene::scene::{SceneAction, SceneScene};
use gpui_telegram_media::editor::controllers::undo_controller::{
    EnableRequest, UndoCommand, UndoControllerControllers,
};
use gpui_telegram_media::editor::controllers::stickers_panel_controller::{
    ShowRequest, StickersPanelControllerControllers,
};
use gpui_telegram_media::editor::video::video_clip::{VideoClipVideo, VideoTrim};
use gpui_telegram_media::editor::video::video_editor_quality::{
    compute_video_quality_levels, downscaled_size, video_quality_label, VideoEditorQualityVideo,
    K_QUALITY_LEVELS,
};
use gpui_telegram_media::editor::video::video_quality_slider::{VideoQualitySliderVideo, K_SHIFT_DURATION_MS};
use gpui_telegram_media::editor::video::video_timeline::{
    VideoTimelineVideo, K_DOT_DURATION_MS, K_MAX_FRAMES, K_RELOAD_DELAY_MS,
};
use gpui_telegram_media::editor::video::video_timeline_seeker::{
    VideoTimelineSeekerVideo, K_SEEK_THROTTLE_MS,
};
use gpui_telegram_media::editor::video::video_segment_player::{
    SegmentPlayerOptions, VideoSegmentPlayerVideo,
};
use gpui_telegram_media::editor::video::video_editor_common::{
    edited_frame_size, video_edited, VideoEditorCommonVideo, VideoModifications,
};
use gpui_telegram_media::editor::video::video_editor_layer::{
    acceptable_dimensions, VideoEditorLayerVideo, K_MAX_SIDE_RATIO, K_PROFILE_VIDEO_FPS,
    K_PROFILE_VIDEO_MAX_DURATION_MS, K_PROFILE_VIDEO_MIN_DURATION_MS, K_PROFILE_VIDEO_SIDE,
};
use gpui_telegram_media::editor::video::video_editor::{
    VideoEditorVideo, K_BUBBLE_DURATION_MS, K_MAX_GIF_SIZE_BYTES,
};

#[test]
fn test_01_affine_transform_angle_normalization_and_snap() {
    assert_eq!(normalize_angle(0.0), 0.0);
    assert_eq!(normalize_angle(360.0), 0.0);
    assert_eq!(normalize_angle(-360.0), 0.0);
    assert_eq!(normalize_angle(400.0), 40.0);
    assert_eq!(normalize_angle(-400.0), -40.0);

    // 45-degree angle snapping
    assert_eq!(snap_angle(42.0), 45.0);
    assert_eq!(snap_angle(47.0), 45.0);
    assert_eq!(snap_angle(88.0), 90.0);
    assert_eq!(snap_angle(134.0), 135.0);
    assert_eq!(snap_angle(-43.0), -45.0);
}

#[test]
fn test_02_sticky_guidelines_generation() {
    let start = 100.0;
    let length = 800.0;
    let lines: Vec<f64> = (0..K_STICKY_LINES)
        .map(|idx| sticky_line(start, length, idx))
        .collect();

    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], 100.0);
    assert_eq!(lines[1], 300.0);
    assert_eq!(lines[2], 500.0);
    assert_eq!(lines[3], 700.0);
    assert_eq!(lines[4], 900.0);
}

#[test]
fn test_03_item_base_handle_bounds_and_resize() {
    let mut item = SceneItemBaseScene::new(10, (200.0, 300.0), 100.0, 1.0);
    assert_eq!(item.horizontal_size, 100.0);
    assert_eq!(item.vertical_size, 100.0);

    let (rx, ry, rw, rh) = item.right_handle_rect();
    assert_eq!(rw, 12.0);
    assert_eq!(rh, 12.0);
    assert_eq!(rx, 50.0 - 6.0);
    assert_eq!(ry, -6.0);

    let (lx, ly, lw, lh) = item.left_handle_rect();
    assert_eq!(lw, 12.0);
    assert_eq!(lh, 12.0);
    assert_eq!(lx, -50.0 - 6.0);
    assert_eq!(ly, -6.0);

    // Drag to resize
    item.apply_resize(80.0, 80.0, false);
    assert_eq!(item.horizontal_size, 160.0);
    assert_eq!(item.vertical_size, 160.0);
}

#[test]
fn test_04_star_shape_parametric_geometry() {
    let star = SceneItemShapeScene::new(ShapeType::Star, [255, 215, 0, 255], 2.0, true, 100.0);
    let expected_ratio = K_STAR_BOX_HEIGHT / K_STAR_BOX_WIDTH;
    assert!((star.default_aspect_ratio() - expected_ratio).abs() < 1e-6);

    let vertices = star.star_vertices(50.0);
    assert_eq!(vertices.len(), 10);
    // Outer vertex at index 0 (top point)
    assert!((vertices[0].0).abs() < 1e-4);
    assert!((vertices[0].1 - (-50.0)).abs() < 1e-4);

    // Inner radius at index 1
    let inner_r = 50.0 * K_STAR_INNER_RATIO;
    let r1 = (vertices[1].0 * vertices[1].0 + vertices[1].1 * vertices[1].1).sqrt();
    assert!((r1 - inner_r).abs() < 1e-4);
}

#[test]
fn test_05_arrow_shape_bend_and_head_geometry() {
    let mut arrow = SceneItemShapeScene::new(ShapeType::Arrow, [255, 0, 0, 255], 4.0, false, 200.0);
    arrow.set_bend(0.5);

    let points = arrow.compute_arrow_points();
    assert_eq!(points.start, (-100.0, 0.0));
    assert_eq!(points.end, (100.0, 0.0));
    let expected_bend = 0.5 * 200.0 * K_ARROW_MAX_BEND_RATIO;
    assert!((points.control.1 - expected_bend).abs() < 1e-6);
    let expected_head = (200.0 * K_ARROW_HEAD_RATIO).max(4.0 * 2.5);
    assert!((points.head_side - expected_head).abs() < 1e-6);
}

#[test]
fn test_06_speech_bubble_aspect_ratio() {
    let bubble = SceneItemShapeScene::new(ShapeType::SpeechBubble, [255, 255, 255, 255], 2.0, true, 120.0);
    assert!((bubble.default_aspect_ratio() - K_BUBBLE_BODY_RATIO).abs() < 1e-6);
}

#[test]
fn test_07_canvas_stroke_smoothing_algorithm() {
    let mut canvas = SceneItemCanvasScene::new((0.0, 0.0, 1000.0, 1000.0));
    canvas.set_brush(BrushTool::Pen, [0, 0, 0, 255], 3.0);

    // Initial point
    assert!(canvas.add_point((10.0, 10.0), 0.5, 100));
    // Point too close (distance < 2.0)
    assert!(!canvas.add_point((11.0, 10.5), 0.5, 110));
    // Valid subsequent points
    assert!(canvas.add_point((20.0, 20.0), 0.5, 120));
    assert!(canvas.add_point((30.0, 30.0), 0.5, 130));

    assert_eq!(canvas.points.len(), 3);
    assert_eq!(canvas.smoothed_points.len(), 3);

    // Verify smoothing calculation for 3rd point
    let p0 = (10.0, 10.0);
    let p1 = (20.0, 20.0);
    let p2 = (30.0, 30.0);
    let expected_sx = p0.0 * K_HALF_STRENGTH + p1.0 * K_INV_STRENGTH + p2.0 * K_HALF_STRENGTH;
    let expected_sy = p0.1 * K_HALF_STRENGTH + p1.1 * K_INV_STRENGTH + p2.1 * K_HALF_STRENGTH;
    assert!((canvas.smoothed_points[2].0 - expected_sx).abs() < 1e-6);
    assert!((canvas.smoothed_points[2].1 - expected_sy).abs() < 1e-6);
}

#[test]
fn test_08_text_bubble_effective_color_and_padding() {
    let text_item = SceneItemTextScene::new("Caption".to_string(), [255, 255, 255, 255], 30.0, TextStyle::Framed);
    assert_eq!(text_item.padding, 30.0 * K_PADDING_FACTOR);

    // Bright background triggers black foreground in Framed mode
    let fg_on_white = effective_text_color([255, 255, 255, 255], TextStyle::Framed);
    assert_eq!(fg_on_white, [0, 0, 0, 255]);

    // Dark background triggers white foreground in Framed mode
    let fg_on_black = effective_text_color([20, 20, 20, 255], TextStyle::Framed);
    assert_eq!(fg_on_black, [255, 255, 255, 255]);

    // SemiTransparent background has 0x99 alpha
    let semi_item = SceneItemTextScene::new("Semi".to_string(), [255, 255, 255, 255], 24.0, TextStyle::SemiTransparent);
    let bg = semi_item.text_background_color();
    assert_eq!(bg[3], K_SEMI_TRANSPARENT_ALPHA);
}

#[test]
fn test_09_text_auto_shrink_budget_scaling() {
    let mut editor = SceneTextEditingScene::new(600.0);
    editor.start_editing(1, "Very long line".to_string(), 40.0, [255, 255, 255, 255]);

    let _budget = 600.0 * K_AUTO_SHRINK_HEIGHT_FACTOR; // 200.0
    // Height below budget -> no shrink
    assert!(!editor.apply_auto_shrink(150.0));
    assert_eq!(editor.font_size, 40.0);

    // Height exceeds budget -> shrinks by K_AUTO_SHRINK_STEP (0.9)
    assert!(editor.apply_auto_shrink(250.0));
    assert!((editor.font_size - 36.0).abs() < 1e-6);
}

#[test]
fn test_10_scene_undo_redo_transaction_stack() {
    let mut scene = SceneScene::new((0.0, 0.0, 1000.0, 1000.0));
    assert!(!scene.has_undo());
    assert!(!scene.has_redo());

    let item1 = SceneItemBaseScene::new(1, (100.0, 100.0), 120.0, 1.0);
    scene.add_item(item1);
    assert_eq!(scene.items.len(), 1);
    assert!(scene.has_undo());
    assert!(!scene.has_redo());

    let item2 = SceneItemBaseScene::new(2, (200.0, 200.0), 120.0, 1.0);
    scene.add_item(item2);
    assert_eq!(scene.items.len(), 2);

    // Undo adding item2
    assert!(scene.perform_undo());
    assert_eq!(scene.items.len(), 1);
    assert_eq!(scene.items[0].id, 1);
    assert!(scene.has_redo());

    // Redo adding item2
    assert!(scene.perform_redo());
    assert_eq!(scene.items.len(), 2);
    assert_eq!(scene.items[1].id, 2);
}

#[test]
fn test_11_video_clip_trim_clamping_and_loop_duration() {
    let mut clip = VideoClipVideo::new("test.mp4".to_string(), 10000, true);
    assert_eq!(clip.loop_duration(), 10000);

    // Valid trim
    clip.set_trim(VideoTrim { from: 2000, till: 8000 });
    assert_eq!(clip.trim.from, 2000);
    assert_eq!(clip.trim.till, 8000);
    assert_eq!(clip.loop_duration(), 6000);

    // Out of bounds trim clamped to duration
    clip.set_trim(VideoTrim { from: 12000, till: 15000 });
    assert_eq!(clip.trim.from, 10000);
    assert_eq!(clip.trim.till, 10000);
    assert_eq!(clip.loop_duration(), 0);
}

#[test]
fn test_12_video_quality_ladder_and_downscale() {
    let levels = compute_video_quality_levels((1920, 1080));
    // Shorter side is 1080 -> rungs for 360, 480, 720, and 0 (original 1080)
    assert_eq!(levels.len(), 4);
    assert_eq!(levels[0].shorter_side, 360);
    assert_eq!(levels[1].shorter_side, 480);
    assert_eq!(levels[2].shorter_side, 720);
    assert_eq!(levels[3].shorter_side, 0);

    assert_eq!(video_quality_label(&levels[0]), "360p");
    assert_eq!(video_quality_label(&levels[3]), "Original");

    // Even resolution alignment
    let downscaled = downscaled_size((1920, 1080), 720);
    assert_eq!(downscaled.0 % 2, 0);
    assert_eq!(downscaled.1 % 2, 0);
}

#[test]
fn test_13_profile_video_layer_constraints() {
    let layer = VideoEditorLayerVideo::new(true);
    assert_eq!(layer.config.exact_size, (K_PROFILE_VIDEO_SIDE, K_PROFILE_VIDEO_SIDE));
    assert_eq!(layer.config.max_duration_ms, K_PROFILE_VIDEO_MAX_DURATION_MS);
    assert_eq!(layer.config.min_duration_ms, K_PROFILE_VIDEO_MIN_DURATION_MS);
    assert_eq!(layer.config.fps_limit, K_PROFILE_VIDEO_FPS);
    assert!(layer.config.remove_audio);
    assert!(layer.config.transcode_always);

    // Aspect ratio within 10:1
    assert!(acceptable_dimensions(1920, 1080));
    assert!(acceptable_dimensions(1000, 100));
    assert!(!acceptable_dimensions(1100, 100)); // 11:1 fails
}

#[test]
fn test_14_timeline_scrubbing_throttler_state_machine() {
    let mut seeker = VideoTimelineSeekerVideo::new();

    // First seek immediately executes
    let executed1 = seeker.seek(1500, 1000);
    assert_eq!(executed1, Some(1500));
    assert_eq!(seeker.current_position_ms, 1500);

    // Fast subsequent seek within 120ms is throttled/pending
    let throttled = seeker.seek(2000, 1050);
    assert_eq!(throttled, None);
    assert_eq!(seeker.pending_seek_ms, Some(2000));

    // Seek after 120ms executes
    let executed2 = seeker.seek(2500, 1150);
    assert_eq!(executed2, Some(2500));
    assert_eq!(seeker.current_position_ms, 2500);
}

#[test]
fn test_15_undo_controller_state_and_commands() {
    let mut undo = UndoControllerControllers::new();
    assert!(!undo.can_perform(UndoCommand::Undo));
    assert!(!undo.can_perform(UndoCommand::Redo));

    undo.update_state(3, 1);
    assert!(undo.can_perform(UndoCommand::Undo));
    assert!(undo.can_perform(UndoCommand::Redo));
    assert_eq!(undo.undo_history_count, 3);
    assert_eq!(undo.redo_history_count, 1);

    undo.update_state(0, 4);
    assert!(!undo.can_perform(UndoCommand::Undo));
    assert!(undo.can_perform(UndoCommand::Redo));
}

#[test]
fn test_16_stickers_panel_controller_fsm() {
    let mut panel = StickersPanelControllerControllers::new(true);
    assert!(!panel.is_shown);
    assert!(panel.with_audio);

    panel.handle_show_request(ShowRequest::ShowAnimated);
    assert!(panel.is_shown);

    panel.handle_show_request(ShowRequest::ToggleAnimated);
    assert!(!panel.is_shown);

    panel.handle_show_request(ShowRequest::ToggleAnimated);
    assert!(panel.is_shown);

    panel.handle_show_request(ShowRequest::HideFast);
    assert!(!panel.is_shown);

    panel.set_anchor(320, 480);
    assert_eq!(panel.anchor_pos, (320, 480));
}

#[test]
fn test_17_video_quality_slider_notch_math() {
    let levels = vec![
        gpui_telegram_media::editor::video::video_editor_quality::VideoQualityLevel { shorter_side: 360, resolution: (640, 360) },
        gpui_telegram_media::editor::video::video_editor_quality::VideoQualityLevel { shorter_side: 720, resolution: (1280, 720) },
        gpui_telegram_media::editor::video::video_editor_quality::VideoQualityLevel { shorter_side: 0, resolution: (1920, 1080) },
    ];
    let slider = VideoQualitySliderVideo::new(levels);
    assert_eq!(K_SHIFT_DURATION_MS, 120);
    assert!(slider.has_choice());
    assert_eq!(slider.dot_x(0), 0.0);
    assert_eq!(slider.dot_x(1), 120.0);
    assert_eq!(slider.dot_x(2), 240.0);

    assert_eq!(slider.index_at(10.0), 0);
    assert_eq!(slider.index_at(110.0), 1);
    assert_eq!(slider.index_at(230.0), 2);
}

#[test]
fn test_18_video_timeline_slots_and_coordinates() {
    let timeline = VideoTimelineVideo::new(24000, 480.0);
    assert_eq!(K_MAX_FRAMES, 24);
    assert_eq!(K_DOT_DURATION_MS, 500);
    assert_eq!(K_RELOAD_DELAY_MS, 150);
    assert_eq!(timeline.frame_slots.len(), 24);

    assert_eq!(timeline.x_at(0), 0.0);
    assert_eq!(timeline.x_at(12000), 240.0);
    assert_eq!(timeline.x_at(24000), 480.0);
}

#[test]
fn test_19_video_segment_player_looping_fsm() {
    let options = SegmentPlayerOptions { keep_alpha: false, audio: true, volume: 0.8 };
    let mut player = VideoSegmentPlayerVideo::new("test.mp4".to_string(), 1000, 5000, options);
    assert_eq!(player.position_ms, 1000);
    assert!(!player.is_held());

    // Advance within segment
    assert_eq!(player.tick(2000), 3000);
    assert_eq!(player.position_ms, 3000);

    // Advance beyond segment till_ms -> loops back to from_ms (1000)
    assert_eq!(player.tick(3000), 1000);
    assert_eq!(player.position_ms, 1000);

    player.set_paused(true);
    assert!(player.is_held());
    assert_eq!(player.tick(500), 1000); // Paused -> no advance
}

#[test]
fn test_20_video_editor_common_and_size_estimation() {
    let mut editor = VideoEditorVideo::new("sample.mp4".to_string(), (1920, 1080), 10000, 10_000_000);
    assert_eq!(K_BUBBLE_DURATION_MS, 500);
    assert_eq!(K_MAX_GIF_SIZE_BYTES, 10 * 1024 * 1024);

    assert_eq!(editor.estimate_output_size(), 10_000_000);

    // Trim in half -> size drops by half
    editor.modifications.from_ms = 2000;
    editor.modifications.till_ms = 7000;
    assert_eq!(editor.estimate_output_size(), 5_000_000);

    // Toggle GIF
    assert!(editor.toggle_gif());
    assert!(editor.is_gif);

    // Edited frame size with 90 degree rotation swaps width and height
    let rotated = edited_frame_size((1920, 1080), None, 90);
    assert_eq!(rotated, (1080, 1920));

    // Video edited check
    let common = VideoEditorCommonVideo::new((1920, 1080), 10000, true);
    assert!(!common.is_reencode_required());
    let mut edited_mods = common.modifications.clone();
    edited_mods.rotation_angle = 90;
    assert!(video_edited(&edited_mods, (1920, 1080), 10000, true));
}
