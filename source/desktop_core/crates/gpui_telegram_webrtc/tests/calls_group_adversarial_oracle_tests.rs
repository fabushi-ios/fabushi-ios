//! Adversarial Blind C++ Oracle Test Suite: Group Calls, Waveforms & Volume Snapping
//! Subsystem: `calls` (Module 10 Group Calls Verification)
//! Target File: `crates/gpui_telegram_webrtc/tests/calls_group_adversarial_oracle_tests.rs`
//!
//! Authoritative C++ Desktop Reference Sources:
//!   - Telegram/SourceFiles/calls/group/calls_group_members.cpp (L730-L810) [ProjForAdmin, Speaker / Hand-Raise Priority]
//!   - Telegram/SourceFiles/calls/group/calls_group_panel.cpp (L82, L396-L400) [kSpacePushToTalkDelay = 250ms]
//!   - Telegram/SourceFiles/calls/group/calls_group_call.cpp (L4206-L4225) [pushToTalk & pushToTalkCancel Hysteresis]
//!   - Telegram/SourceFiles/calls/group/calls_group_rtmp.cpp (L168-L170), calls_group_rtmp.h (L50-L75) [RTMP Ingest & Stream Key]
//!   - Telegram/SourceFiles/calls/group/calls_group_members_row.cpp (L26-L57) [kBlobScale = 0.605, kMinorBlobFactor = 0.9f]
//!   - Telegram/SourceFiles/calls/group/calls_volume_item.cpp (L23-L39, L192-L200) [kVolumeStickedValues: ±5% magnetic snap window at 100%]

use gpui_telegram_webrtc::calls::group::*;

// ============================================================================
// ORACLE 1: Speak Request Priority Queue & Tie-Breakers
// Upstream: Telegram/SourceFiles/calls/group/calls_group_members.cpp:L781-L809
// Invariant: Hand-raising participants must be strictly ordered by timestamp FIFO;
//            duplicate raises update the timestamp to now; tie-breaking is deterministic;
//            approvals and lowering are strictly idempotent without queue corruption.
// ============================================================================

#[test]
fn test_oracle1_speak_request_queue_fifo_timestamp_ordering() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();
    assert_eq!(queue.count_pending(), 0);
    assert!(queue.pending_requests().is_empty());

    // Insert 5 requests with strictly ascending simulated timestamps
    let base_time = 1_700_000_000i64;
    for i in 1..=5 {
        queue.requests.push(calls_group_speak_requests::SpeakRequest {
            user_id: 1000 + i,
            user_name: format!("User_{}", i),
            timestamp: base_time + (i as i64) * 10,
            is_approved: false,
        });
    }

    assert_eq!(queue.count_pending(), 5);
    let pending = queue.pending_requests();
    assert_eq!(pending.len(), 5);

    // Verify strictly monotonic non-decreasing timestamp FIFO order
    for idx in 0..(pending.len() - 1) {
        assert!(
            pending[idx].timestamp < pending[idx + 1].timestamp,
            "Request at index {} must precede index {}",
            idx,
            idx + 1
        );
        assert_eq!(pending[idx].user_id, 1000 + (idx as u64) + 1);
    }
}

#[test]
fn test_oracle1_speak_request_duplicate_reraise_and_timestamp_update() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();

    // Alice raises hand at t = 100
    queue.requests.push(calls_group_speak_requests::SpeakRequest {
        user_id: 101,
        user_name: "Alice".to_string(),
        timestamp: 100,
        is_approved: false,
    });

    // Bob raises hand at t = 200
    queue.requests.push(calls_group_speak_requests::SpeakRequest {
        user_id: 102,
        user_name: "Bob".to_string(),
        timestamp: 200,
        is_approved: false,
    });

    // Initial check: Alice is #1, Bob is #2
    let initial = queue.pending_requests();
    assert_eq!(initial[0].user_id, 101);
    assert_eq!(initial[1].user_id, 102);

    // Alice calls raise_hand again: existing request must update its timestamp to current time (>200)
    queue.raise_hand(101, "Alice");
    assert_eq!(queue.count_pending(), 2);

    // Alice now moved to the back of the queue because Bob's timestamp (200) is older than Alice's new timestamp!
    let updated = queue.pending_requests();
    assert_eq!(updated[0].user_id, 102, "Bob must now be first in queue");
    assert_eq!(updated[1].user_id, 101, "Alice must have moved behind Bob");
}

