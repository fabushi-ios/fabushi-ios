//! Module 10 Calls Core & Window Subsystem Tests (Slice 1)
//! Verifies state machines, controllers, fingerprints, signal bars, video bubbles, and windows.
#![allow(unused_imports)]

use gpui_telegram_webrtc::calls::{
    calls_box_controller::{CallRecord, CallsBoxController},
    calls_call::{CallState, CallType, CallsCall, RemoteAudioState, RemoteBatteryState},
    calls_controller::{AudioConfig, CallVoipState, CallsController, NetworkType},
    calls_controller_tgvoip::{CallsControllerTgvoip, TgVoipEndpoint, TgVoipProxy},
    calls_controller_webrtc::{CallsControllerWebrtc, WebrtcSessionState},
    calls_emoji_fingerprint::{
        compute_emoji_fingerprint, compute_emoji_index_from_slice, CallsEmojiFingerprint,
        EMOJI_PALETTE, K_EMOJI_COUNT, K_EMOJI_IN_FINGERPRINT,
    },
    calls_instance::{CallSound, CallsInstance},
    calls_panel::{CallsPanel, PanelActionState},
    calls_panel_background::CallsPanelBackground,
    calls_rate_call::CallsRateCall,
    calls_signal_bars::CallsSignalBars,
    calls_top_bar::{BarState, CallsTopBar},
    calls_userpic::CallsUserpic,
    calls_video_bubble::{BubbleCorner, BubbleDragMode, CallsVideoBubble},
    calls_video_incoming::{CallsVideoIncoming, VideoRendererType},
    calls_window::CallsWindow,
    ui::calls_device_menu::{CallsDeviceMenuUi, DeviceInfo, DeviceType},
};

#[test]
fn test_call_state_machine_lifecycle() {
    let mut call = CallsCall::new(101, 202, CallType::Incoming, false);
    assert_eq!(call.state, CallState::WaitingIncoming);
    assert!(call.is_incoming_waiting());

    call.answer();
    assert_eq!(call.state, CallState::ExchangingKeys);

    let dummy_sha = [0x42u8; 32];
    call.keys_exchanged(dummy_sha);
    assert_eq!(call.state, CallState::Established);
    assert_eq!(call.signal_bar_count, 4);
    assert_eq!(call.key_sha256, Some(dummy_sha));

    call.hangup();
    assert_eq!(call.state, CallState::HangingUp);

    call.finish(true);
    assert_eq!(call.state, CallState::Ended);
    assert!(call.rating_requested);
}

#[test]
fn test_controller_audio_config_ducking() {
    let mut ctrl = CallsController::new(CallsController::WEBRTC_VERSION);
    assert!(ctrl.is_webrtc());
    assert_eq!(ctrl.audio_config.ducking_enabled, true);
    assert_eq!(ctrl.audio_config.echo_cancellation_strength, 3);

    ctrl.set_mute(true);
    assert!(ctrl.audio_config.mute_microphone);

    ctrl.record_traffic(1024, 2048);
    assert_eq!(ctrl.stats.bytes_sent, 1024);
    assert_eq!(ctrl.stats.bytes_received, 2048);

    let versions = CallsController::collect_controller_versions();
    assert_eq!(versions.len(), 2);
    assert!(versions.contains(&CallsController::WEBRTC_VERSION.to_string()));
}

#[test]
fn test_webrtc_controller_signaling_and_state() {
    let ep = TgVoipEndpoint {
        endpoint_id: 1,
        ipv4: "192.168.1.1".to_string(),
        ipv6: "::1".to_string(),
        port: 5000,
        peer_tag: [0xAA; 16],
        is_tcp: false,
    };
    let mut webrtc = CallsControllerWebrtc::new(&[ep], None, vec![0xBB; 32], true, true);
    assert_eq!(webrtc.session_state, WebrtcSessionState::Initializing);
    assert_eq!(webrtc.current_voip_state(), CallVoipState::WaitInit);

    assert!(webrtc.receive_signaling_data(b"webrtc-offer-packet"));
    assert_eq!(webrtc.signaling_packets.len(), 1);

    webrtc.set_session_state(WebrtcSessionState::Connected);
    assert_eq!(webrtc.current_voip_state(), CallVoipState::Established);

    webrtc.stop();
    assert!(webrtc.is_stopped);
    assert_eq!(webrtc.current_voip_state(), CallVoipState::Failed);
}

#[test]
fn test_tgvoip_controller_endpoints() {
    let ep = TgVoipEndpoint {
        endpoint_id: 99,
        ipv4: "10.0.0.1".to_string(),
        ipv6: "::1".to_string(),
        port: 443,
        peer_tag: [1; 16],
        is_tcp: true,
    };
    let proxy = TgVoipProxy {
        host: "proxy.tg.org".to_string(),
        port: 1080,
        login: "user".to_string(),
        password: "pwd".to_string(),
    };
    let mut tgvoip = CallsControllerTgvoip::new(vec![ep], Some(proxy));
    assert_eq!(tgvoip.endpoints.len(), 1);
    assert!(tgvoip.proxy.is_some());

    tgvoip.update_signal_bars(3);
    assert_eq!(tgvoip.signal_bars, 3);

    let stats = tgvoip.stop();
    assert!(tgvoip.stopped);
    assert_eq!(stats.bytes_sent, 0);
}

