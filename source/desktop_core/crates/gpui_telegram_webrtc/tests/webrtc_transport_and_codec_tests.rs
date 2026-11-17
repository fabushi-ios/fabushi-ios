use gpui_telegram_webrtc::calls::peer_transport::{
    PeerConnectionState, PeerTransportConfig, TransportMode, VideoFrameData, WebRtcPeerTransport,
};
use gpui_telegram_webrtc::calls::engine::WebRtcCallEngine;
use gpui_telegram_webrtc::media::ai_media_tap::{AiMediaTap, DefaultAiMediaTap};
use gpui_telegram_webrtc::media::opus_codec::{
    CodecError, OpusBandwidth, OpusDepacketizer, OpusFrameHeader, OpusFramingCodec,
    OpusPacketizer, RtpPacket, OPUS_PAYLOAD_TYPE, OPUS_SAMPLE_RATE, OPUS_SAMPLES_PER_FRAME_MONO,
    OPUS_SAMPLES_PER_FRAME_STEREO,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

#[test]
fn test_opus_frame_header_parsing_and_serialization() {
    // Config 20, mono (bit 2 = 0), frame count 0 (bits 1..0 = 0)
    // 20 << 3 = 160 = 0xA0
    let toc_mono = 0xA0;
    let header_mono = OpusFrameHeader::parse(toc_mono);
    assert_eq!(header_mono.config, 20);
    assert!(!header_mono.stereo);
    assert_eq!(header_mono.bandwidth, OpusBandwidth::Fullband);
    assert_eq!(header_mono.to_toc_byte(), toc_mono);

    // Config 20, stereo (bit 2 = 1) -> 0xA4
    let toc_stereo = 0xA4;
    let header_stereo = OpusFrameHeader::parse(toc_stereo);
    assert_eq!(header_stereo.config, 20);
    assert!(header_stereo.stereo);
    assert_eq!(header_stereo.bandwidth, OpusBandwidth::Fullband);
    assert_eq!(header_stereo.to_toc_byte(), toc_stereo);
}

#[test]
fn test_rtp_packet_serialization_and_deserialization() {
    let payload = vec![0x11, 0x22, 0x33, 0x44, 0x55];
    let packet = RtpPacket::new(OPUS_PAYLOAD_TYPE, 1234, 48000, 0xCAFEBABE, payload.clone());

    let serialized = packet.serialize();
    assert_eq!(serialized.len(), 12 + payload.len());

    let parsed = RtpPacket::parse(&serialized).expect("RTP parsing failed");
    assert_eq!(parsed.version, 2);
    assert_eq!(parsed.payload_type, OPUS_PAYLOAD_TYPE);
    assert_eq!(parsed.sequence_number, 1234);
    assert_eq!(parsed.timestamp, 48000);
    assert_eq!(parsed.ssrc, 0xCAFEBABE);
    assert_eq!(parsed.payload, payload);
}

#[test]
fn test_opus_codec_roundtrip_mono_960_samples() {
    // Synthetic 20ms mono sine wave (960 samples @ 48kHz)
    let mut original_samples = Vec::with_capacity(OPUS_SAMPLES_PER_FRAME_MONO);
    for i in 0..OPUS_SAMPLES_PER_FRAME_MONO {
        let phase = 2.0 * std::f32::consts::PI * 440.0 * (i as f32) / (OPUS_SAMPLE_RATE as f32);
        original_samples.push((phase.sin() * 16384.0) as i16);
    }

    let encoded = OpusFramingCodec::encode_frame(&original_samples, 1)
        .expect("Mono encoding should succeed");
    assert_eq!(encoded.len(), 1 + OPUS_SAMPLES_PER_FRAME_MONO * 2);

    let decoded = OpusFramingCodec::decode_frame(&encoded, 1)
        .expect("Mono decoding should succeed");
    assert_eq!(decoded.len(), OPUS_SAMPLES_PER_FRAME_MONO);
    assert_eq!(decoded, original_samples);
}

#[test]
fn test_opus_codec_roundtrip_stereo_1920_samples() {
    // Synthetic 20ms stereo frame (1920 samples @ 48kHz)
    let mut original_samples = Vec::with_capacity(OPUS_SAMPLES_PER_FRAME_STEREO);
    for i in 0..OPUS_SAMPLES_PER_FRAME_STEREO {
        let phase = 2.0 * std::f32::consts::PI * 880.0 * (i as f32) / (OPUS_SAMPLE_RATE as f32);
        original_samples.push((phase.sin() * 8192.0) as i16);
    }

    let encoded = OpusFramingCodec::encode_frame(&original_samples, 2)
        .expect("Stereo encoding should succeed");
    assert_eq!(encoded.len(), 1 + OPUS_SAMPLES_PER_FRAME_STEREO * 2);

    let decoded = OpusFramingCodec::decode_frame(&encoded, 2)
        .expect("Stereo decoding should succeed");
    assert_eq!(decoded.len(), OPUS_SAMPLES_PER_FRAME_STEREO);
    assert_eq!(decoded, original_samples);
}

#[test]
fn test_opus_packetizer_streaming_boundary_accumulation() {
    let mut packetizer = OpusPacketizer::new(1, 0x12345678);

    // Push 500 samples: not enough for 960 (20ms) -> 0 packets
    let p1 = vec![100i16; 500];
    let packets = packetizer.push_pcm(&p1);
    assert!(packets.is_empty());

    // Push 460 samples: total 960 -> exactly 1 RTP packet produced
    let p2 = vec![100i16; 460];
    let packets = packetizer.push_pcm(&p2);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].sequence_number, 1000);
    assert_eq!(packets[0].timestamp, 0);

    // Push 1920 samples: exactly 2 RTP packets produced
    let p3 = vec![200i16; 1920];
    let packets = packetizer.push_pcm(&p3);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].sequence_number, 1001);
    assert_eq!(packets[0].timestamp, 960);
    assert_eq!(packets[1].sequence_number, 1002);
    assert_eq!(packets[1].timestamp, 1920);

    // Test depacketization of the first packet
    let mut depacketizer = OpusDepacketizer::new(1);
    let decoded = depacketizer.depacketize(&packets[0]).expect("Depacketize should succeed");
    assert_eq!(decoded.len(), 960);
    assert_eq!(depacketizer.last_sequence_number(), Some(1001));
    assert_eq!(depacketizer.last_timestamp(), Some(960));
}