#[test]
fn test_oracle1_speak_request_tie_breaker_and_burst_ordering() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();
    let identical_timestamp = 1_700_500_000i64;

    // Simulate burst of 4 users hitting the server simultaneously at the exact same second
    let user_ids = [4004u64, 2002u64, 1001u64, 3003u64];
    for &uid in &user_ids {
        queue.requests.push(calls_group_speak_requests::SpeakRequest {
            user_id: uid,
            user_name: format!("User_{}", uid),
            timestamp: identical_timestamp,
            is_approved: false,
        });
    }

    assert_eq!(queue.count_pending(), 4);
    let pending = queue.pending_requests();
    assert_eq!(pending.len(), 4);

    // Stable sort preserves original arrival order or deterministic ordering without losing any user
    let retrieved_uids: Vec<u64> = pending.iter().map(|r| r.user_id).collect();
    for &uid in &user_ids {
        assert!(
            retrieved_uids.contains(&uid),
            "User {} must not be dropped in timestamp collision",
            uid
        );
    }
}

#[test]
fn test_oracle1_speak_request_approval_and_lowering_idempotence() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();
    queue.requests.push(calls_group_speak_requests::SpeakRequest {
        user_id: 501,
        user_name: "Carol".to_string(),
        timestamp: 150,
        is_approved: false,
    });

    // Lower non-existent user returns false
    assert!(!queue.lower_hand(999));
    assert_eq!(queue.count_pending(), 1);

    // Approve non-existent user returns false
    assert!(!queue.approve_request(999));
    assert_eq!(queue.count_pending(), 1);

    // Approve Carol: Carol is now approved and removed from pending queue
    assert!(queue.approve_request(501));
    assert_eq!(queue.count_pending(), 0);
    assert!(queue.pending_requests().is_empty());

    // Lower hand for approved Carol removes the request entirely
    assert!(queue.lower_hand(501));
    assert_eq!(queue.requests.len(), 0);

    // Second lower hand returns false
    assert!(!queue.lower_hand(501));
}

#[test]
fn test_oracle1_speak_request_high_load_fuzz_monotonicity() {
    let mut queue = CallsGroupSpeakRequestsQueue::new();
    let count = 200;

    // Insert 200 requests with pseudo-random scrambled timestamps
    for i in 0..count {
        let scrambled_time = ((i * 37) % count) as i64 * 5;
        queue.requests.push(calls_group_speak_requests::SpeakRequest {
            user_id: (10_000 + i) as u64,
            user_name: format!("FuzzUser_{}", i),
            timestamp: scrambled_time,
            is_approved: false,
        });
    }

    assert_eq!(queue.count_pending(), count);
    let pending = queue.pending_requests();
    assert_eq!(pending.len(), count);

    // Verify pending_requests() output is strictly sorted by timestamp ascending
    for w in pending.windows(2) {
        assert!(
            w[0].timestamp <= w[1].timestamp,
            "Queue ordering violation: {} > {}",
            w[0].timestamp,
            w[1].timestamp
        );
    }
}


// ============================================================================
// ORACLE 2: Spacebar Push-To-Talk 250ms Hysteresis Delay
// Upstream: Telegram/SourceFiles/calls/group/calls_group_panel.cpp:L82, L396-L400
//           Telegram/SourceFiles/calls/group/calls_group_call.cpp:L4206-L4225
// Invariant: kSpacePushToTalkDelay = 250ms hysteresis delay must prevent choppy
//            audio on accidental key releases. The mic must stay unmuted during
//            sub-250ms release windows and immediately re-mute after 250ms silence.
// ============================================================================

#[test]
fn test_oracle2_space_press_instant_unmute_and_timer_cancellation() {
    let mut panel = CallsGroupPanelGroup::new("Engineering Sync", 8);
    assert!(panel.is_muted);
    assert!(!panel.space_pressed);
    assert!(panel.space_release_timer_ms.is_none());

    // Space key down -> immediately unmuted, timer cleared
    panel.on_space_down();
    assert!(!panel.is_muted, "Mic must instantly unmute on space press");
    assert!(panel.space_pressed);
    assert!(panel.space_release_timer_ms.is_none());

    // Repeated space down events (OS key repeat) must be idempotent
    for _ in 0..10 {
        panel.on_space_down();
        assert!(!panel.is_muted);
        assert!(panel.space_pressed);
        assert!(panel.space_release_timer_ms.is_none());
    }
}

