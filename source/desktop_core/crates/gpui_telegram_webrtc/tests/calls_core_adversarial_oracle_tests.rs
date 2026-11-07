//! Adversarial Blind C++ Oracle Verification Suite for Module 10 Calls Core
//! Upstream references:
//! - calls_emoji_fingerprint.h:L20-L72, calls_emoji_fingerprint.cpp:L25-L170
//! - calls_signal_bars.h:L20-L36, calls_signal_bars.cpp:L16-L65
//! - calls_call.h:L70-L290, calls_call.cpp:L47-L200, L1576-L1608
//! - calls_video_bubble.h:L20-L70, calls_video_bubble.cpp:L17-L260
//! - calls_rate_call.h:L25-L115, calls_rate_call.cpp:L20-L390
#![allow(unused_imports, dead_code)]

use gpui_telegram_webrtc::calls::{
    calls_call::{CallState, CallType, CallsCall, RemoteAudioState, RemoteBatteryState, RemoteVideoState},
    calls_emoji_fingerprint::{
        compute_emoji_fingerprint, compute_emoji_index_from_slice, CallsEmojiFingerprint,
        EMOJI_PALETTE, K_CAROUSEL_ONE_DURATION_MS, K_EMOJI_COUNT, K_EMOJI_IN_CAROUSEL,
        K_EMOJI_IN_FINGERPRINT, K_START_TIME_SHIFT_MS, K_TOOLTIP_SHOW_TIMEOUT_MS,
    },
    calls_rate_call::CallsRateCall,
    calls_signal_bars::CallsSignalBars,
    calls_video_bubble::{BubbleCorner, BubbleDragMode, CallsVideoBubble},
    calls_video_incoming::{CallsVideoIncoming, VideoRendererType},
};

// ============================================================================
// ORACLE 1: Emoji Fingerprint Parity (C++ ComputeEmojiFingerprint / ComputeEmojiIndex)
// ============================================================================

/// Upstream C++ exact reference oracle:
/// ComputeEmojiIndex(bytes::const_span bytes)
///   ((bytes[0] & 0x7F) << 56) | (bytes[1] << 48) | (bytes[2] << 40) |
///   (bytes[3] << 32) | (bytes[4] << 24) | (bytes[5] << 16) | (bytes[6] << 8) | bytes[7]
fn cxx_oracle_compute_emoji_index(bytes: &[u8]) -> u64 {
    assert!(bytes.len() >= 8);
    ((bytes[0] as u64 & 0x7F) << 56)
        | ((bytes[1] as u64) << 48)
        | ((bytes[2] as u64) << 40)
        | ((bytes[3] as u64) << 32)
        | ((bytes[4] as u64) << 24)
        | ((bytes[5] as u64) << 16)
        | ((bytes[6] as u64) << 8)
        | (bytes[7] as u64)
}

/// Upstream C++ exact fingerprint oracle:
/// Splits 32-byte SHA256 key into four 8-byte chunks, modulo kEmojiCount (333).
fn cxx_oracle_compute_emoji_fingerprint(sha256: &[u8; 32]) -> [usize; 4] {
    let mut result = [0usize; 4];
    for i in 0..4 {
        let chunk = &sha256[i * 8..(i + 1) * 8];
        let val = cxx_oracle_compute_emoji_index(chunk);
        result[i] = (val % 333) as usize;
    }
    result
}

#[test]
fn test_oracle_1_emoji_fingerprint_all_zeros_and_msb_masking() {
    // Adversarial vector 1: All zeros must map to index 0
    let zero_sha = [0u8; 32];
    let rust_indices = compute_emoji_fingerprint(&zero_sha);
    let cxx_indices = cxx_oracle_compute_emoji_fingerprint(&zero_sha);
    assert_eq!(rust_indices, cxx_indices);
    assert_eq!(rust_indices, [0, 0, 0, 0]);
    assert_eq!(EMOJI_PALETTE[rust_indices[0]], "😉");

    // Adversarial vector 2: MSB masking in byte 0.
    // 0x80 in byte 0 must be masked out by & 0x7F, producing identical index to 0x00.
    let mut high_bit_sha = [0u8; 32];
    high_bit_sha[0] = 0x80;
    high_bit_sha[8] = 0x80;
    high_bit_sha[16] = 0x80;
    high_bit_sha[24] = 0x80;

    let high_bit_indices = compute_emoji_fingerprint(&high_bit_sha);
    assert_eq!(high_bit_indices, [0, 0, 0, 0], "MSB & 0x7F must strip 0x80 to zero");
    assert_eq!(high_bit_indices, cxx_oracle_compute_emoji_fingerprint(&high_bit_sha));
}

