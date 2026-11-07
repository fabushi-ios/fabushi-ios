//! Subsystem tests for Module 09 Slice 3: Media Lightbox Viewer, Overlay Controls, Zoom & Navigation
//!
//! Verifies open request dispatcher, video timestamp extraction, quote entity stripping,
//! snap_xy coordinate centering and boundary clamping, continuous zoom matrix, and swipe gesture threshold.

use gpui_telegram_media::media::view::media_view_open_common::*;
use gpui_telegram_media::media::view::media_view_overlay_widget::*;
use gpui_telegram_media::media::view::media_view_playback_controls::*;
use gpui_telegram_media::media::view::media_view_playback_progress::*;
use gpui_telegram_media::media::view::media_view_playback_sponsored::*;
use gpui_telegram_media::media::view::media_view_group_thumbs::*;
use gpui_telegram_media::media::view::media_view_video_stream::*;
use gpui_telegram_media::media::view::media_view_pip::*;
use gpui_telegram_media::media::view::media_view_metal_texture::*;
use gpui_telegram_media::media::view::media_view_recognition_selection::*;

#[test]
fn test_open_request_photo_and_document() {
    let photo_req = OpenRequest::for_photo(1001, Some(2001));
    assert_eq!(photo_req.item_id, Some(2001));
    assert!(photo_req.show_draw_button);
    assert!(!photo_req.continue_streaming);
    match photo_req.target {
        OpenTarget::Photo { photo_id, peer_id } => {
            assert_eq!(photo_id, 1001);
            assert_eq!(peer_id, None);
        }
        _ => panic!("Expected OpenTarget::Photo"),
    }

    let doc_req = OpenRequest::for_document(5001, Some(2002), true, 15000);
    assert_eq!(doc_req.item_id, Some(2002));
    assert!(!doc_req.show_draw_button);
    assert!(doc_req.continue_streaming);
    assert_eq!(doc_req.start_time, 15000);
    match doc_req.target {
        OpenTarget::Document { document_id, cloud_theme } => {
            assert_eq!(document_id, 5001);
            assert_eq!(cloud_theme, None);
        }
        _ => panic!("Expected OpenTarget::Document"),
    }
}

#[test]
fn test_extract_video_timestamp() {
    // Media timestamp takes precedence
    assert_eq!(extract_video_timestamp(Some(42), Some(10)), 42);
    // Webpage timestamp fallback when media timestamp is 0 or None
    assert_eq!(extract_video_timestamp(None, Some(10)), 10);
    assert_eq!(extract_video_timestamp(Some(0), Some(15)), 15);
    // Returns 0 when neither present
    assert_eq!(extract_video_timestamp(None, None), 0);
    assert_eq!(extract_video_timestamp(Some(0), Some(0)), 0);
}

#[test]
fn test_strip_quote_entities() {
    let entities = vec![
        EntityInText {
            entity_type: EntityType::Bold,
            offset: 0,
            length: 5,
        },
        EntityInText {
            entity_type: EntityType::Blockquote,
            offset: 6,
            length: 20,
        },
        EntityInText {
            entity_type: EntityType::Pre,
            offset: 27,
            length: 15,
        },
    ];

    let stripped = strip_quote_entities(&entities);
    assert_eq!(stripped.len(), 2);
    assert_eq!(stripped[0].entity_type, EntityType::Bold);
    assert_eq!(stripped[1].entity_type, EntityType::Code); // Pre converted to Code
    assert_eq!(stripped[1].offset, 27);
    assert_eq!(stripped[1].length, 15);
}

#[test]
fn test_overlay_snap_xy_centered_and_clamped() {
    // 1. Content smaller than viewport: centers exactly
    let (cx, cy) = snap_xy(1280, 800, 800, 600, 0, 800, 0, 800, false, 0, 0);
    assert_eq!(cx, (1280 - 800) / 2); // 240
    assert_eq!(cy, (800 - 600) / 2);  // 100

    // 2. Content larger than viewport: allows panning between (viewport - content) and 0
    let (px, py) = snap_xy(1280, 800, 1600, 1000, 0, 800, 0, 800, false, -100, -50);
    assert_eq!(px, -100);
    assert_eq!(py, -50);

    // Overflow beyond bounds clamps to min and max boundaries
    let (min_x, _) = snap_xy(1280, 800, 1600, 1000, 0, 800, 0, 800, false, -500, 0);
    assert_eq!(min_x, 1280 - 1600); // clamped to -320

    let (max_x, _) = snap_xy(1280, 800, 1600, 1000, 0, 800, 0, 800, false, 200, 0);
    assert_eq!(max_x, 0); // clamped to 0
}