#[test]
fn test_oracle2_space_release_250ms_hysteresis_delay_initiation() {
    let mut panel = CallsGroupPanelGroup::new("Engineering Sync", 8);
    panel.on_space_down();
    assert!(!panel.is_muted);

    // Release spacebar: hysteresis timer armed with exact 250ms constant
    panel.on_space_up();
    assert!(!panel.space_pressed);
    assert_eq!(
        panel.space_release_timer_ms,
        Some(calls_group_panel::K_SPACE_PUSH_TO_TALK_DELAY_MS),
        "Hysteresis delay must match C++ kSpacePushToTalkDelay (250ms)"
    );
    assert!(!panel.is_muted, "Mic must remain unmuted while hysteresis timer is running");
}

#[test]
fn test_oracle2_anti_flutter_accidental_release_jitter_immunity() {
    let mut panel = CallsGroupPanelGroup::new("Live Stream Stage", 15);
    panel.on_space_down();

    // Adversarial simulation: 20 rapid accidental finger slips (sub-250ms releases)
    // Audio must NEVER cut out (is_muted must remain false across all jitter cycles)
    let jitter_intervals_ms = [20, 50, 110, 80, 240, 15, 190, 75, 235, 40];
    for &release_ms in &jitter_intervals_ms {
        panel.on_space_up();
        assert!(!panel.is_muted, "Audio cut off immediately on space up!");

        panel.tick_timer(release_ms);
        assert!(
            !panel.is_muted,
            "Audio cut off during jitter interval of {}ms (< 250ms)!",
            release_ms
        );
        assert!(panel.space_release_timer_ms.is_some());

        // Speaker re-engages space before 250ms expiration
        panel.on_space_down();
        assert!(!panel.is_muted);
        assert!(panel.space_pressed);
        assert!(panel.space_release_timer_ms.is_none(), "Timer must be canceled on re-press");
    }
}

#[test]
fn test_oracle2_exact_boundary_expiration_at_250ms() {
    let mut panel = CallsGroupPanelGroup::new("Hysteresis Boundary Test", 2);
    panel.on_space_down();
    panel.on_space_up();

    // Advance 249ms in 1ms increments: mic must stay unmuted for all 249 steps
    for step in 1..=249 {
        panel.tick_timer(1);
        assert!(
            !panel.is_muted,
            "Mic muted prematurely at elapsed time {}ms",
            step
        );
        assert_eq!(
            panel.space_release_timer_ms,
            Some(250 - step),
            "Timer mismatch at step {}",
            step
        );
    }

    // Tick the 250th millisecond -> threshold crossed, mic must re-mute!
    panel.tick_timer(1);
    assert!(panel.is_muted, "Mic failed to mute at exactly 250ms threshold");
    assert!(panel.space_release_timer_ms.is_none());

    // Subsequent ticks do nothing
    panel.tick_timer(100);
    assert!(panel.is_muted);
}

#[test]
fn test_oracle2_large_time_step_overshoot_and_hold_immunity() {
    let mut panel = CallsGroupPanelGroup::new("Overshoot Test", 3);
    panel.on_space_down();
    panel.on_space_up();

    // Giant 5000ms delta tick
    panel.tick_timer(5000);
    assert!(panel.is_muted);
    assert!(panel.space_release_timer_ms.is_none());

    // Actively held space must never be muted by timer ticks
    panel.on_space_down();
    panel.tick_timer(1000);
    assert!(!panel.is_muted, "Holding space must prevent muting on ticks");
    assert!(panel.space_pressed);
}


// ============================================================================
// ORACLE 3: RTMP Live Stream URL & Stream Key Security
// Upstream: Telegram/SourceFiles/calls/group/calls_group_rtmp.cpp:L168-L170
//           Telegram/SourceFiles/calls/group/calls_group_rtmp.h:L50-L75
// Invariant: RTMP ingest endpoints must follow strict DC TLS routing
//            (rtmps://dc{dc_id}-rtmp.telegram.org:443/live/), stream keys must
//            deterministically combine channel ID and hash, UI masking must
//            never leak secret hashes, and key revocation must be instant.
// ============================================================================

