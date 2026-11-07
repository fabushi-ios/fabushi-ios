use mahayana_host_runtime::extensions::meeting_bot::audio_bridge::{
    decode_meeting_audio_base64, is_meeting_audio_base64, AudioSampleRate, MeetingAudioChunk,
};
use mahayana_host_runtime::extensions::meeting_bot::browser_controller::{
    MeetingBrowserActLock, BROWSER_ACT_TIMEOUT_MESSAGE,
};
use mahayana_host_runtime::extensions::meeting_bot::session_lifecycle::{
    is_meeting_realtime_route_ready, is_meeting_talk_back_mode, MeetingBrowserHealth,
    MeetingSessionCleanupTracker,
};

#[test]
fn test_ocrm_meeting_bot_audio_bridge() {
    // Valid base64 payload
    let valid_b64 = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YQAAAAA=";
    assert!(is_meeting_audio_base64(valid_b64));
    let decoded = decode_meeting_audio_base64(valid_b64, "play").expect("valid audio payload");
    assert!(!decoded.is_empty());

    // Invalid base64 payload
    assert!(!is_meeting_audio_base64(""));
    let err = decode_meeting_audio_base64("!!!invalid-base64!!!", "inject")
        .expect_err("should reject invalid base64");
    assert!(err.contains("inject base64 must be a valid audio payload"));

    // Audio chunk duration calculation: 16000Hz, mono (1 channel), 16-bit (2 bytes per sample)
    // 32000 bytes = 16000 samples = 1000ms
    let chunk = MeetingAudioChunk::new(
        "sess-1",
        0,
        AudioSampleRate::Rate16kHz,
        1,
        vec![0u8; 32000],
    );
    assert_eq!(chunk.duration_ms(), 1000);
}

#[test]
fn test_ocrm_meeting_bot_browser_controller() {
    let act_lock = MeetingBrowserActLock::new();
    let target = "tab-target-1";

    // Immediate acquisition when free
    assert!(act_lock.acquire_lock(target, 1000, 2000).is_ok());

    // Contention while target is locked (now=1100 < 2000)
    let err = act_lock
        .acquire_lock(target, 1100, 2000)
        .expect_err("locked target should reject");
    assert_eq!(err, BROWSER_ACT_TIMEOUT_MESSAGE);

    // Expired deadline request (now=2500 >= deadline=2000)
    let expired_err = act_lock
        .acquire_lock(target, 2500, 2000)
        .expect_err("expired deadline should reject");
    assert_eq!(expired_err, BROWSER_ACT_TIMEOUT_MESSAGE);

    // Release lock
    act_lock.release_lock(target);
    assert!(act_lock.acquire_lock(target, 1200, 2500).is_ok());
}

#[test]
fn test_ocrm_meeting_bot_session_lifecycle() {
    // Mode checks
    assert!(is_meeting_talk_back_mode("agent"));
    assert!(is_meeting_talk_back_mode("bidi"));
    assert!(!is_meeting_talk_back_mode("listen"));
    assert!(!is_meeting_talk_back_mode("transcribe_only"));

    // Realtime route ready check
    let ready_health = MeetingBrowserHealth {
        in_call: true,
        mic_muted: false,
        audio_input_routed: true,
        audio_output_routed: true,
        manual_action: None,
    };
    assert!(is_meeting_realtime_route_ready("bidi", &ready_health));
    assert!(!is_meeting_realtime_route_ready("listen", &ready_health));

    let muted_health = MeetingBrowserHealth {
        mic_muted: true,
        ..ready_health.clone()
    };
    assert!(!is_meeting_realtime_route_ready("bidi", &muted_health));

    let manual_action_health = MeetingBrowserHealth {
        manual_action: Some("user_clicked_leave".to_string()),
        ..ready_health
    };
    assert!(!is_meeting_realtime_route_ready("bidi", &manual_action_health));

    // Cleanup tracker
    let mut tracker = MeetingSessionCleanupTracker::new();
    tracker.register_session("sess-meet-1");
    assert!(!tracker.is_cleanup_complete("sess-meet-1"));

    tracker.mark_browser_left("sess-meet-1");
    assert!(!tracker.is_cleanup_complete("sess-meet-1"));

    tracker.mark_stop_settled("sess-meet-1");
    assert!(!tracker.is_cleanup_complete("sess-meet-1"));

    tracker.mark_unpublished("sess-meet-1");
    assert!(tracker.is_cleanup_complete("sess-meet-1"));
}