#[test]
fn test_ai_media_tap_vad_barge_in_latency() {
    let tap = DefaultAiMediaTap::new(48000, 1, 0.05).with_consecutive_frames(3);
    assert!(!tap.trigger_vad_barge_in());

    // Silent frame: RMS ~ 0.0
    let silent_frame = vec![0i16; 960];
    tap.on_incoming_pcm_frame(&silent_frame, 48000, 1);
    assert!(!tap.trigger_vad_barge_in());

    // Active speech frame: high amplitude ~ 16000
    let mut speech_frame = Vec::with_capacity(960);
    for i in 0..960 {
        let s = if (i / 10) % 2 == 0 { 16000i16 } else { -16000i16 };
        speech_frame.push(s);
    }
    let rms = DefaultAiMediaTap::compute_rms(&speech_frame);
    assert!(rms > 0.4);

    // Frame 1 (20ms): not yet triggered (requires 3 frames = 60ms)
    tap.on_incoming_pcm_frame(&speech_frame, 48000, 1);
    assert!(!tap.trigger_vad_barge_in());

    // Frame 2 (40ms): not yet triggered
    tap.on_incoming_pcm_frame(&speech_frame, 48000, 1);
    assert!(!tap.trigger_vad_barge_in());

    // Frame 3 (60ms): triggers barge-in!
    tap.on_incoming_pcm_frame(&speech_frame, 48000, 1);
    assert!(tap.trigger_vad_barge_in(), "VAD barge-in should trigger at <=60ms");

    // Reset barge-in
    tap.reset_barge_in();
    assert!(!tap.trigger_vad_barge_in());
}