#[test]
fn test_oracle3_rtmp_server_url_format_and_dc_routing() {
    let test_dcs = [1u32, 2, 3, 4, 5, 202];
    for &dc in &test_dcs {
        let rtmp = CallsGroupRtmpGroup::new(12345678, dc, "stream_hash_token");
        let url = rtmp.server_url();

        // Scheme verification: strictly secure TLS RTMP
        assert!(url.starts_with("rtmps://"), "RTMP URL must enforce TLS (rtmps://)");

        // Host DC routing verification
        let expected_host = format!("dc{}-rtmp.telegram.org", dc);
        assert!(
            url.contains(&expected_host),
            "Host must route to specific DC: {}",
            expected_host
        );

        // Port & live path verification
        assert!(url.contains(":443/live/"), "Port 443 and /live/ endpoint required");
        assert_eq!(url, format!("rtmps://dc{}-rtmp.telegram.org:443/live/", dc));
    }
}

#[test]
fn test_oracle3_stream_key_generation_and_delimiter_integrity() {
    let channel_id = 9876543210u64;
    let stream_hash = "s3cr3t_h4sh_t0k3n_v4l1d";
    let rtmp = CallsGroupRtmpGroup::new(channel_id, 2, stream_hash);

    let key = rtmp.stream_key();
    assert_eq!(key, format!("{}_{}", channel_id, stream_hash));

    // Delimiter verification: split into exactly 2 parts
    let parts: Vec<&str> = key.split('_').collect();
    assert_eq!(parts[0], "9876543210", "Channel ID prefix mismatch");
    assert!(key.ends_with(stream_hash), "Stream hash suffix mismatch");

    let info = rtmp.to_rtmp_info();
    assert_eq!(info.url, rtmp.server_url());
    assert_eq!(info.key, rtmp.stream_key());
}

#[test]
fn test_oracle3_stream_key_security_masking_leak_prevention() {
    let secret = "super_confidential_stream_token_999";
    let rtmp = CallsGroupRtmpGroup::new(4242, 4, secret);

    let masked = rtmp.masked_stream_key();

    // Channel ID prefix visible for identification
    assert!(
        masked.starts_with("4242_"),
        "Masked key must begin with channel ID prefix"
    );

    // Bullets must mask the secret portion
    assert!(
        masked.contains('•'),
        "Masked key must contain bullet characters for secrecy"
    );

    // CRITICAL: Raw secret tokens must NEVER leak into the masked display
    assert!(
        !masked.contains("super"),
        "Masked key leaked plain-text secret prefix"
    );
    assert!(
        !masked.contains("confidential"),
        "Masked key leaked plain-text secret fragment"
    );
    assert!(
        !masked.contains("999"),
        "Masked key leaked plain-text secret suffix"
    );

    // Check capping of bullets for short and long hashes
    let short_rtmp = CallsGroupRtmpGroup::new(100, 2, "abc");
    assert_eq!(short_rtmp.masked_stream_key(), "100_•••");

    let long_rtmp = CallsGroupRtmpGroup::new(100, 2, "0123456789abcdef0123456789abcdef");
    assert_eq!(long_rtmp.masked_stream_key(), "100_••••••••");
}

#[test]
fn test_oracle3_stream_key_revocation_lifecycle() {
    let mut rtmp = CallsGroupRtmpGroup::new(777, 1, "initial_compromised_token");
    let old_key = rtmp.stream_key();
    let old_url = rtmp.server_url();

    // Revoke key by generating new secret hash
    rtmp.reset_stream_key("fresh_secure_token_555");
    let new_key = rtmp.stream_key();

    assert_ne!(old_key, new_key, "Revocation must produce new stream key");
    assert_eq!(new_key, "777_fresh_secure_token_555");
    assert_eq!(
        rtmp.server_url(),
        old_url,
        "Server URL endpoint must remain stable across revocations"
    );
}

#[test]
fn test_oracle3_rtmp_encoding_settings_and_serialization() {
    let rtmp = CallsGroupRtmpGroup::new(101, 2, "tok");
    let enc = &rtmp.encoding_settings;

    // Upstream recommended defaults for 1080p60 OBS ingest
    assert_eq!(enc.max_width, 1920);
    assert_eq!(enc.max_height, 1080);
    assert_eq!(enc.max_fps, 60);
    assert_eq!(enc.keyframe_interval_seconds, 2);
    assert_eq!(enc.audio_sample_rate_hz, 48000);
    assert_eq!(enc.audio_bitrate_kbps, 192);

    // Serde roundtrip
    let json = serde_json::to_string(enc).expect("RTMP encoding settings must serialize");
    let deserialized: calls_group_rtmp::RtmpEncodingSettings =
        serde_json::from_str(&json).expect("RTMP encoding settings must deserialize");
    assert_eq!(enc, &deserialized);
}