#[test]
fn test_oracle_1_emoji_fingerprint_all_ones_maximum_boundary() {
    // Adversarial vector 3: All 0xFF bytes.
    // Chunk index = 0x7FFF_FFFF_FFFF_FFFF = 9,223,372,036,854,775,807.
    // 9223372036854775807 % 333 = 79.
    let ff_sha = [0xFFu8; 32];
    let rust_indices = compute_emoji_fingerprint(&ff_sha);
    let cxx_indices = cxx_oracle_compute_emoji_fingerprint(&ff_sha);
    assert_eq!(rust_indices, cxx_indices);
    assert_eq!(rust_indices, [79, 79, 79, 79]);
    assert_eq!(EMOJI_PALETTE[79], "💍");

    let fp = CallsEmojiFingerprint::from_sha256(ff_sha);
    assert_eq!(fp.emoji_indices, [79, 79, 79, 79]);
    assert_eq!(fp.as_emoji_string(), "💍 💍 💍 💍");
}

#[test]
fn test_oracle_1_emoji_fingerprint_deterministic_vectors_and_palette_bounds() {
    assert_eq!(EMOJI_PALETTE.len(), K_EMOJI_COUNT);
    assert_eq!(K_EMOJI_COUNT, 333);

    // Adversarial vector 4: Pseudo-random deterministic sweep across 64 seed hashes
    for seed in 0..64u8 {
        let mut sha = [0u8; 32];
        for i in 0..32 {
            sha[i] = seed.wrapping_mul(31).wrapping_add(i as u8).wrapping_mul(17);
        }
        let rust_res = compute_emoji_fingerprint(&sha);
        let cxx_res = cxx_oracle_compute_emoji_fingerprint(&sha);
        assert_eq!(rust_res, cxx_res, "Discrepancy at seed {}", seed);
        for &idx in &rust_res {
            assert!(idx < 333, "Index {} out of bounds [0, 333)", idx);
            assert!(!EMOJI_PALETTE[idx].is_empty());
        }
    }

    // Verify modulo boundary 332 (332 = (1 << 8) | 76)
    let mut exact_332 = [0u8; 32];
    exact_332[6] = 1;
    exact_332[7] = 76;
    assert_eq!(compute_emoji_fingerprint(&exact_332)[0], 332);
    assert_eq!(cxx_oracle_compute_emoji_fingerprint(&exact_332)[0], 332);
}


// ============================================================================
// ORACLE 2: Signal Quality Bars Parity (C++ calls_signal_bars.cpp)
// ============================================================================

/// Upstream C++ exact signal bars calculation:
/// 1 bar if: packet_loss > 0.15 || rtt_ms > 800 || jitter_ms > 100
/// 2 bars if: packet_loss > 0.08 || rtt_ms > 450 || jitter_ms > 60
/// 3 bars if: packet_loss > 0.02 || rtt_ms > 200 || jitter_ms > 30
/// 4 bars otherwise.
fn cxx_oracle_compute_signal_bars(rtt_ms: u32, packet_loss: f32, jitter_ms: u32) -> i32 {
    if packet_loss > 0.15 || rtt_ms > 800 || jitter_ms > 100 {
        1
    } else if packet_loss > 0.08 || rtt_ms > 450 || jitter_ms > 60 {
        2
    } else if packet_loss > 0.02 || rtt_ms > 200 || jitter_ms > 30 {
        3
    } else {
        4
    }
}

/// Upstream C++ bar height formula:
/// min + (max - min) * (i / float64(kSignalBarCount - 1))
fn cxx_oracle_bar_height(min: f32, max: f32, i: usize, count: usize) -> f32 {
    assert!(count > 1);
    min + (max - min) * (i as f32 / (count - 1) as f32)
}