#[test]
fn test_overlay_zoom_matrix_in_and_out() {
    let mut view = MediaViewOverlayWidgetView::new();
    assert_eq!(view.zoom, 0);
    assert_eq!(view.content_w, MEDIAVIEW_DEFAULT_WIDTH); // 800
    assert_eq!(view.content_h, MEDIAVIEW_DEFAULT_HEIGHT); // 600

    // Zoom in step to zoom=1 (scale = 1 + 1 = 2x)
    view.zoom_in(None);
    assert_eq!(view.zoom, 1);
    assert_eq!(view.content_w, 1600);
    assert_eq!(view.content_h, 1200);

    // Zoom in step to zoom=2 (scale = 2 + 1 = 3x)
    view.zoom_in(None);
    assert_eq!(view.zoom, 2);
    assert_eq!(view.content_w, 2400);
    assert_eq!(view.content_h, 1800);

    // Zoom out steps back
    view.zoom_out(None);
    assert_eq!(view.zoom, 1);
    view.zoom_out(None);
    assert_eq!(view.zoom, 0);
    assert_eq!(view.content_w, 800);
    assert_eq!(view.content_h, 600);

    // Zoom out step below 0 to zoom=-1 (scale = 1 / (-(-1) + 1) = 1/2)
    view.zoom_out(None);
    assert_eq!(view.zoom, -1);
    assert_eq!(view.content_w, 400);
    assert_eq!(view.content_h, 300);
}

#[test]
fn test_overlay_swipe_navigation_threshold() {
    let mut view = MediaViewOverlayWidgetView::new();
    view.total_items = 5;
    view.current_index = 2;

    // Below threshold (80px): no navigation
    assert_eq!(view.handle_swipe(50), None);
    assert_eq!(view.current_index, 2);

    // Forward swipe (>= 80px): moves to index 3
    assert_eq!(view.handle_swipe(85), Some(true));
    assert_eq!(view.current_index, 3);

    // Backward swipe (<=-80px): moves to index 2
    assert_eq!(view.handle_swipe(-100), Some(false));
    assert_eq!(view.current_index, 2);

    // Clamping at boundaries
    view.current_index = 0;
    assert_eq!(view.handle_swipe(-90), None);
    assert_eq!(view.current_index, 0);
}

#[test]
fn test_playback_controls_seek_and_volume() {
    let mut controls = MediaViewPlaybackControlsView::new();
    controls.duration_ms = 60_000; // 60s
    controls.ready_bytes = 10_000_000;
    controls.total_bytes = 20_000_000;
    controls.file_header_size = 1_000_000;

    // Seek progress clamps to [0, duration]
    assert_eq!(controls.handle_seek_progress(0.5), 30_000);
    assert_eq!(controls.seek_position_ms, Some(30_000));
    assert_eq!(controls.handle_seek_finished(0.75), 45_000);
    assert_eq!(controls.seek_position_ms, None);
    assert_eq!(controls.position_ms, 45_000);

    // Download buffer calculation
    // (10MB - 1MB) / (20MB - 1MB) = 9 / 19 ≈ 0.47368
    let pct = controls.count_downloaded_till_percent();
    assert!((pct - (9.0 / 19.0)).abs() < 1e-4);

    // Volume level categorization
    controls.set_volume(0.0);
    assert_eq!(controls.volume_level(), VolumeLevel::Muted);
    controls.set_volume(0.35);
    assert_eq!(controls.volume_level(), VolumeLevel::Low);
    controls.set_volume(0.8);
    assert_eq!(controls.volume_level(), VolumeLevel::High);
}

#[test]
fn test_playback_progress_animation_extrapolation() {
    let mut progress = MediaViewPlaybackProgressView::new();
    // position = 10_000, length = 100_000, frequency = 1000
    // animated_position = 10_000 + (1000 * 200 / 1000) = 10_200
    progress.update_state(10_000, 100_000, 1000, 15_000, 0.25);

    assert!(progress.is_playing);
    assert!((progress.value - 0.102).abs() < 1e-4);
    assert!((progress.available_till - 0.25).abs() < 1e-4); // max(15000/100000, 0.25) = 0.25
}

#[test]
fn test_playback_sponsored_ad_timer_and_dismiss() {
    let mut ad = MediaViewPlaybackSponsoredView::with_ad(SponsoredAdData {
        title: "Sponsored Channel".into(),
        text: "Join for exclusive news".into(),
        sponsor_url: "https://t.me/news".into(),
        about_url: None,
    });

    assert_eq!(ad.state, SponsoredState::Pending);
    assert_eq!(ad.countdown_seconds(), 6); // 1000ms delay + 5000ms duration

    // Tick 1200ms -> Showing
    ad.tick(1200);
    assert_eq!(ad.state, SponsoredState::Showing);
    assert!(!ad.dismiss()); // cannot dismiss yet

    // Tick remaining 4800ms -> AllowClose
    ad.tick(4800);
    assert_eq!(ad.state, SponsoredState::AllowClose);
    assert_eq!(ad.countdown_seconds(), 0);
    assert!(ad.dismiss());
    assert_eq!(ad.state, SponsoredState::Dismissed);
}