#[tokio::test]
async fn test_loopback_transport_pair_audio_data_flow() {
    let config = PeerTransportConfig {
        mode: TransportMode::MockLoopback,
        audio_sample_rate: 48_000,
        audio_channels: 1,
        ..Default::default()
    };

    let (alice, bob) = WebRtcPeerTransport::create_loopback_pair(config);
    assert_eq!(alice.state(), PeerConnectionState::Connected);
    assert_eq!(bob.state(), PeerConnectionState::Connected);

    let bob_tap = Arc::new(DefaultAiMediaTap::new(48000, 1, 0.05));
    bob.set_ai_media_tap(bob_tap.clone());

    let received_samples_count = Arc::new(AtomicUsize::new(0));
    let count_clone = received_samples_count.clone();
    bob.set_audio_frame_callback(Arc::new(move |samples, _rate, _channels| {
        count_clone.fetch_add(samples.len(), Ordering::SeqCst);
    }));

    // Alice generates a 20ms mono PCM frame and sends it over the loopback channel
    let pcm = vec![1234i16; 960];
    alice.send_audio_pcm(&pcm).await.expect("Alice audio send failed");

    // Bob receives from the loopback channel
    let received = bob.recv_loopback_audio().await.expect("Bob receive failed");
    assert_eq!(received.len(), 960);
    assert_eq!(received_samples_count.load(Ordering::SeqCst), 960);
    assert!(bob_tap.last_incoming_rms() > 0.0);
}

#[tokio::test]
async fn test_loopback_transport_pair_video_data_flow() {
    let config = PeerTransportConfig {
        mode: TransportMode::MockLoopback,
        video_width: 320,
        video_height: 240,
        ..Default::default()
    };

    let (alice, bob) = WebRtcPeerTransport::create_loopback_pair(config);

    let received_video = Arc::new(AtomicBool::new(false));
    let rec_clone = received_video.clone();
    bob.set_video_frame_callback(Arc::new(move |frame| {
        if frame.width == 320 && frame.height == 240 {
            rec_clone.store(true, Ordering::SeqCst);
        }
    }));

    let frame = VideoFrameData {
        width: 320,
        height: 240,
        timestamp_ms: 1000,
        data: vec![255u8; 320 * 240 * 4],
    };

    alice.send_video_frame(&frame).await.expect("Alice video send failed");
    let received_frame = bob.recv_loopback_video().await.expect("Bob receive video failed");
    assert_eq!(received_frame.width, 320);
    assert_eq!(received_frame.height, 240);
    assert!(received_video.load(Ordering::SeqCst));
}

#[test]
fn test_webrtc_call_engine_peer_transport_integration() {
    let config = PeerTransportConfig::default();
    let (transport_a, _transport_b) = WebRtcPeerTransport::create_loopback_pair(config);

    let mut engine = WebRtcCallEngine::default();
    assert!(engine.peer_transport.is_none());

    engine.attach_peer_transport(transport_a);
    assert!(engine.peer_transport.is_some());
}

#[test]
fn test_calls_video_bubble_initial_coords_and_small_parent_clamp() {
    use gpui_telegram_webrtc::calls::calls_video_bubble::{BubbleCorner, CallsVideoBubble};
    let mut bubble = CallsVideoBubble::new();
    assert_eq!(bubble.active_corner, BubbleCorner::BottomRight);
    assert!(bubble.x > 0.0);
    assert!(bubble.y > 0.0);

    bubble.snap_to_corner(100.0, 80.0, BubbleCorner::BottomRight);
    assert!(bubble.x >= 0.0, "x must be non-negative, got {}", bubble.x);
    assert!(bubble.y >= 0.0, "y must be non-negative, got {}", bubble.y);

    bubble.snap_to_corner(400.0, 600.0, BubbleCorner::TopLeft);
    assert_eq!(bubble.x, CallsVideoBubble::MARGIN);
    assert_eq!(bubble.y, CallsVideoBubble::MARGIN);
}

