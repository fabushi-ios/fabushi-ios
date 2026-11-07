//! Subsystem Tests for Module 10 Slice 2: Group Calls (Part 1)
//! Covers: Group call participants, speak requests queue, RTMP parameters,
//! sound settings, subtitle layout, tooltip alerts, push-to-talk throttling.

use gpui_telegram_webrtc::calls::group::*;

#[test]
fn test_choose_join_as_peer_selection() {
    let mut join_as = CallsChooseJoinAsGroup::new();
    join_as.start(1001, 42, "Alice");
    join_as.add_channel(2001, "Tech News Channel", 15000);
    join_as.add_channel(2002, "Rust Developers", 8500);

    // Initial state: personal identity selected
    let info = join_as.finish(true, false).expect("Join info must be created");
    assert_eq!(info.call_id, 1001);
    assert_eq!(info.join_as_peer_id, 42);
    assert!(info.muted);
    assert!(!info.video_enabled);

    // Select channel identity
    let selected = join_as.select(2002);
    assert!(selected);
    let info2 = join_as.finish(false, true).expect("Join info must be created");
    assert_eq!(info2.join_as_peer_id, 2002);
    assert!(!info2.muted);
    assert!(info2.video_enabled);

    // Select invalid ID returns false
    assert!(!join_as.select(9999));
}

#[test]
fn test_cover_item_layout_and_scheduling() {
    let mut cover = CallsCoverItemGroup::new("Rust Community Meetup", "Live Voice Chat", 101);
    assert_eq!(cover.info.title, "Rust Community Meetup");
    assert!(!cover.info.has_custom_cover);

    // Without custom cover: fixed card height
    let (w1, h1) = cover.compute_card_size(400.0);
    assert_eq!(w1, 400.0);
    assert_eq!(h1, 120.0);

    // With custom 16:9 cover
    cover.set_custom_cover(1920, 1080);
    let (w2, h2) = cover.compute_card_size(400.0);
    assert_eq!(w2, 400.0);
    assert_eq!(h2, 225.0); // 400 * 0.5625

    // Scheduling
    cover.set_scheduled(1700000000);
    assert!(cover.is_scheduled);
    assert_eq!(cover.schedule_timestamp, Some(1700000000));
}

#[test]
fn test_group_call_state_transitions() {
    let mut call = CallsGroupCallGroup::new(555, 42, true);
    assert_eq!(call.state, calls_group_call::CallState::Disconnected);
    assert_eq!(call.participants.len(), 1); // self participant initialized

    // Join
    call.join(calls_group_common::JoinInfo {
        call_id: 555,
        join_as_peer_id: 42,
        muted: true,
        video_enabled: false,
        schedule_date: None,
    });
    assert_eq!(call.state, calls_group_call::CallState::Connecting);

    // Connected
    call.mark_connected();
    assert_eq!(call.state, calls_group_call::CallState::Active);

    // Add and remove peer
    call.add_participant(calls_group_common::ParticipantState {
        id: 100,
        name: "Bob".to_string(),
        mute_state: calls_group_common::MuteState::Unmuted,
        volume: 10000,
        audio_level: 0.0,
        raise_hand_timestamp: None,
        is_self: false,
        can_self_unmute: true,
    });
    assert_eq!(call.participants.len(), 2);

    call.remove_participant(100);
    assert_eq!(call.participants.len(), 1);

    // Terminate
    call.terminate();
    assert_eq!(call.state, calls_group_call::CallState::Terminated);
    assert_eq!(call.participants.len(), 0);
}