#[test]
fn test_calls_instance_coordination() {
    let mut instance = CallsInstance::new();
    assert!(!instance.in_call());

    instance.start_outgoing_call(1001, 555, true);
    assert!(instance.in_call());
    assert_eq!(instance.last_sound_played, Some("call_connect".to_string()));

    instance.destroy_current_call();
    assert!(!instance.in_call());
    assert_eq!(instance.last_sound_played, Some("call_end".to_string()));
}

#[test]
fn test_conference_invites_registration() {
    let mut instance = CallsInstance::new();
    instance.register_conference_invite(500, 777, 10001, true);
    instance.register_conference_invite(500, 777, 10002, false);

    let conf = instance.conference_invites.get(&500).unwrap();
    let user_invites = conf.get(&777).unwrap();
    assert!(user_invites.incoming.contains(&10001));
    assert!(user_invites.outgoing.contains(&10002));

    instance.unregister_conference_invite(500, 777, 10001, true);
    let user_invites = instance.conference_invites.get(&500).unwrap().get(&777).unwrap();
    assert!(!user_invites.incoming.contains(&10001));
    assert!(user_invites.outgoing.contains(&10002));
}

#[test]
fn test_box_controller_history_pagination() {
    let mut box_ctrl = CallsBoxController::new();
    let page1: Vec<CallRecord> = (1..=20)
        .map(|i| CallRecord {
            message_id: i,
            peer_id: 100 + (i as u64),
            date: 1700000000 + i,
            duration_seconds: (i * 10) as i32,
            is_video: i % 2 == 0,
            is_outgoing: i % 3 == 0,
            is_missed: false,
        })
        .collect();

    box_ctrl.receive_calls_page(page1);
    assert_eq!(box_ctrl.records.len(), 20);
    assert_eq!(box_ctrl.offset_id, 20);

    assert!(box_ctrl.delete_call_record(10));
    assert_eq!(box_ctrl.records.len(), 19);

    box_ctrl.clear_all_calls();
    assert_eq!(box_ctrl.records.len(), 0);
    assert!(box_ctrl.all_loaded);
}

#[test]
fn test_emoji_fingerprint_generation_parity() {
    let dummy_sha = [
        0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0,
        0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01,
        0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x12,
        0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23,
    ];
    let indices = compute_emoji_fingerprint(&dummy_sha);
    assert_eq!(indices.len(), K_EMOJI_IN_FINGERPRINT);
    for &idx in &indices {
        assert!(idx < EMOJI_PALETTE.len());
    }

    let fp = CallsEmojiFingerprint::from_sha256(dummy_sha);
    assert_eq!(fp.emoji_chars.len(), 4);
    assert!(!fp.as_emoji_string().is_empty());
}

#[test]
fn test_signal_bars_quality_calculation() {
    let mut bars = CallsSignalBars::new();
    assert_eq!(bars.count, CallsSignalBars::K_SIGNAL_BAR_STARTING);

    // Excellent connection: low latency, zero loss -> 4 bars
    bars.update_network_stats(45, 0.0, 5);
    assert_eq!(bars.count, 4);

    // Moderate jitter/loss -> 3 bars
    bars.update_network_stats(250, 0.03, 35);
    assert_eq!(bars.count, 3);

    // Poor connection -> 2 bars
    bars.update_network_stats(500, 0.10, 70);
    assert_eq!(bars.count, 2);

    // Terrible connection -> 1 bar
    bars.update_network_stats(900, 0.20, 120);
    assert_eq!(bars.count, 1);
}

#[test]
fn test_signal_bars_geometry_and_opacity() {
    let mut bars = CallsSignalBars::new();
    bars.count = 3;

    let h0 = bars.calculate_bar_height(0);
    let h3 = bars.calculate_bar_height(3);
    assert_eq!(h0, 4.0);
    assert_eq!(h3, 10.0);

    assert_eq!(bars.bar_opacity(2), 1.0);
    assert_eq!(bars.bar_opacity(3), 0.5); // 4th bar is inactive
    assert!(bars.total_width() > 10.0);
}

#[test]
fn test_calls_window_state_controls() {
    let mut window = CallsWindow::new();
    assert_eq!(window.width, CallsWindow::DEFAULT_WIDTH);
    assert_eq!(window.height, CallsWindow::DEFAULT_HEIGHT);

    assert!(window.toggle_pin_on_top());
    assert!(window.is_pinned_on_top);

    assert!(window.toggle_full_screen());
    assert!(!window.is_pinned_on_top); // full screen cancels pin on top

    window.show_layer("settings_layer");
    assert_eq!(window.active_layer_id, Some("settings_layer".to_string()));
    window.hide_layer();
    assert!(window.active_layer_id.is_none());
}

