//! Subsystem tests for Module 09 Slice 4: Stories Media Viewer
//!
//! Verifies stories playback controller, segmented progress bar, hold-to-pause gesture FSM,
//! stealth mode countdown & formatting, sibling hover scaling, and recent views classification.

use gpui_telegram_media::media::stories::*;

#[test]
fn test_stories_playback_controller_progress_and_completion() {
    let now = 1000;
    let controller = MediaStoriesControllerStories::new(12345, 5, 0, now);

    assert_eq!(controller.current_index, 0);
    assert_eq!(controller.total_stories, 5);
    assert_eq!(controller.pause_state, PauseState::Playing);
    assert!(!controller.marked_read);

    // Initial progress at t=0
    assert_eq!(controller.playback.progress(now), 0.0);
    assert!(!controller.playback.is_finished(now));

    // Mid-playback at t=2500ms (half of 5000ms kPhotoDuration)
    let mid = now + 2500;
    let progress_mid = controller.playback.progress(mid);
    assert!((progress_mid - 0.5).abs() < 1e-4);
    assert!(!controller.playback.is_finished(mid));

    // Completed at t=5000ms
    let end = now + 5000;
    assert_eq!(controller.playback.progress(end), 1.0);
    assert!(controller.playback.is_finished(end));

    // Beyond completion clamps at 1.0
    let over = now + 7000;
    assert_eq!(controller.playback.progress(over), 1.0);
}

#[test]
fn test_hold_to_pause_gesture_fsm() {
    let start = 10_000;
    let mut controller = MediaStoriesControllerStories::new(99, 3, 0, start);

    // Play for 1000ms
    let t1 = start + 1000;
    assert_eq!(controller.playback.elapsed(t1), 1000);

    // User presses and holds content stage at t1 (t=11000)
    controller.content_pressed(true, t1);
    assert_eq!(controller.pause_state, PauseState::Paused);
    assert!(controller.playback.paused_at.is_some());

    // While held for 3000ms (until t=14000), elapsed time remains frozen at 1000ms
    let t2 = t1 + 3000;
    assert_eq!(controller.playback.elapsed(t2), 1000);

    // User releases hold at t2
    controller.content_pressed(false, t2);
    assert_eq!(controller.pause_state, PauseState::Playing);
    assert!(controller.playback.paused_at.is_none());

    // Play resumes; at t=16000 (2000ms after release), elapsed is 1000 + 2000 = 3000ms
    let t3 = t2 + 2000;
    assert_eq!(controller.playback.elapsed(t3), 3000);
    assert!((controller.playback.progress(t3) - 0.6).abs() < 1e-4);
}

#[test]
fn test_segmented_progress_bar_arithmetic() {
    let mut slider = MediaStoriesSliderStories::new(4, 1);
    slider.set_progress(0.5);

    let container_width = 320.0;
    let segments = slider.compute_layout(container_width);

    assert_eq!(segments.len(), 4);

    // Total width = 4 * one + 3 * skip = container_width
    let skip = slider.skip_gap;
    let total_width = segments.iter().map(|s| s.width).sum::<f32>() + 3.0 * skip;
    assert!((total_width - container_width).abs() < 1e-3);

    // First segment (index 0 < current 1): completed, opacity 1.0, full active width
    assert_eq!(segments[0].opacity, K_OPACITY_ACTIVE);
    assert!(!segments[0].is_active);
    assert_eq!(segments[0].active_fill_width, segments[0].width);

    // Second segment (index 1 == current 1): active, 50% filled
    assert!(segments[1].is_active);
    let expected_fill = segments[1].width * 0.5;
    assert!((segments[1].active_fill_width - expected_fill).abs() < 1e-3);

    // Subsequent segments (index 2, 3 > current 1): unplayed, opacity 0.4, 0.0 fill
    assert_eq!(segments[2].opacity, K_OPACITY_INACTIVE);
    assert!(!segments[2].is_active);
    assert_eq!(segments[2].active_fill_width, 0.0);

    assert_eq!(segments[3].opacity, K_OPACITY_INACTIVE);
    assert!(!segments[3].is_active);
    assert_eq!(segments[3].active_fill_width, 0.0);
}

#[test]
fn test_stealth_mode_countdown_and_time_format() {
    // Exact C++ formatting test: seconds -> 0:ss, m:ss, h:mm:ss
    assert_eq!(time_left_text(5), "0:05");
    assert_eq!(time_left_text(45), "0:45");
    assert_eq!(time_left_text(60), "1:00");
    assert_eq!(time_left_text(75), "1:15");
    assert_eq!(time_left_text(3599), "59:59");
    assert_eq!(time_left_text(3600), "1:00:00");
    assert_eq!(time_left_text(3665), "1:01:05");
    assert_eq!(time_left_text(7325), "2:02:05");

    // Stealth mode state machine test
    let now = 100_000;
    let state = StealthModeState {
        enabled_till: now + 300,  // Active for 5 minutes
        cooldown_till: now + 1500, // Cooldown for 25 minutes
        now,
        premium: true,
        has_callback: false,
    };

    assert!(state.is_active());
    assert_eq!(state.time_left(), 300);
    assert_eq!(time_left_text(state.time_left()), "5:00");
    assert!(!state.lock_visible());

    // After active period expired, enters cooldown
    let cooldown_state = StealthModeState {
        enabled_till: now - 10,
        cooldown_till: now + 1200,
        now,
        premium: true,
        has_callback: false,
    };
    assert!(!cooldown_state.is_active());
    assert!(cooldown_state.is_cooldown());
    assert_eq!(cooldown_state.button_opacity(), K_COOLDOWN_BUTTON_LABEL_OPACITY);
}