#[test]
fn test_group_thumbs_layout_and_hit_test() {
    let mut thumbs = MediaViewGroupThumbsView::new();
    let keys = vec![
        GroupThumbKey::Photo(101),
        GroupThumbKey::Photo(102),
        GroupThumbKey::Photo(103),
    ];

    thumbs.refresh(&keys, 1, 800); // middle item active
    assert_eq!(thumbs.item_count(), 3);
    assert!(!thumbs.items[0].is_current);
    assert!(thumbs.items[1].is_current);
    assert!(!thumbs.items[2].is_current);

    // Active item has expanded width (112px) vs inactive (56px)
    assert_eq!(thumbs.items[0].current_width, MEDIAVIEW_GROUP_WIDTH);
    assert_eq!(thumbs.items[1].current_width, MEDIAVIEW_GROUP_WIDTH * 2);

    // Hit test matches correct thumb key
    let hit_mid = thumbs.items[1].final_left + 10;
    assert_eq!(thumbs.hit_test(hit_mid), Some(GroupThumbKey::Photo(102)));
}

#[test]
fn test_pip_edge_snapping_and_resize() {
    // Clamping within snap area (20px of 12px margin)
    // screen: 1920x1080. Right edge = 1920 - 12 = 1908
    // inner: w=320, h=180. if x=1580, right=1900 (within 20px of 1908) -> snaps right to 1908-320 = 1588
    let (sx, sy) = clamp_to_edges(0, 0, 1920, 1080, 1580, 100, 320, 180);
    assert_eq!(sx, 1920 - PIP_BORDER_SKIP - 320); // 1588
    assert_eq!(sy, 100);

    // Resize via BottomRight corner
    let mut pip = MediaViewPipView::new();
    pip.x = 200;
    pip.y = 200;
    pip.width = 320;
    pip.height = 180;

    pip.drag_resize(50, 30, RectPart::BottomRight);
    assert_eq!(pip.width, 370);
    assert_eq!(pip.height, 210);
    assert_eq!(pip.x, 200);
    assert_eq!(pip.y, 200);
}

#[test]
fn test_recognition_reading_order_and_text_assembly() {
    let items = vec![
        TextRect { x: 100, y: 50, width: 80, height: 20, text: "World".into() },
        TextRect { x: 10, y: 52, width: 80, height: 20, text: "Hello".into() },
        TextRect { x: 10, y: 90, width: 120, height: 20, text: "Telegram".into() },
    ];

    let order = compute_reading_order(&items);
    assert_eq!(order, vec![1, 0, 2]); // "Hello", then "World", then next row "Telegram"

    let full_text = recognized_text(&items);
    assert_eq!(full_text, "Hello World\nTelegram");
}

#[test]
fn test_metal_texture_cache_formats() {
    let mut cache = MediaViewMetalTextureView::new();
    assert!(cache.is_flushed);

    // Standard video range 420v
    let ok = cache.create_textures_from_pixel_buffer(K_CV_PIXEL_FORMAT_420_VIDEO_RANGE, 1920, 1080);
    assert!(ok);
    assert_eq!(cache.format, MetalPixelFormat::BiPlanarVideoRange);
    assert!(cache.has_y_texture);
    assert!(cache.has_uv_texture);
    assert!(!cache.is_flushed);

    // Flush cache
    cache.flush();
    assert!(cache.is_flushed);
    assert!(!cache.has_y_texture);

    // Unsupported format returns false
    let bad = cache.create_textures_from_pixel_buffer(0x12345678, 800, 600);
    assert!(!bad);
}

#[test]
fn test_video_stream_initialization_and_donors() {
    let mut stream = MediaViewVideoStreamView::with_call("live-slug-1234", 9999);
    assert!(stream.is_active);
    assert_eq!(stream.call_link_slug, "live-slug-1234");
    assert_eq!(stream.join_message_id, 9999);
    assert_eq!(stream.volume, 1.0);
    assert!(stream.comments_shown);

    stream.set_volume(0.65);
    assert_eq!(stream.volume, 0.65);

    let shown = stream.toggle_comments();
    assert!(!shown);
    assert!(!stream.comments_shown);

    stream.set_geometry(10, 20, 1920, 1080);
    assert_eq!(stream.x, 10);
    assert_eq!(stream.y, 20);
    assert_eq!(stream.width, 1920);
    assert_eq!(stream.height, 1080);

    let donors = vec![
        MessageReactionTopPaid {
            peer_id: 111,
            count: 50,
            is_my: false,
        },
        MessageReactionTopPaid {
            peer_id: 222,
            count: 100,
            is_my: true,
        },
    ];
    stream.update_donors(donors, 5);
    assert_eq!(stream.donors.len(), 2);
    assert_eq!(stream.donors[0].count, 100);
    assert_eq!(stream.donors[1].count, 50);
}