#[test]
fn test_calls_panel_autohide_and_battery() {
    let mut panel = CallsPanel::new(101, 202, true);
    assert_eq!(panel.action_state, PanelActionState::Answer);

    panel.user_activity(1000);
    assert!(panel.controls_visible);

    panel.check_autohide(1000 + 3000);
    assert!(panel.controls_visible);

    panel.check_autohide(1000 + 5001);
    assert!(!panel.controls_visible);

    panel.set_battery_low(true);
    assert!(panel.low_battery_warning);

    assert!(panel.toggle_camera());
    assert!(!panel.is_screen_sharing);
    assert!(panel.toggle_screen_sharing());
    assert!(!panel.is_camera_on);
}

#[test]
fn test_calls_panel_background_gradient() {
    let mut bg = CallsPanelBackground::new(1001, vec![0x2AABEE, 0x223344]);
    bg.update_geometry(720.0, 540.0, 21.0, 160.0);

    assert_eq!(bg.center_x, 360.0);
    assert_eq!(bg.center_y, 101.0);
    assert!(bg.radius > 360.0);

    let text_color = bg.text_color_override();
    assert!(text_color == 0x000000 || text_color == 0xFFFFFF);
}

#[test]
fn test_video_bubble_corner_snapping() {
    let mut bubble = CallsVideoBubble::new();
    bubble.snap_to_corner(720.0, 540.0, BubbleCorner::BottomRight);
    assert_eq!(bubble.x, 720.0 - CallsVideoBubble::DEFAULT_WIDTH - CallsVideoBubble::MARGIN);
    assert_eq!(bubble.y, 540.0 - CallsVideoBubble::DEFAULT_HEIGHT - CallsVideoBubble::MARGIN);

    bubble.snap_to_corner(720.0, 540.0, BubbleCorner::TopLeft);
    assert_eq!(bubble.x, CallsVideoBubble::MARGIN);
    assert_eq!(bubble.y, CallsVideoBubble::MARGIN);

    assert!(bubble.is_mirrored);
    bubble.set_mirrored(false);
    assert!(!bubble.is_mirrored);
}

#[test]
fn test_video_incoming_surface_and_ratio() {
    let mut incoming = CallsVideoIncoming::new(VideoRendererType::RhiMetal);
    assert_eq!(incoming.renderer_type, VideoRendererType::RhiMetal);
    assert_eq!(incoming.bottom_shadow_alpha_max, CallsVideoIncoming::K_BOTTOM_SHADOW_ALPHA_MAX);

    incoming.update_frame_dimensions(1920, 1080, 0);
    assert!(incoming.has_frame);
    let ratio = incoming.aspect_ratio();
    assert!((ratio - (16.0 / 9.0)).abs() < 0.01);
}

#[test]
fn test_rate_call_burst_threshold() {
    let mut rate = CallsRateCall::new(999);
    assert_eq!(rate.rating, 0);

    // Rating 3 should NOT trigger celebratory burst
    let burst3 = rate.set_rating(3);
    assert!(!burst3);
    assert_eq!(rate.burst_count, 0);

    // Rating 4 SHOULD trigger celebratory burst
    let burst4 = rate.set_rating(4);
    assert!(burst4);
    assert_eq!(rate.burst_count, 1);

    // Rating 5 also triggers burst
    let burst5 = rate.set_rating(5);
    assert!(burst5);
    assert_eq!(rate.burst_count, 2);

    rate.submit("Clear audio quality, great connection!");
    assert!(rate.is_submitted);
    assert_eq!(rate.comment, "Clear audio quality, great connection!");
}

#[test]
fn test_device_selection_menu() {
    let mut menu = CallsDeviceMenuUi::new(DeviceType::Capture);
    let dev1 = DeviceInfo {
        id: "mic-1".to_string(),
        name: "Built-in Microphone".to_string(),
        is_default: true,
        is_inactive: false,
    };
    let dev2 = DeviceInfo {
        id: "mic-2".to_string(),
        name: "Studio USB Mic".to_string(),
        is_default: false,
        is_inactive: false,
    };
    let dev_inactive = DeviceInfo {
        id: "mic-old".to_string(),
        name: "Disconnected Headset".to_string(),
        is_default: false,
        is_inactive: true,
    };

    menu.set_devices(vec![dev1, dev2, dev_inactive]);
    assert_eq!(menu.active_devices().len(), 2);

    menu.select_device("mic-2");
    assert_eq!(menu.selected_device_id, "mic-2");

    assert!(menu.toggle_menu());
    assert!(menu.is_open);
}