#[test]
fn test_group_call_audio_level_and_active_speaker_detection() {
    let mut call = CallsGroupCallGroup::new(101, 42, false);
    call.add_participant(calls_group_common::ParticipantState {
        id: 200,
        name: "Charlie".to_string(),
        mute_state: calls_group_common::MuteState::Unmuted,
        volume: 10000,
        audio_level: 0.0,
        raise_hand_timestamp: None,
        is_self: false,
        can_self_unmute: true,
    });

    // Level below threshold (0.05) -> still unmuted, not in active speakers
    call.update_audio_level(200, 0.02);
    assert_eq!(call.participants.get(&200).unwrap().mute_state, calls_group_common::MuteState::Unmuted);
    assert!(!call.active_speakers.contains(&200));

    // Level above threshold (0.05) -> transitions to Speaking, added to active speakers
    call.update_audio_level(200, 0.45);
    assert_eq!(call.participants.get(&200).unwrap().mute_state, calls_group_common::MuteState::Speaking);
    assert!(call.active_speakers.contains(&200));

    // Level drops back -> transitions back to Unmuted, removed from active speakers
    call.update_audio_level(200, 0.01);
    assert_eq!(call.participants.get(&200).unwrap().mute_state, calls_group_common::MuteState::Unmuted);
    assert!(!call.active_speakers.contains(&200));
}

#[test]
fn test_participant_volume_curve_and_scaling() {
    // 100% volume
    assert_eq!(calls_group_common::ratio_to_volume(1.0), 10000);
    assert_eq!(calls_group_common::volume_to_ratio(10000), 1.0);

    // 0% volume
    assert_eq!(calls_group_common::ratio_to_volume(0.0), 0);
    assert_eq!(calls_group_common::volume_to_ratio(0), 0.0);

    // 200% volume (max)
    assert_eq!(calls_group_common::ratio_to_volume(2.0), 20000);
    assert_eq!(calls_group_common::volume_to_ratio(20000), 2.0);

    // Clamping beyond 2.0 / 20000
    assert_eq!(calls_group_common::ratio_to_volume(3.5), 20000);
    assert_eq!(calls_group_common::volume_to_ratio(25000), 2.0);

    // Duration formatting
    assert_eq!(calls_group_common::format_call_duration(45), "00:45");
    assert_eq!(calls_group_common::format_call_duration(125), "02:05");
    assert_eq!(calls_group_common::format_call_duration(3665), "01:01:05");
}

#[test]
fn test_self_mute_state_machine_and_hand_raising() {
    let mut call = CallsGroupCallGroup::new(101, 42, false);

    // Default self state is MutedByMe -> toggles to Unmuted
    let s1 = call.toggle_self_mute();
    assert_eq!(s1, calls_group_common::MuteState::Unmuted);

    // Unmuted -> toggles to MutedByMe
    let s2 = call.toggle_self_mute();
    assert_eq!(s2, calls_group_common::MuteState::MutedByMe);

    // When admin mutes user -> user cannot unmute directly, toggling raises hand
    if let Some(p) = call.participants.get_mut(&42) {
        p.mute_state = calls_group_common::MuteState::MutedByAdmin;
        p.can_self_unmute = false;
    }
    let s3 = call.toggle_self_mute();
    assert_eq!(s3, calls_group_common::MuteState::RaisedHand);
    assert!(call.participants.get(&42).unwrap().raise_hand_timestamp.is_some());

    // Toggling while hand raised lowers hand
    let s4 = call.toggle_self_mute();
    assert_eq!(s4, calls_group_common::MuteState::MutedByAdmin);
    assert!(call.participants.get(&42).unwrap().raise_hand_timestamp.is_none());
}

#[test]
fn test_speak_requests_queue_priority_and_approval() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();
    queue.raise_hand(101, "Dave");
    queue.raise_hand(102, "Eva");
    queue.raise_hand(103, "Frank");

    assert_eq!(queue.count_pending(), 3);
    let pending = queue.pending_requests();
    assert_eq!(pending[0].user_id, 101);
    assert_eq!(pending[1].user_id, 102);

    // Approve Eva
    let approved = queue.approve_request(102);
    assert!(approved);
    assert_eq!(queue.count_pending(), 2);

    // Lower Frank hand
    let lowered = queue.lower_hand(103);
    assert!(lowered);
    assert_eq!(queue.count_pending(), 1);
    assert_eq!(queue.pending_requests()[0].user_id, 101);
}

