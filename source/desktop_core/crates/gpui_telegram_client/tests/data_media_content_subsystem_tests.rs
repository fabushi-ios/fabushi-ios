#![allow(unused_imports)]
use gpui_telegram_client::data::{
    business::*,
    notify::*,
    stickers::*,
    data_document::*,
    data_document_resolver::*,
    data_document_media::*,
    data_photo::*,
    data_photo_media::*,
    data_forum::*,
    data_forum_topic::*,
    data_forum_icons::*,
    data_thread::*,
    data_saved_messages::*,
    data_saved_sublist::*,
    data_saved_music::*,
    data_replies_list::*,
    data_reply_preview::*,
    data_shared_media::*,
    data_stories::*,
    data_stories_ids::*,
    data_story::*,
    data_message_reaction_id::*,
    data_message_reactions::*,
    data_star_gift::*,
    data_poll::*,
    data_poll_messages::*,
    data_web_page::*,
    data_wall_paper::*,
    data_drafts::*,
    data_emoji_statuses::*,
    data_game::*,
    data_group_call::*,
    data_location::*,
    data_send_action::*,
    data_statistics::*,
    data_statistics_chart::*,
    data_statistics_lists::*,
    data_todo_list::*,
    data_ai_compose_tones::*,
    data_bot_app::*,
    data_cloud_file::*,
    data_compose_stash::*,
    data_media_types::*,
    data_media_preload::*,
    data_media_rotation::*,
    data_download_manager::*,
    data_auto_download::*,
    data_file_origin::*,
    data_file_click_handler::*,
    data_streaming::*,
    data_audio_msg_id::*,
};

#[test]
fn test_document_mime_and_type_detection() {
    let resolver = DataDocumentResolver::new();
    assert_eq!(resolver.resolve_mime("song.mp3"), "audio/mpeg");
    assert_eq!(resolver.resolve_mime("voice.ogg"), "audio/ogg");
    assert_eq!(resolver.resolve_mime("clip.mp4"), "video/mp4");
    assert_eq!(resolver.resolve_mime("sticker.webp"), "image/webp");
    assert_eq!(resolver.resolve_mime("doc.pdf"), "application/pdf");
    assert_eq!(resolver.resolve_mime("unknown.xyz"), "application/octet-stream");

    let doc = DataDocument::new(1001, 1048576, "audio/ogg")
        .with_filename("voice_message.ogg")
        .with_voice(VoiceInfo {
            waveform: vec![0, 5, 10, 25, 31, 15, 2],
            duration_seconds: 12,
        });

    assert_eq!(doc.doc_type, DocumentType::Voice);
    assert!(doc.is_voice_message());
    assert!(!doc.is_song());
    assert!(doc.supports_streaming());

    let mut doc_loading = doc.clone();
    doc_loading.set_loading(0.45, 471859);
    assert!((doc_loading.progress() - 0.45).abs() < 1e-4);

    doc_loading.set_ready();
    assert_eq!(doc_loading.progress(), 1.0);
}

#[test]
fn test_photo_progressive_sizes_and_dimensions() {
    let mut photo = DataPhoto::new(2001, 1700000000)
        .with_remote(2, 987654321, vec![1, 2, 3, 4]);

    assert!(!photo.is_null());
    assert_eq!(photo.width(), 0);
    assert_eq!(photo.height(), 0);

    photo.set_dimensions(PhotoSize::Small, 100, 75, 4096);
    photo.set_dimensions(PhotoSize::Thumbnail, 320, 240, 16384);
    photo.set_dimensions(PhotoSize::Large, 1280, 960, 150000);

    assert!(photo.has_exact(PhotoSize::Small));
    assert!(photo.has_exact(PhotoSize::Thumbnail));
    assert!(photo.has_exact(PhotoSize::Large));

    assert_eq!(photo.width(), 1280);
    assert_eq!(photo.height(), 960);
    assert_eq!(photo.size_dimensions(PhotoSize::Small), Some((100, 75)));

    photo.set_video(500, Some(15));
    assert!(photo.has_video);
    assert_eq!(photo.video_start_ts, 500);
    assert_eq!(photo.video_duration_seconds, Some(15));
}

#[test]
fn test_forum_topic_creation_and_counters() {
    let general = DataForumTopic::general(10001);
    assert!(general.is_general());
    assert_eq!(general.root_id, FORUM_GENERAL_TOPIC_ID);
    assert_eq!(general.title, "General");

    let mut custom_topic = DataForumTopic::new(10001, 42, "Rust Architecture")
        .with_icon(999888, 2);

    assert!(!custom_topic.is_general());
    assert_eq!(custom_topic.color_id, 2);

    let color_rgb = DataForumIcons::resolve_color(custom_topic.color_id);
    assert_eq!(color_rgb, 0xCB86DB); // Purple index 2

    // Apply new message
    custom_topic.apply_new_message(101, true, true, false);
    assert_eq!(custom_topic.unread_count, 1);
    assert_eq!(custom_topic.unread_mentions_count, 1);
    assert_eq!(custom_topic.last_message_id, 101);

    custom_topic.read_till_end();
    assert_eq!(custom_topic.unread_count, 0);
    assert_eq!(custom_topic.unread_mentions_count, 0);

    // Forum aggregator
    let mut forum = DataForum::new(10001);
    forum.apply_topic_added(ForumTopicSummary {
        root_id: 1,
        title: "General".to_string(),
        icon_id: 0,
        color_id: 0,
        is_closed: false,
        is_pinned: true,
        is_hidden: false,
        unread_count: 0,
        last_message_date: 100,
    });
    forum.apply_topic_added(ForumTopicSummary {
        root_id: 42,
        title: "Rust Architecture".to_string(),
        icon_id: 999888,
        color_id: 2,
        is_closed: false,
        is_pinned: false,
        is_hidden: false,
        unread_count: 3,
        last_message_date: 200,
    });

    assert_eq!(forum.total_unread_topics, 1);
    assert_eq!(forum.ordered_topic_ids[0], 1); // Pinned topic first
    assert_eq!(forum.ordered_topic_ids[1], 42);

    forum.clear_all_unreads();
    assert_eq!(forum.total_unread_topics, 0);
}