#[test]
fn test_sibling_perspective_and_hover_scaling() {
    let mut sibling = MediaStoriesSiblingStories::new(777, 42, "Alice".to_string());

    // Rest state: over = 0.0
    sibling.set_hover(0.0);
    let view_rest = sibling.compute_view();
    assert_eq!(view_rest.fade, K_SIBLING_FADE);
    assert_eq!(view_rest.name_opacity, K_SIBLING_NAME_OPACITY);
    assert_eq!(view_rest.scale, 1.0);

    // Full hover: over = 1.0 (perspective scale up to 1.05x, fade 0.4, opacity 1.0)
    sibling.set_hover(1.0);
    let view_hover = sibling.compute_view();
    assert_eq!(view_hover.fade, K_SIBLING_FADE_OVER);
    assert_eq!(view_hover.name_opacity, K_SIBLING_NAME_OPACITY_OVER);
    assert!((view_hover.scale - 1.05).abs() < 1e-4);

    // Mid-hover: over = 0.5
    sibling.set_hover(0.5);
    let view_mid = sibling.compute_view();
    assert!((view_mid.fade - 0.45).abs() < 1e-4);
    assert!((view_mid.name_opacity - 0.9).abs() < 1e-4);
    assert!((view_mid.scale - 1.025).abs() < 1e-4);
}

#[test]
fn test_recent_views_type_classification_and_admin_permissions() {
    // Video stream always classifies as Other
    assert_eq!(
        recent_views_type_for(true, false, false, false),
        RecentViewsType::Other
    );

    // Self stories
    assert_eq!(
        recent_views_type_for(false, true, false, false),
        RecentViewsType::SelfStory
    );

    // Channel broadcasts
    assert_eq!(
        recent_views_type_for(false, false, true, false),
        RecentViewsType::Channel
    );

    // Telegram service/changelog notifications
    assert_eq!(
        recent_views_type_for(false, false, false, true),
        RecentViewsType::Changelog
    );

    // Standard peer stories
    assert_eq!(
        recent_views_type_for(false, false, false, false),
        RecentViewsType::Other
    );

    // Reaction visibility permissions for channels:
    // Only creators or administrators may view detailed reactions
    assert!(can_view_reactions_for(true, true, false));   // Creator
    assert!(can_view_reactions_for(true, false, true));   // Admin
    assert!(!can_view_reactions_for(true, false, false)); // Regular subscriber
    assert!(!can_view_reactions_for(false, true, true));  // Non-channel ignores channel admin logic
}


#[test]
fn test_share_at_time_formatter_and_deep_links() {
    assert_eq!(format_share_at_time(0), "0:00");
    assert_eq!(format_share_at_time(9), "0:09");
    assert_eq!(format_share_at_time(75), "1:15");
    assert_eq!(format_share_at_time(3600), "1:00:00");
    assert_eq!(format_share_at_time(3665), "1:01:05");

    let config = ShareStoryConfig {
        peer_id: 123456,
        story_id: 88,
        has_direct_link: true,
        is_call_story: false,
        video_timestamp: Some(45),
    };
    let share = MediaStoriesShareStories::new(config);

    // Formatted timestamp
    assert_eq!(share.config.formatted_timestamp(), Some("0:45".to_string()));

    // Story deep link with timestamp
    let link = share.generate_story_link(Some("durov")).unwrap();
    assert_eq!(link, "https://t.me/durov/s/88?t=45");

    // Comment count: 1 if empty, 2 if has comment
    assert_eq!(share.config.count_messages_for_comment(""), 1);
    assert_eq!(share.config.count_messages_for_comment("Check this out!"), 2);
}

#[test]
fn test_repost_view_caption_geometry() {
    let metrics = RepostLayoutMetrics {
        padding_top: 4,
        padding_bottom: 4,
        semibold_font_height: 16,
        normal_font_height: 14,
    };

    assert_eq!(metrics.full_height(), 38);
    assert_eq!(metrics.simple_height(), 24);

    let full_repost = MediaStoriesRepostViewStories::new(RepostSourceInfo {
        source_peer_id: 100,
        source_post_id: Some(501),
        source_name: "Channel News".to_string(),
        text: "Breaking updates".to_string(),
    });
    assert!(!full_repost.is_simple());
    assert_eq!(full_repost.height(), 38);

    let simple_repost = MediaStoriesRepostViewStories::new(RepostSourceInfo {
        source_peer_id: 100,
        source_post_id: Some(502),
        source_name: "Channel News".to_string(),
        text: "   ".to_string(), // Whitespace only is treated as simple
    });
    assert!(simple_repost.is_simple());
    assert_eq!(simple_repost.height(), 24);

    assert_eq!(full_repost.quote_bg_rgba(), (0, 0, 0, 64));
}