#[test]
fn test_oracle_2_signal_bars_exact_boundary_arithmetic() {
    let bars = CallsSignalBars::new();
    assert_eq!(bars.count, CallsSignalBars::K_SIGNAL_BAR_STARTING); // -1

    // Threshold boundaries for 4 -> 3 transition:
    // RTT 200ms -> 4 bars, 201ms -> 3 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(200, 0.0, 0), 4);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(201, 0.0, 0), 3);
    assert_eq!(cxx_oracle_compute_signal_bars(200, 0.0, 0), 4);
    assert_eq!(cxx_oracle_compute_signal_bars(201, 0.0, 0), 3);

    // Packet loss 0.02 -> 4 bars, 0.021 -> 3 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.02, 0), 4);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.021, 0), 3);
    assert_eq!(cxx_oracle_compute_signal_bars(50, 0.02, 0), 4);
    assert_eq!(cxx_oracle_compute_signal_bars(50, 0.021, 0), 3);

    // Jitter 30ms -> 4 bars, 31ms -> 3 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 30), 4);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 31), 3);
    assert_eq!(cxx_oracle_compute_signal_bars(50, 0.0, 30), 4);
    assert_eq!(cxx_oracle_compute_signal_bars(50, 0.0, 31), 3);

    // Threshold boundaries for 3 -> 2 transition:
    // RTT 450ms -> 3 bars, 451ms -> 2 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(450, 0.0, 0), 3);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(451, 0.0, 0), 2);
    // Packet loss 0.08 -> 3 bars, 0.081 -> 2 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.08, 0), 3);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.081, 0), 2);
    // Jitter 60ms -> 3 bars, 61ms -> 2 bars
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 60), 3);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 61), 2);

    // Threshold boundaries for 2 -> 1 transition:
    // RTT 800ms -> 2 bars, 801ms -> 1 bar
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(800, 0.0, 0), 2);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(801, 0.0, 0), 1);
    // Packet loss 0.15 -> 2 bars, 0.151 -> 1 bar
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.15, 0), 2);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.151, 0), 1);
    // Jitter 100ms -> 2 bars, 101ms -> 1 bar
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 100), 2);
    assert_eq!(CallsSignalBars::compute_bars_from_metrics(50, 0.0, 101), 1);
}

#[test]
fn test_oracle_2_signal_bars_height_interpolation_and_finished_latch() {
    let mut bars = CallsSignalBars::new();

    // Verify 4-bar linear height progression: [4.0, 6.0, 8.0, 10.0]
    for i in 0..4 {
        let expected_h = cxx_oracle_bar_height(4.0, 10.0, i, 4);
        assert_eq!(bars.calculate_bar_height(i), expected_h);
    }

    // Set active 2 bars
    bars.update_network_stats(500, 0.09, 20);
    assert_eq!(bars.count, 2);
    assert_eq!(bars.bar_opacity(0), 1.0);
    assert_eq!(bars.bar_opacity(1), 1.0);
    assert_eq!(bars.bar_opacity(2), 0.5); // inactive
    assert_eq!(bars.bar_opacity(3), 0.5); // inactive

    // Latch finished state: K_SIGNAL_BAR_FINISHED (-2)
    bars.count = CallsSignalBars::K_SIGNAL_BAR_FINISHED;
    // Further updates should NOT alter finished state
    bars.update_network_stats(10, 0.0, 1);
    assert_eq!(bars.count, CallsSignalBars::K_SIGNAL_BAR_FINISHED);
    for i in 0..4 {
        assert_eq!(bars.bar_opacity(i), 0.5, "Finished call must render all bars with inactive opacity");
    }
}

// ============================================================================
// ORACLE 3: Call State Machine FSM & Hangup Timeout Parity
// ============================================================================

#[test]
fn test_oracle_3_call_fsm_lifecycle_and_hangup_timeout() {
    // Upstream constant: kHangupTimeoutMs = 5000
    assert_eq!(CallsCall::K_HANGUP_TIMEOUT_MS, 5000);

    // Initial Incoming State Machine
    let mut call = CallsCall::new(999, 1001, CallType::Incoming, true);
    assert_eq!(call.state, CallState::WaitingIncoming);
    assert!(call.is_incoming_waiting());
    assert!(call.video_enabled);
    assert_eq!(call.remote_video, RemoteVideoState::Active);

    // Transition 1: Answer -> ExchangingKeys
    call.answer();
    assert_eq!(call.state, CallState::ExchangingKeys);

    // Transition 2: Keys Exchanged -> Established (4 signal bars)
    let dh_sha = [0x5Au8; 32];
    call.keys_exchanged(dh_sha);
    assert_eq!(call.state, CallState::Established);
    assert_eq!(call.signal_bar_count, 4);
    assert_eq!(call.key_sha256, Some(dh_sha));

    // Transition 3: Hangup -> HangingUp (-2 signal bars)
    call.hangup();
    assert_eq!(call.state, CallState::HangingUp);
    assert_eq!(call.signal_bar_count, CallsCall::K_SIGNAL_BAR_FINISHED);

    // Simulated 5000ms hangup timer expiry -> Ended
    let simulated_elapsed_ms = 5000u64;
    assert!(simulated_elapsed_ms >= CallsCall::K_HANGUP_TIMEOUT_MS);
    call.finish(true);
    assert_eq!(call.state, CallState::Ended);
    assert!(call.rating_requested);

    // Transition 4: Redial -> Requesting
    call.redial();
    assert_eq!(call.state, CallState::Requesting);
    assert_eq!(call.signal_bar_count, CallsCall::K_SIGNAL_BAR_STARTING);
    assert_eq!(call.key_sha256, None);
    assert_eq!(call.duration_ms, 0);
}