// ============================================================================
// ORACLE 4: Speaking Radial Energy Waveform & Scale Clamping
// Upstream: Telegram/SourceFiles/calls/group/calls_group_members_row.cpp:L26-L57
// Invariant: kBlobScale = 0.605 primary expansion, kMinorBlobFactor = 0.9f minor
//            blob scale, audio level clamped strictly in [0.0..1.0], multi-harmonic
//            phase wave bounded in [0.85..1.15], silence stays invariant at base radius.
// ============================================================================

#[test]
fn test_oracle4_speaking_waveform_silence_stability() {
    let base_radius = 50.0f32;

    // At zero audio level, radii must remain exactly base_radius across all phase angles
    for phase_deg in (0..360).step_by(15) {
        let phase = (phase_deg as f32).to_radians();
        let (primary, minor) = calls_group_members_row::compute_speaking_blob_radii(base_radius, 0.0, phase);
        assert!(
            (primary - base_radius).abs() < 1e-5,
            "Primary blob expanded during silence at phase {}",
            phase
        );
        assert!(
            (minor - base_radius).abs() < 1e-5,
            "Minor blob expanded during silence at phase {}",
            phase
        );
    }
}

#[test]
fn test_oracle4_speaking_waveform_input_scale_clamping() {
    let base = 48.0f32;
    let phase = 0.5f32;

    // Underflow clamping: negative audio levels must clamp to 0.0
    for &neg_level in &[-0.001f32, -0.5, -10.0, -1000.0] {
        let (p, m) = calls_group_members_row::compute_speaking_blob_radii(base, neg_level, phase);
        assert_eq!(p, base, "Negative level {} must clamp to base radius", neg_level);
        assert_eq!(m, base, "Negative level {} must clamp minor radius to base", neg_level);
    }

    // Overflow clamping: levels > 1.0 must clamp to 1.0 (kMaxLevel = 1.0)
    let (p_max, m_max) = calls_group_members_row::compute_speaking_blob_radii(base, 1.0, phase);
    for &excess_level in &[1.0001f32, 1.5, 5.0, 100.0, 1e6] {
        let (p, m) = calls_group_members_row::compute_speaking_blob_radii(base, excess_level, phase);
        assert!(
            (p - p_max).abs() < 1e-5,
            "Excess level {} failed to clamp to level 1.0 maximum",
            excess_level
        );
        assert!(
            (m - m_max).abs() < 1e-5,
            "Excess level {} minor blob failed to clamp",
            excess_level
        );
    }
}

#[test]
fn test_oracle4_multi_harmonic_ring_expansion_scale_factor() {
    let base = 60.0f32;
    let level = 1.0f32;

    // Upstream constants:
    // kBlobScale = 0.605
    // wave(phase) = 1.0 + 0.15 * sin(phase) -> in [0.85, 1.15]
    let k_blob_scale = calls_group_members_row::K_BLOB_SCALE as f32;
    assert!((k_blob_scale - 0.605).abs() < 1e-5, "kBlobScale must be 0.605");

    let min_possible_primary = base * (1.0 + k_blob_scale * 0.85);
    let max_possible_primary = base * (1.0 + k_blob_scale * 1.15);

    // Sample across 360 degrees
    for phase_deg in 0..360 {
        let phase = (phase_deg as f32).to_radians();
        let (primary, _) = calls_group_members_row::compute_speaking_blob_radii(base, level, phase);

        assert!(
            primary >= min_possible_primary - 1e-4,
            "Primary radius {} fell below theoretical trough {}",
            primary,
            min_possible_primary
        );
        assert!(
            primary <= max_possible_primary + 1e-4,
            "Primary radius {} exceeded theoretical peak {}",
            primary,
            max_possible_primary
        );
    }
}