#[test]
fn test_caption_full_view_margin_skip_and_lifecycle() {
    // Skip calculation: (header_y + header_height - content_y)
    let skip = compute_caption_top_skip(10, 50, 20);
    assert_eq!(skip, 40); // 10 + 50 - 20 = 40

    let skip_clamped = compute_caption_top_skip(0, 30, 50);
    assert_eq!(skip_clamped, 0); // Negative clamped to 0

    let mut caption_view = MediaStoriesCaptionFullViewStories::new(
        "A long and detailed story caption...".to_string(),
        10,
        50,
        20,
    );
    assert!(caption_view.is_visible());
    assert!(!caption_view.is_closing());
    assert_eq!(caption_view.top_skip, 40);

    caption_view.request_close();
    assert!(caption_view.is_closing());

    caption_view.state.finish_closing();
    assert!(!caption_view.is_visible());
    assert!(!caption_view.is_closing());
}

#[test]
fn test_story_interactive_reaction_sticker_geometry() {
    let geom_no_count = ReactionStickerGeometry::for_size(100.0, false);
    assert_eq!(geom_no_count.base_size, 100.0);
    assert!((geom_no_count.tail_big_radius - 26.4).abs() < 1e-4);
    assert!((geom_no_count.tail_big_offset - 46.4).abs() < 1e-4);
    assert!((geom_no_count.tail_big_deg - -42.29).abs() < 1e-4);
    assert!((geom_no_count.tail_small_radius - 11.0).abs() < 1e-4);
    assert!((geom_no_count.tail_small_offset - 69.7).abs() < 1e-4);
    assert!((geom_no_count.tail_small_deg - -40.87).abs() < 1e-4);
    assert!((geom_no_count.icon_size - 70.0).abs() < 1e-4); // K_SUGGESTED_REACTION_SIZE = 0.7

    let geom_with_count = ReactionStickerGeometry::for_size(100.0, true);
    assert!((geom_with_count.icon_size - 55.0).abs() < 1e-4); // K_SUGGESTED_WITH_COUNT_SIZE = 0.55
}

#[test]
fn test_story_header_privacy_and_volume_controls() {
    let header_public = MediaStoriesHeaderStories::new(HeaderData {
        peer_id: 1,
        peer_name: "Channel".to_string(),
        date: 1700000000,
        full_index: 0,
        full_count: 5,
        privacy: StoryPrivacyLevel::Public,
        edited: false,
        is_video: true,
        is_video_stream: false,
        is_silent: false,
    });
    assert!(!header_public.has_privacy_badge());

    let mut header_private = MediaStoriesHeaderStories::new(HeaderData {
        peer_id: 2,
        peer_name: "Friend".to_string(),
        date: 1700000000,
        full_index: 1,
        full_count: 5,
        privacy: StoryPrivacyLevel::CloseFriends,
        edited: true,
        is_video: true,
        is_video_stream: false,
        is_silent: false,
    });
    assert!(header_private.has_privacy_badge());

    // Volume and mute controls
    assert_eq!(header_private.volume, 1.0);
    assert!(!header_private.is_muted);

    header_private.set_volume(0.5);
    assert_eq!(header_private.volume, 0.5);
    assert!(!header_private.is_muted);

    header_private.set_volume(0.0);
    assert!(header_private.is_muted);

    header_private.set_volume(1.5); // Clamps to 1.0
    assert_eq!(header_private.volume, 1.0);
    assert!(!header_private.is_muted);

    assert!(header_private.toggle_mute());
    assert!(header_private.is_muted);
}

#[test]
fn test_reply_bar_placeholder_and_stealth_awareness() {
    // 1. In stealth mode with time remaining
    let placeholder_stealth = determine_reply_placeholder(ReplyAreaType::Reply, 180, 0);
    assert!(placeholder_stealth.contains("anonymously"));
    assert!(placeholder_stealth.contains("180 left"));

    // 2. Video stream comment with stars requirement
    let placeholder_video_paid = determine_reply_placeholder(ReplyAreaType::VideoStreamComment, 0, 50);
    assert_eq!(placeholder_video_paid, "Comment (50 stars)");

    // 3. Regular comment with stars requirement
    let placeholder_story_paid = determine_reply_placeholder(ReplyAreaType::Comment, 0, 25);
    assert_eq!(placeholder_story_paid, "Send message (25 stars)");

    // 4. Free comment on channel story
    let placeholder_comment = determine_reply_placeholder(ReplyAreaType::Comment, 0, 0);
    assert_eq!(placeholder_comment, "Comment on story...");

    // 5. Free standard direct reply to user story
    let placeholder_standard = determine_reply_placeholder(ReplyAreaType::Reply, 0, 0);
    assert_eq!(placeholder_standard, "Send a message...");
}
