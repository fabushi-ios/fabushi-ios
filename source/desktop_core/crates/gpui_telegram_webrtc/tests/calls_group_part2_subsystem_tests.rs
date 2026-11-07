//! Subsystem Tests for Module 10 Slice 3: Group Calls (Part 2)
//! Covers: Group call settings, PTT delays, mic level tester, Stars donations box,
//! toasts notifications, multi-tile viewport layout, OpenGL/RHI shaders,
//! video tile geometry, participant volume slider with magnetic snap,
//! recording box, scheduled labels, stars coloring tiers, desktop capture.

use gpui_telegram_webrtc::calls::group::*;
#[allow(unused_imports)]
use gpui_telegram_webrtc::calls::group::ui::*;

#[test]
fn test_ptt_delay_quantization_and_formatting() {
    // Upstream kDelaysCount = 201 (0..2000ms in 10ms steps)
    assert_eq!(calls_group_settings::K_DELAYS_COUNT, 201);
    assert_eq!(calls_group_settings::delay_by_index(0), 0);
    assert_eq!(calls_group_settings::delay_by_index(20), 200);
    assert_eq!(calls_group_settings::delay_by_index(100), 1000);
    assert_eq!(calls_group_settings::delay_by_index(200), 2000);
    assert_eq!(calls_group_settings::delay_by_index(999), 2000); // clamped

    // Formatting threshold at 1000ms: < 1000ms uses ms, >= 1000ms uses s
    assert_eq!(calls_group_settings::format_delay(200), "200 ms");
    assert_eq!(calls_group_settings::format_delay(990), "990 ms");
    assert_eq!(calls_group_settings::format_delay(1000), "1.00 s");
    assert_eq!(calls_group_settings::format_delay(1500), "1.50 s");
    assert_eq!(calls_group_settings::format_delay(2000), "2.00 s");
}

#[test]
fn test_mic_level_tester_tooltip_trigger() {
    let mut tester = calls_group_settings::MicLevelTester::new();

    // Quiet samples below 0.2 do not trigger tooltip
    assert!(!tester.update_sample(0.1));
    assert!(!tester.update_sample(0.19));
    assert_eq!(tester.loud_count, 0);

    // Loud samples >= 0.2 accumulate, trigger tooltip on 3rd sample
    assert!(!tester.update_sample(0.25));
    assert_eq!(tester.loud_count, 1);
    assert!(!tester.update_sample(0.5));
    assert_eq!(tester.loud_count, 2);
    assert!(tester.update_sample(0.8)); // 3rd loud sample triggers!
    assert_eq!(tester.loud_count, 3);
    assert!(tester.tooltip_shown);

    // Subsequent loud sample does not duplicate tooltip
    assert!(!tester.update_sample(0.9));

    // 5 quiet samples reset the loud accumulator
    for _ in 0..4 {
        tester.update_sample(0.05);
        assert!(tester.tooltip_shown);
    }
    tester.update_sample(0.05); // 5th quiet sample resets
    assert!(!tester.tooltip_shown);
    assert_eq!(tester.loud_count, 0);
}

#[test]
fn test_video_stream_stars_box_clamp_and_ranking() {
    // Minimum clamped to 2
    assert_eq!(calls_group_stars_box::max_video_stream_stars_count(Some(1)), 2);
    assert_eq!(calls_group_stars_box::max_video_stream_stars_count(Some(50)), 50);
    assert_eq!(calls_group_stars_box::max_video_stream_stars_count(None), 10_000);

    let mut box_ctrl = CallsGroupStarsBoxGroup::new(5, 0, Some(500), false);
    assert_eq!(box_ctrl.min_stars, 5);
    assert_eq!(box_ctrl.chosen_stars, 10); // default fallback
    assert_eq!(box_ctrl.max_stars, 500);

    box_ctrl.user_balance = 100;
    assert!(box_ctrl.can_submit());

    box_ctrl.set_chosen_stars(200);
    assert!(!box_ctrl.can_submit()); // insufficient balance

    // Donor ranking
    box_ctrl.add_or_update_donor(101, "Alice".to_string(), 50, false, false);
    box_ctrl.add_or_update_donor(102, "Bob".to_string(), 150, false, false);
    box_ctrl.add_or_update_donor(103, "Charlie".to_string(), 25, true, false);

    assert_eq!(box_ctrl.top_donors[0].name, "Bob");
    assert_eq!(box_ctrl.top_donors[0].count, 150);
    assert_eq!(box_ctrl.top_donors[1].name, "Alice");
    assert_eq!(box_ctrl.top_donors[2].name, "Charlie");
}