#[test]
fn test_story_lifecycle_and_expiration() {
    let now = 1700000000;
    let story = DataStory::new(501, 777, now, 24)
        .with_caption("Teamwork GPUI milestone release!")
        .with_privacy(StoryPrivacy::CloseFriends)
        .with_photo(2001);

    assert_eq!(story.privacy, StoryPrivacy::CloseFriends);
    assert_eq!(story.expires, now + 86400);
    assert!(!story.is_expired(now + 3600)); // 1 hour later: still active
    assert!(story.is_expired(now + 86400)); // 24 hours later: expired
    assert!(story.is_expired(now + 100000)); // Past expiry

    let mut peer_stories = DataStories::new(777);
    peer_stories.set_active_stories(vec![501, 502, 503], 501);
    assert!(peer_stories.has_unseen_stories());
    assert_eq!(peer_stories.unread_count(), 2);

    peer_stories.mark_read(503);
    assert!(!peer_stories.has_unseen_stories());
    assert_eq!(peer_stories.unread_count(), 0);
}

#[test]
fn test_poll_voting_and_results_tally() {
    let mut poll = DataPoll::new(9001, "What is the best UI framework for Rust?");
    poll.add_answer(vec![1], "GPUI", true);
    poll.add_answer(vec![2], "Slint", false);
    poll.add_answer(vec![3], "Iced", false);
    poll.is_multi_choice = false;
    poll.is_quiz = true;

    assert!(!poll.voted());
    assert_eq!(poll.total_voters, 0);

    // Single choice constraint
    let err = poll.vote(&[vec![1], vec![2]]);
    assert!(err.is_err());

    // Valid vote
    let ok = poll.vote(&[vec![1]]);
    assert!(ok.is_ok());
    assert!(poll.voted());
    assert_eq!(poll.total_voters, 1);
    assert_eq!(poll.percentage_for(&[1]), 100.0);
    assert_eq!(poll.percentage_for(&[2]), 0.0);

    // Close poll
    poll.close();
    assert!(poll.is_closed);
    assert!(poll.vote(&[vec![2]]).is_err());
}

#[test]
fn test_star_gifts_and_rarity() {
    let standard = DataStarGift::standard(801, 100, 7001);
    assert_eq!(standard.stars_cost, 100);
    assert_eq!(standard.stars_converted, 85);
    assert!(!standard.is_limited());

    let limited = DataStarGift::limited(802, 500, 7002, 10000);
    assert!(limited.is_limited());
    assert_eq!(limited.limited_total, Some(10000));

    let unique = UniqueGift {
        collectible_id: 9999,
        slug: "dragon-egg".to_string(),
        title: "Golden Dragon Egg".to_string(),
        number: 42,
        rarity: UniqueGiftRarity::Legendary,
        model_document_id: 7003,
        owner_address: Some("EQB_abc123".to_string()),
        resale_stars: Some(25000),
        resale_ton: Some(150),
        can_transfer_at: 1710000000,
    };

    let gift_collectible = limited.with_unique(unique);
    assert!(gift_collectible.is_collectible());
    assert_eq!(
        DataStarGift::rarity_name(gift_collectible.unique_gift.as_ref().unwrap().rarity),
        "Legendary"
    );
}

#[test]
fn test_business_chatbots_and_shortcuts() {
    let mut chatbots = DataBusinessChatbotsBusiness::new();
    chatbots.set_bot(
        1234567,
        ChatbotsPermissions {
            can_reply: true,
            can_read: true,
            can_mark_read: true,
            can_delete_sent: false,
        },
    );
    assert!(chatbots.can_reply());

    chatbots.toggle_paused(999, true);
    assert!(chatbots.is_paused_for(999));
    chatbots.toggle_paused(999, false);
    assert!(!chatbots.is_paused_for(999));

    let mut shortcuts = DataShortcutMessagesBusiness::new();
    shortcuts.add_shortcut(1, "pricing", 456);
    shortcuts.add_shortcut(2, "support", 457);

    let sc = shortcuts.find_by_name("/pricing");
    assert!(sc.is_some());
    assert_eq!(sc.unwrap().id, 1);
    assert_eq!(sc.unwrap().name, "pricing");

    let sc2 = shortcuts.find_by_name("support");
    assert!(sc2.is_some());
    assert_eq!(sc2.unwrap().id, 2);
}

#[test]
fn test_streaming_and_media_rotation() {
    let mut stream = DataStreaming::new(10 * 1024 * 1024); // 10MB file
    assert!(!stream.can_playback());

    // Buffer first 2MB
    stream.append_buffered_range(0, 2 * 1024 * 1024);
    assert!(stream.can_playback());

    // Advance playback 500KB
    stream.advance_playback(512 * 1024);
    assert!(stream.can_playback());

    // Media rotation
    let mut rot = DataMediaRotation::new();
    assert!(rot.is_identity());
    assert_eq!(rot.transform_dimensions(1920, 1080), (1920, 1080));

    rot.rotate_cw();
    assert_eq!(rot.angle.as_degrees(), 90);
    assert_eq!(rot.transform_dimensions(1920, 1080), (1080, 1920));
}