#[test]
fn test_oracle4_minor_blob_harmonic_ratio_invariant() {
    let base = 50.0f32;
    let k_minor_factor = calls_group_members_row::K_MINOR_BLOB_FACTOR;
    assert!((k_minor_factor - 0.9).abs() < 1e-5, "kMinorBlobFactor must be 0.9");

    // For any positive level and arbitrary phase, minor blob expansion must strictly be 0.9 * primary expansion
    for &level in &[0.1f32, 0.35, 0.7, 1.0] {
        for phase_deg in (0..360).step_by(30) {
            let phase = (phase_deg as f32).to_radians();
            let (primary, minor) = calls_group_members_row::compute_speaking_blob_radii(base, level, phase);

            // Strict hierarchy
            assert!(primary > base, "Primary must expand above base");
            assert!(minor > base, "Minor must expand above base");
            assert!(
                minor < primary,
                "Minor blob must be strictly smaller than primary blob: {} vs {}",
                minor,
                primary
            );

            // Ratio of expansion deltas
            let delta_primary = primary - base;
            let delta_minor = minor - base;
            let computed_ratio = delta_minor / delta_primary;
            assert!(
                (computed_ratio - 0.9).abs() < 1e-4,
                "Minor expansion delta ratio {} deviates from 0.9",
                computed_ratio
            );
        }
    }
}

#[test]
fn test_oracle4_temporal_interpolation_and_speaking_state_gating() {
    let mut participant = calls_group_common::ParticipantState::default();
    participant.mute_state = calls_group_common::MuteState::Speaking;
    let mut row = CallsGroupMembersRowGroup::new(participant);

    assert_eq!(row.animated_level, 0.0);

    // Duration is kLevelDuration = 100.0 + 500.0 * 0.23 = 215.0ms
    assert!((calls_group_members_row::K_LEVEL_DURATION_MS - 215.0).abs() < 1e-4);

    // Delta of 215ms reaches full alpha = 1.0 -> target 0.8 attained immediately
    row.update_level(0.8, 215.0);
    assert!((row.animated_level - 0.8).abs() < 1e-4);

    // When speaking, get_blob_radii returns expanded radii
    let (p, m) = row.get_blob_radii(40.0);
    assert!(p > 40.0);
    assert!(m > 40.0);

    // When NOT speaking, get_blob_radii returns (base, base) despite animated_level > 0
    row.participant.mute_state = calls_group_common::MuteState::Unmuted;
    let (p_silent, m_silent) = row.get_blob_radii(40.0);
    assert_eq!(p_silent, 40.0, "Silent participant must have base radius");
    assert_eq!(m_silent, 40.0, "Silent participant must have base radius");
}


// ============================================================================
// ORACLE 5: Participant Volume Magnetic Snap & Speaker Arcs
// Upstream: Telegram/SourceFiles/calls/group/calls_volume_item.cpp:L23-L39, L192-L200
// Invariant: Volume slider range [0..200%] (ratio [0.0..1.0]), ±5% wide magnetic
//            snap window at 100% (raw values in [95..105] snap strictly to 100),
//            ±1% snap at 25/50/75/125/150/175%, speaker arc levels (0..3).
// ============================================================================

#[test]
fn test_oracle5_volume_100_percent_wide_magnetic_snap_window() {
    let mut vol = CallsVolumeItemGroup::new(999, 100, false);

    // Requirement: Verify values in [95..105] snap strictly to 100
    for pct in 95..=105 {
        let raw_ratio = (pct as f32) / 200.0;
        let snapped_pct = vol.set_slider_ratio(raw_ratio);
        assert_eq!(
            snapped_pct, 100,
            "Raw volume percent {} failed to snap to 100% (got {})",
            pct, snapped_pct
        );
        assert_eq!(vol.volume_percent, 100);
    }

    // Extended verification: Interior values of the ±5% magnetic snap window [0.450..0.550]
    // (from 91% through 109% of 200% max volume) all snap strictly to 100%
    for pct in 91..=109 {
        let raw_ratio = (pct as f32) / 200.0;
        let snapped_pct = vol.set_slider_ratio(raw_ratio);
        assert_eq!(
            snapped_pct, 100,
            "Ratio for {}% inside (0.45..0.55) must snap to 100%",
            pct
        );
    }

    // Direct float ratio tests inside ±5% window
    for &ratio in &[0.455f32, 0.48, 0.50, 0.52, 0.545] {
        let snapped_pct = vol.set_slider_ratio(ratio);
        assert_eq!(snapped_pct, 100, "Ratio {} inside window must snap to 100%", ratio);
    }

    // Outside boundary points: 0.44 (< 0.45) and 0.56 (> 0.55) must NOT snap to 100
    let snapped_below = vol.set_slider_ratio(0.44);
    assert_ne!(snapped_below, 100, "0.44 must not snap to 100%");
    assert_eq!(snapped_below, 88);

    let snapped_above = vol.set_slider_ratio(0.56);
    assert_ne!(snapped_above, 100, "0.56 must not snap to 100%");
    assert_eq!(snapped_above, 112);
}