#[test]
fn test_group_call_toasts_error_mapping_and_duration() {
    assert_eq!(calls_group_toasts::K_ERROR_DURATION_MS, 2000);

    let errors = [
        (calls_group_toasts::GroupCallError::NoCamera, "lng_call_error_no_camera"),
        (calls_group_toasts::GroupCallError::CameraFailed, "lng_group_call_failed_camera"),
        (calls_group_toasts::GroupCallError::ScreenFailed, "lng_group_call_failed_screen"),
        (calls_group_toasts::GroupCallError::MutedNoCamera, "lng_group_call_muted_no_camera"),
        (calls_group_toasts::GroupCallError::MutedNoScreen, "lng_group_call_muted_no_screen"),
        (calls_group_toasts::GroupCallError::DisabledNoCamera, "lng_group_call_chat_no_camera"),
        (calls_group_toasts::GroupCallError::DisabledNoScreen, "lng_group_call_chat_no_screen"),
    ];

    let mut toasts = CallsGroupToastsGroup::new();
    for (err, key) in errors {
        assert_eq!(err.translation_key(), key);
        toasts.push_error(err);
    }
    assert_eq!(toasts.active_toasts.len(), 7);
    assert_eq!(toasts.active_toasts[0].duration_ms, 2000);
}

#[test]
fn test_group_call_toasts_pinned_video_scenarios() {
    // 4 distinct pinned/unpinned scenarios
    assert_eq!(calls_group_toasts::pinned_video_toast_key(true, true, true), "lng_group_call_pinned_camera_me");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(true, true, false), "lng_group_call_unpinned_camera_me");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(false, true, true), "lng_group_call_pinned_screen_me");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(false, true, false), "lng_group_call_unpinned_screen_me");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(true, false, true), "lng_group_call_pinned_camera");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(true, false, false), "lng_group_call_unpinned_camera");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(false, false, true), "lng_group_call_pinned_screen");
    assert_eq!(calls_group_toasts::pinned_video_toast_key(false, false, false), "lng_group_call_unpinned_screen");
}

#[test]
fn test_viewport_tile_grid_columns_vs_rows_minimization() {
    // 4 tiles in 16:9 aspect ratio container
    let tiles = vec![
        (1, 16, 9),
        (2, 16, 9),
        (3, 16, 9),
        (4, 16, 9),
    ];
    let layout = calls_group_viewport::count_wide(&tiles, 1280, 720);
    assert_eq!(layout.tiles.len(), 4);
    assert_eq!(layout.outer_width, 1280);
    assert_eq!(layout.outer_height, 720);

    // Tiles should have positive dimensions
    for tile in &layout.tiles {
        assert!(tile.rect_columns.width > 0);
        assert!(tile.rect_columns.height > 0);
        assert!(tile.rect_rows.width > 0);
        assert!(tile.rect_rows.height > 0);
    }
}

#[test]
fn test_viewport_single_tile_and_spotlight_geometry() {
    let mut vp = CallsGroupViewportGroup::new();
    vp.set_dimensions(1920, 1080);
    vp.add_tile(42);

    let layout = vp.compute_layout();
    assert_eq!(layout.tiles.len(), 1);
    assert_eq!(layout.tiles[0].rect_rows.width, 1920);
    assert_eq!(layout.tiles[0].rect_rows.height, 1080);

    vp.set_spotlight(Some(42));
    assert_eq!(vp.spotlight_tile_id, Some(42));

    vp.remove_tile(42);
    assert_eq!(vp.active_endpoints.len(), 0);
    assert_eq!(vp.spotlight_tile_id, None);
}