#[test]
fn test_audio_level_blob_pulsation_bounds() {
    let base_radius = 40.0;

    // Zero level -> zero blob expansion
    let (p0, m0) = calls_group_members_row::compute_speaking_blob_radii(base_radius, 0.0, 0.0);
    assert!((p0 - 40.0).abs() < 1e-4);
    assert!((m0 - 40.0).abs() < 1e-4);

    // Max level (1.0) -> expansion bounded by kBlobScale (0.605)
    let (p1, m1) = calls_group_members_row::compute_speaking_blob_radii(base_radius, 1.0, 0.0);
    assert!(p1 > base_radius);
    assert!(p1 <= base_radius * (1.0 + 0.605 * 1.16));
    assert!(m1 > base_radius);
    assert!(m1 < p1); // Minor blob is smaller than primary blob
}

#[test]
fn test_invite_controller_batch_slicing_and_links() {
    let mut controller = CallsGroupInviteControllerGroup::new(777);
    let user_ids: Vec<u64> = (1..=25).collect();
    controller.enqueue_invites(&user_ids);

    assert_eq!(controller.pending_count(), 25);

    // Slice 1: exactly 10
    let s1 = controller.process_next_slice();
    assert_eq!(s1.len(), calls_group_invite_controller::K_MAX_INVITE_PER_SLICE);
    assert_eq!(controller.pending_count(), 15);

    // Slice 2: exactly 10
    let s2 = controller.process_next_slice();
    assert_eq!(s2.len(), 10);
    assert_eq!(controller.pending_count(), 5);

    // Slice 3: remaining 5
    let s3 = controller.process_next_slice();
    assert_eq!(s3.len(), 5);
    assert_eq!(controller.pending_count(), 0);

    // Invite link generation
    let link = controller.generate_invite_link(calls_group_invite_controller::InviteLinkType::CanSpeak);
    assert!(link.contains("https://t.me/call?video_chat=777"));
    assert!(link.contains("speak"));
}
#[test]
fn test_members_sorting_by_speaker_and_hand() {
    let mut members = CallsGroupMembersGroup::new();
    members.update_or_insert(calls_group_common::ParticipantState {
        id: 1,
        name: "Alice (Muted)".to_string(),
        mute_state: calls_group_common::MuteState::MutedByMe,
        volume: 10000,
        audio_level: 0.0,
        raise_hand_timestamp: None,
        is_self: false,
        can_self_unmute: true,
    });
    members.update_or_insert(calls_group_common::ParticipantState {
        id: 2,
        name: "Bob (Speaking Loud)".to_string(),
        mute_state: calls_group_common::MuteState::Speaking,
        volume: 10000,
        audio_level: 0.9,
        raise_hand_timestamp: None,
        is_self: false,
        can_self_unmute: true,
    });
    members.update_or_insert(calls_group_common::ParticipantState {
        id: 3,
        name: "Charlie (Speaking Soft)".to_string(),
        mute_state: calls_group_common::MuteState::Speaking,
        volume: 10000,
        audio_level: 0.2,
        raise_hand_timestamp: None,
        is_self: false,
        can_self_unmute: true,
    });
    members.update_or_insert(calls_group_common::ParticipantState {
        id: 4,
        name: "Dave (Raised Hand)".to_string(),
        mute_state: calls_group_common::MuteState::RaisedHand,
        volume: 10000,
        audio_level: 0.0,
        raise_hand_timestamp: Some(100),
        is_self: false,
        can_self_unmute: false,
    });

    let sorted = members.sorted_members();
    assert_eq!(sorted.len(), 4);
    // Speaking loud first
    assert_eq!(sorted[0].id, 2);
    // Speaking soft second
    assert_eq!(sorted[1].id, 3);
    // Raised hand third
    assert_eq!(sorted[2].id, 4);
    // Muted last
    assert_eq!(sorted[3].id, 1);

    // Filter by search query
    members.set_search_query("dave");
    let filtered = members.sorted_members();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, 4);
}