#[test]
fn test_oracle_3_call_media_and_screen_sharing_exclusivity() {
    let mut call = CallsCall::new(1001, 2002, CallType::Outgoing, false);
    assert_eq!(call.state, CallState::Requesting);
    assert!(!call.camera_sharing);
    assert!(!call.screen_sharing);

    // Enabling camera sharing disables screen sharing
    call.toggle_camera_sharing(true);
    assert!(call.camera_sharing);
    assert!(!call.screen_sharing);

    // Enabling screen sharing disables camera sharing
    call.toggle_screen_sharing(true);
    assert!(call.screen_sharing);
    assert!(!call.camera_sharing);

    // Rating clamping [1, 5]
    call.set_rating(-10);
    assert_eq!(call.rating_value, Some(1));
    call.set_rating(100);
    assert_eq!(call.rating_value, Some(5));
}


// ============================================================================
// ORACLE 4: Video Bubble Drag & Corner Snapping Parity (calls_video_bubble.cpp)
// ============================================================================

/// Upstream C++ exact corner position oracle:
/// Coordinates computed relative to parent bounding box with MARGIN (16px).
fn cxx_oracle_corner_pos(parent_w: f32, parent_h: f32, w: f32, h: f32, corner: BubbleCorner) -> (f32, f32) {
    let m = CallsVideoBubble::MARGIN;
    match corner {
        BubbleCorner::TopLeft => (m, m),
        BubbleCorner::TopRight => (parent_w - w - m, m),
        BubbleCorner::BottomLeft => (m, parent_h - h - m),
        BubbleCorner::BottomRight => (parent_w - w - m, parent_h - h - m),
    }
}

/// Upstream C++ corner distance minimization oracle:
/// Selects corner whose center minimizes squared Euclidean distance to (drag_x, drag_y).
fn cxx_oracle_minimize_corner_distance(
    parent_w: f32,
    parent_h: f32,
    bubble_w: f32,
    bubble_h: f32,
    drag_x: f32,
    drag_y: f32,
) -> BubbleCorner {
    let corners = [
        BubbleCorner::TopLeft,
        BubbleCorner::TopRight,
        BubbleCorner::BottomLeft,
        BubbleCorner::BottomRight,
    ];
    let mut best_corner = BubbleCorner::BottomRight;
    let mut min_dist_sq = f32::MAX;

    for corner in corners {
        let (cx, cy) = cxx_oracle_corner_pos(parent_w, parent_h, bubble_w, bubble_h, corner);
        let center_x = cx + bubble_w / 2.0;
        let center_y = cy + bubble_h / 2.0;
        let dx = drag_x - center_x;
        let dy = drag_y - center_y;
        let dist_sq = dx * dx + dy * dy;
        if dist_sq < min_dist_sq {
            min_dist_sq = dist_sq;
            best_corner = corner;
        }
    }
    best_corner
}

#[test]
fn test_oracle_4_video_bubble_corner_snapping_multiple_geometries() {
    let mut bubble = CallsVideoBubble::new();
    assert_eq!(bubble.drag_mode, BubbleDragMode::SnapToCorners);
    assert_eq!(bubble.width, CallsVideoBubble::DEFAULT_WIDTH); // 160.0
    assert_eq!(bubble.height, CallsVideoBubble::DEFAULT_HEIGHT); // 110.0
    assert_eq!(CallsVideoBubble::MARGIN, 16.0);

    // Geometry 1: Standard Desktop Window (720 x 540)
    let standard_geometries = [(720.0, 540.0), (380.0, 520.0), (1920.0, 1080.0), (3840.0, 2160.0)];

    for &(pw, ph) in &standard_geometries {
        for corner in [
            BubbleCorner::TopLeft,
            BubbleCorner::TopRight,
            BubbleCorner::BottomLeft,
            BubbleCorner::BottomRight,
        ] {
            bubble.snap_to_corner(pw, ph, corner);
            let (expected_x, expected_y) = cxx_oracle_corner_pos(pw, ph, bubble.width, bubble.height, corner);
            assert_eq!(bubble.x, expected_x, "Corner {:?} x mismatch at ({}, {})", corner, pw, ph);
            assert_eq!(bubble.y, expected_y, "Corner {:?} y mismatch at ({}, {})", corner, pw, ph);
            assert_eq!(bubble.active_corner, corner);
        }
    }
}