#[test]
fn test_opengl_blur_kernel_and_dither_constants() {
    assert_eq!(calls_group_viewport_opengl::K_BLUR_RADIUS, 15);
    assert_eq!(calls_group_viewport_opengl::K_BLUR_DIAMETER, 31);
    assert_eq!(calls_group_viewport_opengl::K_NOISE_TEXTURE_SIZE, 256);
    assert_eq!(calls_group_viewport_opengl::K_BLUR_OPACITY, 0.65);
    assert_eq!(calls_group_viewport_opengl::K_DITHER_NOISE_AMOUNT, 0.002);

    let mut gl = CallsGroupViewportOpenglGroup::new();
    gl.initialize(1280, 720);
    assert!(gl.is_initialized);
    assert_eq!(gl.allocated_textures, 5);

    let weights = gl.compute_blur_weights();
    assert_eq!(weights.len(), 31);
    let sum: f32 = weights.iter().sum();
    assert!((sum - 1.0).abs() < 1e-4); // normalized to 1.0

    // Center weight is maximum
    assert!(weights[15] > weights[0]);
    assert!(weights[15] > weights[30]);
}
#[test]
fn test_raster_software_fallback_tile_lifecycle() {
    assert_eq!(calls_group_viewport_raster::K_BLUR_RADIUS, 15);

    let mut sw = CallsGroupViewportRasterGroup::new();
    assert_eq!(sw.tile_count(), 0);

    // First paint pass
    sw.begin_paint_pass();
    sw.paint_tile(101, false, true);  // active video
    sw.paint_tile(102, true, true);   // paused video
    sw.paint_tile(103, false, false); // avatar only
    let evicted = sw.end_paint_pass();
    assert_eq!(evicted, 0);
    assert_eq!(sw.tile_count(), 3);

    // Second paint pass: endpoint 103 disappears
    sw.begin_paint_pass();
    sw.paint_tile(101, false, true);
    sw.paint_tile(102, true, true);
    let evicted2 = sw.end_paint_pass();
    assert_eq!(evicted2, 1);
    assert_eq!(sw.tile_count(), 2);
}

#[test]
fn test_rhi_uniform_buffer_sizes_and_vulkan_ndc_flip() {
    assert_eq!(std::mem::size_of::<calls_group_viewport_rhi::GroupFrameUniforms>(), 128);
    assert_eq!(std::mem::size_of::<calls_group_viewport_rhi::BlurUniforms>(), 16);
    assert_eq!(std::mem::size_of::<calls_group_viewport_rhi::ImageUniforms>(), 16);

    // Vulkan has inverted NDC Y
    assert_eq!(calls_group_viewport_rhi::ndc_flip_y(true), -1.0);
    assert_eq!(calls_group_viewport_rhi::ndc_flip_y(false), 1.0);

    let quad_vulkan = calls_group_viewport_rhi::offscreen_quad(true);
    assert_eq!(quad_vulkan.len(), 16);
    let quad_metal = calls_group_viewport_rhi::offscreen_quad(false);
    assert_eq!(quad_metal.len(), 16);

    let mut rhi = CallsGroupViewportRhiGroup::new(calls_group_viewport_rhi::RhiBackend::Vulkan);
    rhi.initialize(1920, 1080);
    assert!(rhi.is_initialized);
    assert_eq!(rhi.frame_uniforms.flip_y, -1.0);
    assert_eq!(rhi.frame_uniforms.viewport, [1920.0, 1080.0]);
}

#[test]
fn test_video_tile_mirroring_and_paused_size() {
    assert_eq!(calls_group_viewport_tile::K_PAUSED_VIDEO_SIZE, 90);

    // Self camera: mirrored
    let tile_self_cam = calls_group_viewport_tile::VideoTileItem::new(1, 100, true, true);
    assert!(tile_self_cam.mirror());
    assert!(!tile_self_cam.is_screencast);

    // Self screen share: not mirrored
    let tile_self_screen = calls_group_viewport_tile::VideoTileItem::new(2, 100, true, false);
    assert!(!tile_self_screen.mirror());
    assert!(tile_self_screen.is_screencast);

    // Peer camera: not mirrored
    let mut tile_peer_cam = calls_group_viewport_tile::VideoTileItem::new(3, 200, false, true);
    assert!(!tile_peer_cam.mirror());

    // Active size vs paused size
    assert_eq!(tile_peer_cam.track_or_userpic_size(1280, 720), (1280, 720));
    tile_peer_cam.is_paused = true;
    assert_eq!(tile_peer_cam.track_or_userpic_size(1280, 720), (90, 90));

    // Quality drops to None when hidden
    tile_peer_cam.is_hidden = true;
    let changed = tile_peer_cam.update_requested_quality(calls_group_viewport_tile::VideoQuality::Full);
    assert!(changed);
    assert_eq!(tile_peer_cam.current_quality, None);
}