#[test]
fn test_menu_actions_and_admin_permissions() {
    // Non-admin menu
    let non_admin_menu = CallsGroupMenuGroup::new(false, false, false);
    let actions: Vec<_> = non_admin_menu.entries.iter().map(|e| e.action).collect();
    assert!(actions.contains(&calls_group_menu::GroupMenuAction::AudioSettings));
    assert!(actions.contains(&calls_group_menu::GroupMenuAction::LeaveCall));
    assert!(!actions.contains(&calls_group_menu::GroupMenuAction::EndCallForEveryone));
    assert!(!actions.contains(&calls_group_menu::GroupMenuAction::StartRecording));

    // Admin menu
    let admin_menu = CallsGroupMenuGroup::new(true, false, false);
    let admin_actions: Vec<_> = admin_menu.entries.iter().map(|e| e.action).collect();
    assert!(admin_actions.contains(&calls_group_menu::GroupMenuAction::StartRecording));
    assert!(admin_actions.contains(&calls_group_menu::GroupMenuAction::EditPermissions));
    assert!(admin_actions.contains(&calls_group_menu::GroupMenuAction::EndCallForEveryone));
}

#[test]
fn test_message_encryption_roundtrip_and_tamper_detection() {
    let mut sender_crypto = CallsGroupMessageEncryptionGroup::new(1, [0x42u8; 32]);
    let mut receiver_crypto = CallsGroupMessageEncryptionGroup::new(1, [0x42u8; 32]);

    let original = calls_group_message_encryption::PreparedMessage {
        sender_id: 101,
        message_id: 2001,
        text: "Hello WebRTC Live Chat!".to_string(),
        timestamp: 1700000000,
    };

    // Encrypt
    let mut encrypted = sender_crypto.encrypt(&original).expect("Encryption should succeed");
    assert_eq!(encrypted.seq_no, 1);

    // Decrypt
    let decrypted = receiver_crypto.decrypt(&encrypted).expect("Decryption should succeed");
    assert_eq!(decrypted, original);

    // Tamper with ciphertext -> MAC verification fails
    encrypted.ciphertext[0] ^= 0xff;
    let err = receiver_crypto.decrypt(&encrypted);
    assert!(err.is_err());
}

#[test]
fn test_in_call_chat_ring_buffer_eviction() {
    let mut msg_store = CallsGroupMessagesGroup::new();

    // Push 105 messages -> oldest 5 evicted, capacity stays 100
    for i in 1..=105 {
        msg_store.push_message(10, "Sender", &format!("Message {}", i));
    }

    assert_eq!(msg_store.messages.len(), calls_group_messages::K_MAX_SHOWN_VIDEO_STREAM_MESSAGES);
    let msgs = msg_store.get_messages();
    assert_eq!(msgs[0].text, "Message 6");
    assert_eq!(msgs[msgs.len() - 1].text, "Message 105");

    // Star donation
    msg_store.push_star_donation(101, "DonorAlice", 50);
    msg_store.push_star_donation(102, "DonorBob", 150);
    msg_store.push_star_donation(101, "DonorAlice", 120);

    let top = msg_store.top_donors(5);
    assert_eq!(top.len(), 2);
    assert_eq!(top[0].user_id, 101); // 170 total stars
    assert_eq!(top[0].total_stars, 170);
    assert_eq!(top[1].user_id, 102); // 150 total stars
}

#[test]
fn test_messages_ui_bubble_layout_and_pinning() {
    let mut ui = CallsGroupMessagesUiGroup::new();
    let msg = calls_group_messages::CallChatMessage {
        id: 1,
        sender_id: 42,
        sender_name: "Host".to_string(),
        text: "Welcome to today's livestream! Please keep questions in the chat.".to_string(),
        timestamp: 100,
        stars_count: 0,
    };

    // Pinning
    ui.pin_message(msg.clone());
    assert!(ui.pinned_message.is_some());
    assert_eq!(ui.pinned_message.as_ref().unwrap().id, 1);

    // Normal message layout
    let layout = ui.layout_bubble(&msg, 400.0);
    assert_eq!(layout.bg_opacity, calls_group_messages_ui::K_MESSAGE_BG_OPACITY);
    assert!(!layout.is_donation);

    // Donation message layout
    let donation_msg = calls_group_messages::CallChatMessage {
        id: 2,
        sender_id: 88,
        sender_name: "Fan".to_string(),
        text: "Sent 100 Stars!".to_string(),
        timestamp: 105,
        stars_count: 100,
    };
    let donation_layout = ui.layout_bubble(&donation_msg, 400.0);
    assert_eq!(donation_layout.bg_opacity, calls_group_messages_ui::K_COLORED_MESSAGE_BG_OPACITY);
    assert!(donation_layout.is_donation);
}