#[tokio::test]
async fn test_loopback_parallel_concurrent_bidirectional_burst_no_deadlock() {
    let config = PeerTransportConfig {
        mode: TransportMode::MockLoopback,
        audio_sample_rate: 48_000,
        audio_channels: 1,
        video_width: 64,
        video_height: 48,
        ..Default::default()
    };

    let (alice, bob) = WebRtcPeerTransport::create_loopback_pair(config);
    let alice = Arc::new(alice);
    let bob = Arc::new(bob);

    let alice_audio_cb_count = Arc::new(AtomicUsize::new(0));
    let alice_video_cb_count = Arc::new(AtomicUsize::new(0));
    let bob_audio_cb_count = Arc::new(AtomicUsize::new(0));
    let bob_video_cb_count = Arc::new(AtomicUsize::new(0));

    {
        let c = alice_audio_cb_count.clone();
        alice.set_audio_frame_callback(Arc::new(move |samples, _, _| {
            if samples.len() == 960 {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let c = alice_video_cb_count.clone();
        alice.set_video_frame_callback(Arc::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }
    {
        let c = bob_audio_cb_count.clone();
        bob.set_audio_frame_callback(Arc::new(move |samples, _, _| {
            if samples.len() == 960 {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let c = bob_video_cb_count.clone();
        bob.set_video_frame_callback(Arc::new(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    const BURST: usize = 200; // > 64 frames in both audio and video concurrently

    let alice_send = {
        let alice = Arc::clone(&alice);
        tokio::spawn(async move {
            let pcm = vec![777i16; 960];
            for i in 0..BURST {
                alice.send_audio_pcm(&pcm).await.unwrap();
                let frame = VideoFrameData {
                    width: 64,
                    height: 48,
                    timestamp_ms: i as u64,
                    data: vec![0xAAu8; 64 * 48 * 4],
                };
                alice.send_video_frame(&frame).await.unwrap();
            }
        })
    };

    let bob_send = {
        let bob = Arc::clone(&bob);
        tokio::spawn(async move {
            let pcm = vec![888i16; 960];
            for i in 0..BURST {
                bob.send_audio_pcm(&pcm).await.unwrap();
                let frame = VideoFrameData {
                    width: 64,
                    height: 48,
                    timestamp_ms: i as u64,
                    data: vec![0xBBu8; 64 * 48 * 4],
                };
                bob.send_video_frame(&frame).await.unwrap();
            }
        })
    };

    let alice_recv = {
        let alice = Arc::clone(&alice);
        tokio::spawn(async move {
            let (a_res, v_res) = tokio::join!(
                async {
                    for _ in 0..BURST {
                        alice.recv_loopback_audio().await.unwrap();
                    }
                },
                async {
                    for _ in 0..BURST {
                        alice.recv_loopback_video().await.unwrap();
                    }
                }
            );
            let _ = (a_res, v_res);
        })
    };

    let bob_recv = {
        let bob = Arc::clone(&bob);
        tokio::spawn(async move {
            let (a_res, v_res) = tokio::join!(
                async {
                    for _ in 0..BURST {
                        bob.recv_loopback_audio().await.unwrap();
                    }
                },
                async {
                    for _ in 0..BURST {
                        bob.recv_loopback_video().await.unwrap();
                    }
                }
            );
            let _ = (a_res, v_res);
        })
    };

    let res = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::try_join!(alice_send, bob_send, alice_recv, bob_recv)
    })
    .await;

    assert!(res.is_ok(), "Parallel bidirectional burst timed out / deadlocked");
    res.unwrap().expect("Tasks must not panic");

    assert_eq!(alice_audio_cb_count.load(Ordering::SeqCst), BURST);
    assert_eq!(alice_video_cb_count.load(Ordering::SeqCst), BURST);
    assert_eq!(bob_audio_cb_count.load(Ordering::SeqCst), BURST);
    assert_eq!(bob_video_cb_count.load(Ordering::SeqCst), BURST);
}

#[tokio::test]
async fn test_live_webrtc_two_peers_localhost_offer_answer_ice_and_rtp() {
    use gpui_telegram_webrtc::signaling::SignalingFrame;
    use tokio::sync::mpsc;

    let config = PeerTransportConfig {
        mode: TransportMode::Live,
        ice_servers: Vec::new(),
        enable_audio: true,
        enable_video: true,
        audio_sample_rate: 48_000,
        audio_channels: 1,
        video_width: 32,
        video_height: 24,
        video_fps: 30,
    };

    let mut alice = WebRtcPeerTransport::new(config.clone())
        .await
        .expect("Alice live peer init failed");
    let mut bob = WebRtcPeerTransport::new(config)
        .await
        .expect("Bob live peer init failed");

    let (a_ice_tx, mut a_ice_rx) = mpsc::unbounded_channel::<SignalingFrame>();
    let (b_ice_tx, mut b_ice_rx) = mpsc::unbounded_channel::<SignalingFrame>();

    alice.set_ice_candidate_callback(Arc::new(move |frame| {
        let _ = a_ice_tx.send(frame);
    }));
    bob.set_ice_candidate_callback(Arc::new(move |frame| {
        let _ = b_ice_tx.send(frame);
    }));

    let bob_audio_received = Arc::new(AtomicUsize::new(0));
    let bob_video_received = Arc::new(AtomicBool::new(false));
    let bob_tap = Arc::new(DefaultAiMediaTap::new(48_000, 1, 0.05));
    bob.set_ai_media_tap(bob_tap.clone());

    let audio_cnt = bob_audio_received.clone();
    bob.set_audio_frame_callback(Arc::new(move |samples, _, _| {
        audio_cnt.fetch_add(samples.len(), Ordering::SeqCst);
    }));
    let video_flag = bob_video_received.clone();
    bob.set_video_frame_callback(Arc::new(move |frame| {
        if frame.width == 32 && frame.height == 24 && frame.data.len() == 32 * 24 * 4 {
            video_flag.store(true, Ordering::SeqCst);
        }
    }));

    // Exchange SDP offer & answer
    let offer_sdp = alice.create_offer().await.expect("create_offer failed");
    assert!(offer_sdp.contains("m=audio"), "Offer SDP must contain audio media line");
    assert!(offer_sdp.contains("m=video"), "Offer SDP must contain video media line");

    bob.set_remote_description(&offer_sdp, true)
        .await
        .expect("bob set_remote_description failed");
    let answer_sdp = bob.create_answer().await.expect("create_answer failed");
    alice
        .set_remote_description(&answer_sdp, false)
        .await
        .expect("alice set_remote_description failed");

    // Pump ICE candidates between Alice and Bob until both reach Connected
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        while let Ok(SignalingFrame::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        }) = a_ice_rx.try_recv()
        {
            let _ = bob
                .add_ice_candidate(&candidate, sdp_mid, sdp_mline_index)
                .await;
        }
        while let Ok(SignalingFrame::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        }) = b_ice_rx.try_recv()
        {
            let _ = alice
                .add_ice_candidate(&candidate, sdp_mid, sdp_mline_index)
                .await;
        }
        if alice.state() == PeerConnectionState::Connected
            && bob.state() == PeerConnectionState::Connected
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(
        alice.state(),
        PeerConnectionState::Connected,
        "Alice failed to reach Connected state over localhost ICE"
    );
    assert_eq!(
        bob.state(),
        PeerConnectionState::Connected,
        "Bob failed to reach Connected state over localhost ICE"
    );

    // Send real RTP audio + video packets over DTLS/SRTP/UDP
    let pcm = vec![4096i16; 960];
    let frame = VideoFrameData {
        width: 32,
        height: 24,
        timestamp_ms: 42,
        data: vec![0x55u8; 32 * 24 * 4],
    };

    let media_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < media_deadline {
        let _ = alice.send_audio_pcm(&pcm).await;
        let _ = alice.send_video_frame(&frame).await;
        if bob_audio_received.load(Ordering::SeqCst) >= 960
            && bob_video_received.load(Ordering::SeqCst)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }

    assert!(
        bob_audio_received.load(Ordering::SeqCst) >= 960,
        "Bob should receive live RTP audio frames from Alice"
    );
    assert!(
        bob_video_received.load(Ordering::SeqCst),
        "Bob should receive live RTP video frame from Alice"
    );
    assert!(bob_tap.last_incoming_rms() > 0.0);

    alice.close().await.expect("Alice close failed");
    bob.close().await.expect("Bob close failed");
}

#[test]
fn test_rtp_packet_roundtrip_all_flag_combinations() {
    let payloads: [&[u8]; 2] = [&[1, 2, 3, 4, 5, 6, 7, 8], &[]];
    for payload in payloads {
        for &padding in &[false, true] {
            for &extension in &[false, true] {
                for &csrc_count in &[0u8, 1, 3, 15] {
                    for &marker in &[false, true] {
                        let pkt = RtpPacket {
                            version: 2,
                            padding,
                            extension,
                            csrc_count,
                            marker,
                            payload_type: OPUS_PAYLOAD_TYPE,
                            sequence_number: 42,
                            timestamp: 960,
                            ssrc: 0xDEAD_BEEF,
                            payload: payload.to_vec(),
                        };
                        let wire = pkt.serialize();
                        let parsed = RtpPacket::parse(&wire).unwrap_or_else(|e| {
                            panic!(
                                "RtpPacket roundtrip failed for padding={}, extension={}, csrc_count={}, marker={}, payload_len={}: {:?}",
                                padding, extension, csrc_count, marker, payload.len(), e
                            )
                        });
                        assert_eq!(
                            parsed, pkt,
                            "Roundtrip mismatch for padding={}, extension={}, csrc_count={}, marker={}, payload_len={}",
                            padding, extension, csrc_count, marker, payload.len()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn test_opus_decode_frame_rejects_corrupted_toc_header() {
    let mono_len = 1 + OPUS_SAMPLES_PER_FRAME_MONO * 2;
    let stereo_len = 1 + OPUS_SAMPLES_PER_FRAME_STEREO * 2;

    // 0x03: config=0 (Narrowband), frame_count_code=3 (arbitrary multi-frame)
    // 0x00: config=0, frame_count_code=0
    // 0xA1: config=20, frame_count_code=1
    // 0xA2: config=20, frame_count_code=2
    // 0xA7: config=20, stereo=true, frame_count_code=3
    // 0xFC: config=31, stereo=true, frame_count_code=0
    for &bad_toc in &[0x03u8, 0x00, 0xA1, 0xA2, 0xA7, 0xFC] {
        let mut mono_buf = vec![0u8; mono_len];
        mono_buf[0] = bad_toc;
        assert_eq!(
            OpusFramingCodec::decode_frame(&mono_buf, 1),
            Err(CodecError::CorruptedHeader(bad_toc)),
            "Expected CorruptedHeader for mono toc=0x{bad_toc:02X}"
        );

        let mut stereo_buf = vec![0u8; stereo_len];
        stereo_buf[0] = bad_toc;
        assert_eq!(
            OpusFramingCodec::decode_frame(&stereo_buf, 2),
            Err(CodecError::CorruptedHeader(bad_toc)),
            "Expected CorruptedHeader for stereo toc=0x{bad_toc:02X}"
        );
    }

    // Valid TOC 0xA0 (mono) passed as stereo must still yield UnsupportedChannels(1)
    let mut mono_buf = vec![0u8; stereo_len];
    mono_buf[0] = 0xA0;
    assert_eq!(
        OpusFramingCodec::decode_frame(&mono_buf, 2),
        Err(CodecError::UnsupportedChannels(1))
    );
}

#[test]
fn test_calls_video_bubble_finite_bounds_and_drag_clamping() {
    use gpui_telegram_webrtc::calls::calls_video_bubble::{
        BubbleCorner, BubbleDragMode, CallsVideoBubble,
    };

    let mut bubble = CallsVideoBubble::new();
    let initial_x = bubble.x;
    let initial_y = bubble.y;

    // Non-finite parent bounds must never produce inf or NaN coordinates
    for &(pw, ph) in &[
        (f32::INFINITY, f32::INFINITY),
        (f32::INFINITY, 300.0),
        (400.0, f32::INFINITY),
        (f32::NEG_INFINITY, 300.0),
        (f32::NAN, 300.0),
        (400.0, f32::NAN),
    ] {
        for corner in [
            BubbleCorner::TopLeft,
            BubbleCorner::TopRight,
            BubbleCorner::BottomLeft,
            BubbleCorner::BottomRight,
        ] {
            bubble.snap_to_corner(pw, ph, corner);
            assert!(
                bubble.x.is_finite() && bubble.y.is_finite(),
                "Coordinates became non-finite ({}, {}) for parent ({}, {}) corner {:?}",
                bubble.x,
                bubble.y,
                pw,
                ph,
                corner
            );
        }
    }
    assert_eq!(bubble.x, initial_x);
    assert_eq!(bubble.y, initial_y);

    // Drag boundary clamping within a 400x300 parent
    let pw = 400.0;
    let ph = 300.0;
    let max_x = pw - bubble.width - CallsVideoBubble::MARGIN;
    let max_y = ph - bubble.height - CallsVideoBubble::MARGIN;
    let min_x = CallsVideoBubble::MARGIN;
    let min_y = CallsVideoBubble::MARGIN;

    bubble.start_drag();
    assert!(bubble.is_dragging);

    // Drag past top-left out of bounds
    bubble.drag_to(-200.0, -100.0, pw, ph);
    assert_eq!(bubble.x, min_x);
    assert_eq!(bubble.y, min_y);

    // Drag past bottom-right out of bounds
    bubble.drag_to(5000.0, 5000.0, pw, ph);
    assert_eq!(bubble.x, max_x);
    assert_eq!(bubble.y, max_y);

    // Non-finite drag inputs must be ignored
    bubble.drag_to(f32::NAN, f32::INFINITY, pw, ph);
    assert!(bubble.x.is_finite() && bubble.y.is_finite());

    // Drag near TopRight and release -> snaps to TopRight
    bubble.drag_to(max_x - 5.0, min_y + 5.0, pw, ph);
    bubble.end_drag(pw, ph);
    assert!(!bubble.is_dragging);
    assert_eq!(bubble.active_corner, BubbleCorner::TopRight);
    assert_eq!(bubble.x, max_x);
    assert_eq!(bubble.y, min_y);

    // clamp_to_bounds heals non-finite or out-of-range coordinates
    bubble.x = f32::INFINITY;
    bubble.y = -999.0;
    bubble.clamp_to_bounds(pw, ph);
    assert_eq!(bubble.x, min_x);
    assert_eq!(bubble.y, min_y);

    // BubbleDragMode::None disables dragging
    bubble.drag_mode = BubbleDragMode::None;
    bubble.start_drag();
    assert!(!bubble.is_dragging);
    bubble.drag_to(100.0, 100.0, pw, ph);
    assert_eq!(bubble.x, min_x);
    assert_eq!(bubble.y, min_y);
}