#[test]
fn test_volume_slider_magnetic_snap_and_arc_thresholds() {
    assert_eq!(calls_volume_item::K_MAX_VOLUME_PERCENT, 200);

    // Arc energy thresholds: < 5% -> 0, < 25% -> 1, < 75% -> 2, >= 75% -> 3
    assert_eq!(calls_volume_item::speaker_arc_level(0.0), 0);
    assert_eq!(calls_volume_item::speaker_arc_level(0.04), 0);
    assert_eq!(calls_volume_item::speaker_arc_level(0.15), 1);
    assert_eq!(calls_volume_item::speaker_arc_level(0.50), 2);
    assert_eq!(calls_volume_item::speaker_arc_level(0.85), 3);

    // Magnetic snap at 100% (value = 0.500) has +-5% window [0.450..0.550] -> snaps to 0.500
    assert_eq!(calls_volume_item::apply_magnetic_snap(0.48), 0.50);
    assert_eq!(calls_volume_item::apply_magnetic_snap(0.53), 0.50);
    assert_eq!(calls_volume_item::apply_magnetic_snap(0.50), 0.50);

    // Snap at 50% (value = 0.250) has +-1% window [0.240..0.260]
    assert_eq!(calls_volume_item::apply_magnetic_snap(0.245), 0.25);
    assert_eq!(calls_volume_item::apply_magnetic_snap(0.29), 0.29); // outside window

    // Volume controller
    let mut vol = CallsVolumeItemGroup::new(42, 100, false);
    assert_eq!(vol.volume_percent, 100);
    assert_eq!(vol.slider_ratio(), 0.5);
    assert_eq!(vol.speaker_arcs(), 2);

    // Snap near 100% sets exactly 100%
    let snapped_pct = vol.set_slider_ratio(0.49);
    assert_eq!(snapped_pct, 100);
    assert_eq!(vol.volume_percent, 100);

    // Drag to 0 mutes
    vol.set_slider_ratio(0.0);
    assert_eq!(vol.volume_percent, 0);
    assert!(vol.is_locally_muted);
    assert_eq!(vol.speaker_arcs(), 0);
}

#[test]
fn test_recording_box_modes_and_title_length() {
    assert_eq!(calls_group_recording_box::K_ROUND_RADIUS, 9);
    assert_eq!(calls_group_recording_box::K_MAX_GROUP_CALL_LENGTH, 40);

    let mut rec = CallsGroupRecordingBoxGroupUi::new("Valid Title", false);
    assert_eq!(rec.call_title, "Valid Title");
    assert!(!rec.is_recording);

    // Title truncation at 40 characters
    let long_title = "A".repeat(100);
    rec.set_title(&long_title);
    assert_eq!(rec.call_title.len(), 40);

    // Mode switching
    rec.set_recording_type(calls_group_recording_box::RecordingType::VideoLandscape);
    assert_eq!(rec.recording_type, calls_group_recording_box::RecordingType::VideoLandscape);

    // Start / tick / stop
    assert!(rec.start_recording());
    assert!(!rec.start_recording()); // already running
    rec.tick_duration();
    rec.tick_duration();
    assert_eq!(rec.recording_duration_sec, 2);
    let final_dur = rec.stop_recording();
    assert_eq!(final_dur, 2);
    assert!(!rec.is_recording);
}