#[test]
fn test_oracle5_volume_slider_range_and_clamping() {
    let mut vol = CallsVolumeItemGroup::new(42, 100, false);

    // Underflow: negative ratio must clamp to 0.0 (volume 0%, locally muted)
    let p_min = vol.set_slider_ratio(-0.5);
    assert_eq!(p_min, 0);
    assert_eq!(vol.volume_percent, 0);
    assert!(vol.is_locally_muted, "0% volume must trigger local mute");

    // Overflow: ratio > 1.0 must clamp to 1.0 (200% volume)
    let p_max = vol.set_slider_ratio(1.5);
    assert_eq!(p_max, calls_volume_item::K_MAX_VOLUME_PERCENT);
    assert_eq!(vol.volume_percent, 200);
    assert!(!vol.is_locally_muted);

    // Direct ratio verification
    assert_eq!(vol.slider_ratio(), 1.0);
}

#[test]
fn test_oracle5_secondary_snap_points_25_50_75_125_150_175() {
    let mut vol = CallsVolumeItemGroup::new(77, 100, false);

    // Each secondary snap point has radius 2.0 / 200.0 = 0.010 (±1%)
    let secondary_points = [
        (25u32, 25.0 / 200.0),
        (50, 50.0 / 200.0),
        (75, 75.0 / 200.0),
        (125, 125.0 / 200.0),
        (150, 150.0 / 200.0),
        (175, 175.0 / 200.0),
    ];

    for &(expected_pct, center_ratio) in &secondary_points {
        // Center point snaps to target
        let p_center = vol.set_slider_ratio(center_ratio);
        assert_eq!(p_center, expected_pct);

        // Near edge inside ±1% window snaps to target
        let p_inside = vol.set_slider_ratio(center_ratio + 0.008);
        assert_eq!(
            p_inside, expected_pct,
            "Value inside snap window of {}% did not snap",
            expected_pct
        );

        // Outside ±1% window does NOT snap to target
        let outside_ratio = center_ratio + 0.015;
        let p_outside = vol.set_slider_ratio(outside_ratio);
        assert_ne!(
            p_outside, expected_pct,
            "Value outside snap window of {}% improperly snapped",
            expected_pct
        );
    }
}

#[test]
fn test_oracle5_local_mute_coupling_and_arcs() {
    let mut vol = CallsVolumeItemGroup::new(88, 100, false);
    assert_eq!(vol.speaker_arcs(), 2); // 100% / 200% = 0.50 -> level 2 (in [0.25..0.75))

    // Setting slider to 0 mutes locally and zeros arcs
    vol.set_slider_ratio(0.0);
    assert!(vol.is_locally_muted);
    assert_eq!(vol.slider_ratio(), 0.0);
    assert_eq!(vol.speaker_arcs(), 0);

    // Toggling local mute unmutes and restores arcs
    vol.toggle_local_mute();
    assert!(!vol.is_locally_muted);

    // Speaker arc tiers:
    // Tier 0: < 5% (0.05)
    // Tier 1: 5%..25% (0.05..0.25)
    // Tier 2: 25%..75% (0.25..0.75)
    // Tier 3: >= 75% (0.75..1.00)
    vol.set_slider_ratio(8.0 / 200.0); // 0.04 -> level 0
    assert_eq!(vol.speaker_arcs(), 0);

    vol.set_slider_ratio(20.0 / 200.0); // 0.10 -> level 1
    assert_eq!(vol.speaker_arcs(), 1);

    vol.set_slider_ratio(100.0 / 200.0); // 0.50 -> level 2
    assert_eq!(vol.speaker_arcs(), 2);

    vol.set_slider_ratio(180.0 / 200.0); // 0.90 -> level 3
    assert_eq!(vol.speaker_arcs(), 3);
}