#[test]
fn test_spacebar_push_to_talk_throttling() {
    let mut panel = CallsGroupPanelGroup::new("Office Chat", 5);
    assert!(panel.is_muted);

    // Space down -> instantly unmuted
    panel.on_space_down();
    assert!(!panel.is_muted);
    assert!(panel.space_pressed);
    assert!(panel.space_release_timer_ms.is_none());

    // Space up -> starts 250ms release timer, mic remains unmuted temporarily
    panel.on_space_up();
    assert!(!panel.is_muted);
    assert_eq!(panel.space_release_timer_ms, Some(calls_group_panel::K_SPACE_PUSH_TO_TALK_DELAY_MS));

    // Tick 100ms -> timer not expired, still unmuted
    panel.tick_timer(100);
    assert!(!panel.is_muted);
    assert_eq!(panel.space_release_timer_ms, Some(150));

    // Tick remaining 150ms -> timer expires, mic is re-muted
    panel.tick_timer(150);
    assert!(panel.is_muted);
    assert!(panel.space_release_timer_ms.is_none());
}

#[test]
fn test_rtmp_url_generation_and_stream_key_masking() {
    let rtmp = CallsGroupRtmpGroup::new(123456, 2, "secret_live_hash_abcdef");
    assert_eq!(rtmp.server_url(), "rtmps://dc2-rtmp.telegram.org:443/live/");
    assert_eq!(rtmp.stream_key(), "123456_secret_live_hash_abcdef");

    // Masked stream key for UI display
    let masked = rtmp.masked_stream_key();
    assert!(masked.starts_with("123456_"));
    assert!(masked.contains("••••••••"));
    assert!(!masked.contains("secret_live_hash"));

    let info = rtmp.to_rtmp_info();
    assert_eq!(info.url, "rtmps://dc2-rtmp.telegram.org:443/live/");
    assert_eq!(info.key, "123456_secret_live_hash_abcdef");
}

#[test]
fn test_sound_settings_mic_level_tester() {
    let mut tester = calls_group_settings::MicLevelTester::new();
    assert!(!tester.tooltip_shown);

    // Quiet samples -> does not trigger tooltip
    for _ in 0..5 {
        let show = tester.update_sample(0.1);
        assert!(!show);
    }

    // Loud samples while muted -> after 3 loud samples triggers tooltip
    assert!(!tester.update_sample(0.3));
    assert!(!tester.update_sample(0.35));
    let triggered = tester.update_sample(0.4);
    assert!(triggered);
    assert!(tester.tooltip_shown);
}

#[test]
fn test_toasts_and_tooltips_priority() {
    let mut tooltip_mgr = CallsGroupTooltip::new();

    // Normal tooltip
    tooltip_mgr.show("mute_hint", "You are muted. Press space to speak.", calls_group_tooltip::TooltipPriority::Normal, 3000);
    assert_eq!(tooltip_mgr.current.as_ref().unwrap().key, "mute_hint");

    // Lower priority tooltip is rejected while normal is active
    tooltip_mgr.show("low_hint", "Network is slow", calls_group_tooltip::TooltipPriority::Low, 1000);
    assert_eq!(tooltip_mgr.current.as_ref().unwrap().key, "mute_hint");

    // Critical tooltip replaces normal
    tooltip_mgr.show("mic_denied", "Microphone access denied", calls_group_tooltip::TooltipPriority::Critical, 5000);
    assert_eq!(tooltip_mgr.current.as_ref().unwrap().key, "mic_denied");

    // Toasts error translation key
    let err = calls_group_toasts::GroupCallError::NoCamera;
    assert_eq!(err.translation_key(), "lng_call_error_no_camera");
}