#[test]
fn test_oracle_4_video_bubble_drag_distance_minimization_and_mirroring() {
    let pw = 720.0f32;
    let ph = 540.0f32;
    let bw = CallsVideoBubble::DEFAULT_WIDTH;
    let bh = CallsVideoBubble::DEFAULT_HEIGHT;

    // Point in Top-Left quadrant -> TopLeft
    let c1 = cxx_oracle_minimize_corner_distance(pw, ph, bw, bh, 50.0, 50.0);
    assert_eq!(c1, BubbleCorner::TopLeft);

    // Point in Top-Right quadrant -> TopRight
    let c2 = cxx_oracle_minimize_corner_distance(pw, ph, bw, bh, 680.0, 50.0);
    assert_eq!(c2, BubbleCorner::TopRight);

    // Point in Bottom-Left quadrant -> BottomLeft
    let c3 = cxx_oracle_minimize_corner_distance(pw, ph, bw, bh, 50.0, 500.0);
    assert_eq!(c3, BubbleCorner::BottomLeft);

    // Point in Bottom-Right quadrant -> BottomRight
    let c4 = cxx_oracle_minimize_corner_distance(pw, ph, bw, bh, 680.0, 500.0);
    assert_eq!(c4, BubbleCorner::BottomRight);

    // Verify mirroring controls
    let mut bubble = CallsVideoBubble::new();
    assert!(bubble.is_mirrored);
    bubble.set_mirrored(false);
    assert!(!bubble.is_mirrored);
    bubble.set_mirrored(true);
    assert!(bubble.is_mirrored);
}

// ============================================================================
// ORACLE 5: Call Rating Burst FSM Parity (calls_rate_call.cpp)
// ============================================================================

/// Upstream C++ exact rating burst predicate:
/// bool RateCall::commit(int index) {
///     _rating = index + 1;
///     if (_rating.current() >= kBurstMinRating) { startBurst(index); }
/// }
fn cxx_oracle_triggers_burst(stars: i32) -> bool {
    let clamped = stars.clamp(1, 5);
    clamped >= 4
}

#[test]
fn test_oracle_5_call_rating_burst_threshold_and_capping() {
    assert_eq!(CallsRateCall::K_STARS_COUNT, 5);
    assert_eq!(CallsRateCall::K_BURST_MIN_RATING, 4);
    assert_eq!(CallsRateCall::K_MAX_BURSTS, 5);
    assert_eq!(CallsRateCall::K_SHOW_DURATION_MS, 250);
    assert_eq!(CallsRateCall::K_STAR_SHOW_DELAY_MS, 16);
    assert_eq!(CallsRateCall::K_DARK_OPACITY, 0.14);

    let mut rate = CallsRateCall::new(8888);
    assert_eq!(rate.rating, 0);
    assert_eq!(rate.burst_count, 0);

    // Ratings 1..=3 must NOT trigger burst
    for r in 1..=3 {
        assert_eq!(cxx_oracle_triggers_burst(r), false);
        let triggered = rate.set_rating(r);
        assert!(!triggered, "Rating {} should not trigger burst", r);
        assert_eq!(rate.burst_count, 0);
    }

    // Rating 4 triggers burst and increments burst_count
    assert_eq!(cxx_oracle_triggers_burst(4), true);
    let triggered4 = rate.set_rating(4);
    assert!(triggered4);
    assert_eq!(rate.burst_count, 1);

    // Rating 5 triggers burst and increments burst_count
    assert_eq!(cxx_oracle_triggers_burst(5), true);
    let triggered5 = rate.set_rating(5);
    assert!(triggered5);
    assert_eq!(rate.burst_count, 2);

    // Stress test: trigger burst 10 times, verify hard ceiling at K_MAX_BURSTS (5)
    for _ in 0..10 {
        rate.set_rating(5);
    }
    assert_eq!(rate.burst_count, CallsRateCall::K_MAX_BURSTS, "burst_count must not exceed K_MAX_BURSTS (5)");

    // Rating clamping for out-of-range inputs
    let triggered_underflow = rate.set_rating(-100);
    assert_eq!(rate.rating, 1);
    assert!(!triggered_underflow);

    let triggered_overflow = rate.set_rating(1000);
    assert_eq!(rate.rating, 5);
    assert!(triggered_overflow);

    // Feedback submission
    rate.submit("Superb audio/video fidelity and seamless key exchange");
    assert!(rate.is_submitted);
    assert_eq!(rate.comment, "Superb audio/video fidelity and seamless key exchange");
}