#[test]
fn test_scheduled_call_labels_date_stages_and_gradient() {
    assert_eq!(calls_group_scheduled_labels::K_DAY_SECONDS, 86400);

    let now = 1700000000;
    // Scheduled in 3 days -> ExactDate
    let stage_far = calls_group_scheduled_labels::compute_scheduled_stage(now + 3 * 86400, now);
    assert!(matches!(stage_far, calls_group_scheduled_labels::ScheduledStage::ExactDate(_)));

    // Scheduled tomorrow (36 hours) -> Tomorrow
    let stage_tomorrow = calls_group_scheduled_labels::compute_scheduled_stage(now + 36 * 3600, now);
    assert!(matches!(stage_tomorrow, calls_group_scheduled_labels::ScheduledStage::Tomorrow(_)));

    // Scheduled today (5 hours) -> Today
    let stage_today = calls_group_scheduled_labels::compute_scheduled_stage(now + 5 * 3600, now);
    assert!(matches!(stage_today, calls_group_scheduled_labels::ScheduledStage::Today(_)));

    // Scheduled in past -> LiveNow
    let stage_live = calls_group_scheduled_labels::compute_scheduled_stage(now - 100, now);
    assert!(matches!(stage_live, calls_group_scheduled_labels::ScheduledStage::LiveNow));

    // Gradient stops verification
    let stops = calls_group_scheduled_labels::K_GRADIENT_STOPS;
    assert_eq!(stops.len(), 3);
    assert_eq!(stops[0].0, 0.0);
    assert_eq!(stops[1].0, 0.7);
    assert_eq!(stops[2].0, 1.0);
}

#[test]
fn test_stars_tier_coloring_and_message_limits() {
    let mut coloring_ctrl = CallsGroupStarsColoringGroupUi::default();
    assert_eq!(coloring_ctrl.tiers.len(), 4);

    // 1 star -> tier 1 (60s pin, 64 chars, 1 emoji)
    let tier1 = coloring_ctrl.set_stars(1).expect("Tier 1 must exist");
    assert_eq!(tier1.from_stars, 1);
    assert_eq!(tier1.seconds_pin, 60);
    assert_eq!(coloring_ctrl.pin_duration_text(), "1m");

    // 15 stars -> tier 2 (from_stars = 10, 300s pin)
    let tier2 = coloring_ctrl.set_stars(15).expect("Tier 2 must exist");
    assert_eq!(tier2.from_stars, 10);
    assert_eq!(coloring_ctrl.pin_duration_text(), "5m");

    // 500 stars -> tier 4 (from_stars = 200, 3600s pin)
    let tier4 = coloring_ctrl.set_stars(500).expect("Tier 4 must exist");
    assert_eq!(tier4.from_stars, 200);
    assert_eq!(coloring_ctrl.pin_duration_text(), "1h");

    // Message limit validation: 200 chars and 4 emojis requires tier 3 (from_stars = 50)
    let needed = coloring_ctrl.stars_needed(200, 4);
    assert_eq!(needed, 50);
}

#[test]
fn test_desktop_capture_source_grid_paging() {
    assert_eq!(desktop_capture_choose_source::K_COLUMNS, 3);
    assert_eq!(desktop_capture_choose_source::K_ROWS, 2);
    assert_eq!(desktop_capture_choose_source::K_PAGE_SIZE, 6);

    let mut picker = DesktopCaptureChooseSourceGroupUi::new(true);
    assert!(picker.with_audio);
    assert!(picker.with_audio_supported);

    // Add 8 sources (spanning 2 pages)
    for i in 1..=8 {
        picker.add_source(
            &format!("screen:{}", i),
            &format!("Display {}", i),
            desktop_capture_choose_source::CaptureSourceType::Screen,
        );
    }

    assert_eq!(picker.total_pages(), 2);
    let page0 = picker.visible_sources_for_page(0);
    assert_eq!(page0.len(), 6);
    let page1 = picker.visible_sources_for_page(1);
    assert_eq!(page1.len(), 2);

    // First source auto-selected
    assert_eq!(picker.selected_device_id.as_deref(), Some("screen:1"));

    // Select source on page 1
    assert!(picker.select_source("screen:7"));
    assert_eq!(picker.selected_device_id.as_deref(), Some("screen:7"));

    // Toggle audio
    assert!(!picker.toggle_with_audio());
    assert!(picker.toggle_with_audio());
}
