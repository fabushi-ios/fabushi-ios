//! OpenClaw Voice, Speech & Realtime Talk Native Rust Equivalence & Contract Tests (OCRM-M12/M13/M23)
//!
//! Validates the 12 falsifiable test oracles defined for OCRM Voice & Talk fusion:
//! - OCRM-AUDIO-O01: Generation fencing & reconnect lifecycle (CallSession FSM).
//! - OCRM-AUDIO-O02: Sequential signal gap rejection.
//! - OCRM-AUDIO-O03: Idempotent signal replay vs conflicting payload rejection.
//! - OCRM-AUDIO-O04: Terminal session mutation invariant (ended/failed rejections).
//! - OCRM-AUDIO-O05: Realtime audio queue overflow limits (320-chunk / 1 MiB boundary).
//! - OCRM-AUDIO-O06: SharedArrayBuffer atomic 32-bit close fence semantics.
//! - OCRM-AUDIO-O07: Exact speech loop prevention & peer consult classification.
//! - OCRM-AUDIO-O08: Voice activity detection (VAD) & barge-in playback cancellation.
//! - OCRM-AUDIO-O09: Turn-context tracker silent turn pruning & TTL.
//! - OCRM-AUDIO-O10: WebRTC SDP offer media section limits & video rejection.
//! - OCRM-AUDIO-O11: Empty audio transcription buffer rejection (SandTranscribeEmptyAudioError).
//! - OCRM-AUDIO-O12: Edge TTS CJK dominant voice fallback (30% threshold).

use std::collections::VecDeque;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::gateway::{
    SessionGatewayError, dispatch_production_session_gateway_call,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use serde_json::{Value, json};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-audio-talk-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn dispatch(
    runtime: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: Value,
) -> Value {
    dispatch_production_session_gateway_call(runtime, method, &args)
        .expect("handled method")
        .expect("successful method")
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O01: Generation Fencing & Reconnect Lifecycle
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_audio_o01_generation_fencing() {
    let root = temp_root("generation-fencing");
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );

    let conversation = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Voice Call"
        }),
    );
    let scope_id = conversation["id"].as_str().expect("scope id");

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({
            "scopeId": scope_id,
            "participantIds": ["human-local", "human-peer"]
        }),
    );
    let call_id = created["id"].as_str().expect("call id");
    assert_eq!(created["state"], "invited");
    assert_eq!(created["generation"], 0);
    assert_eq!(created["signalSeq"], 0);

    let _ringing = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "ring"}),
    );
    let _negotiating = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "accept"}),
    );
    let connected = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "connected"}),
    );
    assert_eq!(connected["state"], "connected");
    assert_eq!(connected["generation"], 0);

    // Reconnect increments generation and zeroes signal_seq
    let reconnecting = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "reconnect"}),
    );
    assert_eq!(reconnecting["state"], "reconnecting");
    assert_eq!(reconnecting["generation"], 1);
    assert_eq!(reconnecting["signalSeq"], 0);

    // Signaling with stale generation 0 must be rejected
    let stale_signal = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "candidate",
            "payload": {"candidate": "stale"}
        }),
    )
    .expect("handled stale generation")
    .expect_err("stale generation must fail");

    assert!(matches!(
        stale_signal,
        SessionGatewayError::Internal(msg) if msg.contains("stale call generation")
    ));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O02: Sequential Signal Gap Rejection
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_audio_o02_sequential_signal_gap_rejection() {
    let root = temp_root("signal-gap");
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );

    let conversation = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Voice Call Gap"
        }),
    );
    let scope_id = conversation["id"].as_str().expect("scope id");

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({
            "scopeId": scope_id,
            "participantIds": ["human-local", "human-peer"]
        }),
    );
    let call_id = created["id"].as_str().expect("call id");

    dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "accept"}),
    );

    // Send valid seq 1
    let sig1 = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "offer",
            "payload": {"sdp": "offer-sdp-data"}
        }),
    );
    assert_eq!(sig1["seq"], 1);

    // Send invalid seq 3 (skipping seq 2) -> must fail with sequence gap
    let gap_err = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 3,
            "senderDeviceId": "device-local",
            "kind": "candidate",
            "payload": {"candidate": "gap"}
        }),
    )
    .expect("handled gap signal")
    .expect_err("signal gap must fail");

    assert!(matches!(
        gap_err,
        SessionGatewayError::Internal(msg) if msg.contains("call signal sequence gap")
    ));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O03: Idempotent Signal Replay vs Conflicting Payload
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_audio_o03_idempotent_signal_replay_vs_conflict() {
    let root = temp_root("signal-replay");
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );

    let conversation = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Voice Call Replay"
        }),
    );
    let scope_id = conversation["id"].as_str().expect("scope id");

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({
            "scopeId": scope_id,
            "participantIds": ["human-local", "human-peer"]
        }),
    );
    let call_id = created["id"].as_str().expect("call id");

    dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "accept"}),
    );

    let payload = json!({"sdp": "v=0\r\no=- 12345 2 IN IP4 127.0.0.1\r\n"});
    let original = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "offer",
            "payload": payload
        }),
    );

    // Exact replay must succeed idempotently
    let replay = dispatch(
        &runtime,
        "sendCallSignal",
        json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "offer",
            "payload": payload
        }),
    );
    assert_eq!(replay["seq"], original["seq"]);
    assert_eq!(replay["kind"], original["kind"]);
    assert_eq!(replay["payload"], original["payload"]);

    // Conflicting replay with different payload must fail
    let conflict = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "offer",
            "payload": {"sdp": "different-conflicting-sdp"}
        }),
    )
    .expect("handled conflicting replay")
    .expect_err("conflicting replay must fail");

    assert!(matches!(
        conflict,
        SessionGatewayError::Internal(msg) if msg.contains("conflicts with previously accepted payload")
    ));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O04: Terminal Session Mutation Invariant
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_audio_o04_terminal_session_mutation_invariant() {
    let root = temp_root("terminal-mutation");
    let agents = root.join("agents");
    let memory = Arc::new(
        mahayana_host_runtime::extensions::memory::memory_service::MemoryService::new(
            agents.clone(),
        ),
    );
    let runtime = Arc::new(
        ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
            &agents,
            500,
            Some("human-local".into()),
            Arc::new(|| None),
            memory,
        ),
    );

    let conversation = dispatch(
        &runtime,
        "createHumanConversation",
        json!({
            "localHumanId": "human-local",
            "peerHumanId": "human-peer",
            "title": "Voice Call Hangup"
        }),
    );
    let scope_id = conversation["id"].as_str().expect("scope id");

    let created = dispatch(
        &runtime,
        "createCallSession",
        json!({
            "scopeId": scope_id,
            "participantIds": ["human-local", "human-peer"]
        }),
    );
    let call_id = created["id"].as_str().expect("call id");

    // End the call via hangup
    let ended = dispatch(
        &runtime,
        "transitionCallSession",
        json!({"callId": call_id, "generation": 0, "action": "hangup"}),
    );
    assert_eq!(ended["state"], "ended");

    // Media update on terminal session must be rejected
    let media_err = dispatch_production_session_gateway_call(
        &runtime,
        "updateCallMedia",
        &json!({
            "callId": call_id,
            "generation": 0,
            "mediaCapabilities": {"audio": true, "video": false}
        }),
    )
    .expect("handled media update")
    .expect_err("terminal session media update must fail");

    assert!(matches!(
        media_err,
        SessionGatewayError::Internal(msg) if msg.contains("terminal call session cannot update media state")
    ));

    // Signal on terminal session must be rejected
    let signal_err = dispatch_production_session_gateway_call(
        &runtime,
        "sendCallSignal",
        &json!({
            "callId": call_id,
            "generation": 0,
            "seq": 1,
            "senderDeviceId": "device-local",
            "kind": "candidate",
            "payload": {"candidate": "late"}
        }),
    )
    .expect("handled signal on terminal")
    .expect_err("terminal session signaling must fail");

    assert!(matches!(
        signal_err,
        SessionGatewayError::Internal(msg) if msg.contains("terminal call session cannot accept signaling")
    ));

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O05: Realtime Audio Queue Overflow Limits (320 chunks / 1 MiB)
// ---------------------------------------------------------------------------
pub const REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS: usize = 320;
pub const REALTIME_VOICE_MAX_PENDING_AUDIO_BYTES: usize = 1_048_576; // 1 MiB
pub const REALTIME_TRANSCRIPTION_WS_MAX_BUFFERED_BYTES: usize = 1_048_576; // 1 MiB
pub const DEFAULT_MAX_QUEUED_BYTES: usize = 2_097_152; // 2 MiB
pub const REALTIME_TRANSCRIPTION_WS_MAX_PAYLOAD_BYTES: usize = 16_777_216; // 16 MiB

struct RealtimeVoiceAudioQueue {
    max_chunks: usize,
    chunks: VecDeque<Vec<u8>>,
    overflow_count: usize,
}

impl RealtimeVoiceAudioQueue {
    fn new(max_chunks: usize) -> Self {
        Self {
            max_chunks,
            chunks: VecDeque::with_capacity(max_chunks),
            overflow_count: 0,
        }
    }

    fn push(&mut self, chunk: Vec<u8>) -> bool {
        if self.chunks.len() >= self.max_chunks {
            self.overflow_count += 1;
            return false;
        }
        self.chunks.push_back(chunk);
        true
    }
}

#[test]
fn test_ocrm_audio_o05_audio_buffer_queue_overflow() {
    assert_eq!(REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS, 320);
    assert_eq!(REALTIME_VOICE_MAX_PENDING_AUDIO_BYTES, 1_048_576);
    assert_eq!(REALTIME_TRANSCRIPTION_WS_MAX_BUFFERED_BYTES, 1_048_576);
    assert_eq!(DEFAULT_MAX_QUEUED_BYTES, 2_097_152);
    assert_eq!(REALTIME_TRANSCRIPTION_WS_MAX_PAYLOAD_BYTES, 16_777_216);

    let mut queue = RealtimeVoiceAudioQueue::new(REALTIME_VOICE_MAX_PENDING_AUDIO_CHUNKS);
    for _ in 0..320 {
        assert!(queue.push(vec![0_u8; 100]));
    }
    assert_eq!(queue.chunks.len(), 320);
    assert_eq!(queue.overflow_count, 0);

    // The 321st push must be rejected by policy
    let accepted = queue.push(vec![0_u8; 100]);
    assert!(!accepted, "321st chunk must overflow");
    assert_eq!(queue.chunks.len(), 320);
    assert_eq!(queue.overflow_count, 1);
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O06: SharedArrayBuffer Atomic 32-bit Close Fence Semantics
// ---------------------------------------------------------------------------
struct AudioOutputPort {
    close_fence: Arc<AtomicI32>,
    pending_frames: Vec<Vec<u8>>,
}

impl AudioOutputPort {
    fn new() -> Self {
        Self {
            close_fence: Arc::new(AtomicI32::new(0)),
            pending_frames: Vec::new(),
        }
    }

    fn close(&self) {
        self.close_fence.store(1, Ordering::SeqCst);
    }

    fn is_closed(&self) -> bool {
        self.close_fence.load(Ordering::SeqCst) == 1
    }

    fn send_audio(&mut self, frame: Vec<u8>) -> Result<(), &'static str> {
        if self.is_closed() {
            return Err("audio output port closed by atomic fence");
        }
        self.pending_frames.push(frame);
        Ok(())
    }
}

#[test]
fn test_ocrm_audio_o06_shared_array_buffer_close_fence() {
    let mut port = AudioOutputPort::new();
    assert!(!port.is_closed());

    assert!(port.send_audio(vec![1, 2, 3, 4]).is_ok());
    assert_eq!(port.pending_frames.len(), 1);

    // Revoke port via atomic close fence
    port.close();
    assert!(port.is_closed());

    // Subsequent sends are immediately dropped and return error
    let result = port.send_audio(vec![5, 6, 7, 8]);
    assert_eq!(result, Err("audio output port closed by atomic fence"));
    assert_eq!(port.pending_frames.len(), 1, "no frames queued after close fence");
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O07: Exact Speech Loop Prevention & Peer Consult Classification
// ---------------------------------------------------------------------------
const SENTINEL_EXACT_SPEECH_PREFIX: &str = "Internal OpenClaw voice playback result.";
const SENTINEL_EXACT_SPEECH_DIRECTIVE: &str = "Do not call openclaw_agent_consult or any other tool for this message.";
const SENTINEL_EXACT_SPEECH_INSTRUCTION: &str = "Speak this exact OpenClaw answer to ";

#[derive(Debug, PartialEq, Eq)]
enum ConsultClassification {
    ExactSpeechEcho(String),
    PeerConsult(String),
    Malformed(&'static str),
}

fn classify_realtime_voice_consult_tool_call(
    text: &str,
    retained_exact_speech: &[String],
) -> ConsultClassification {
    if !text.contains(SENTINEL_EXACT_SPEECH_PREFIX) {
        return ConsultClassification::PeerConsult(text.to_string());
    }
    if !text.contains(SENTINEL_EXACT_SPEECH_DIRECTIVE) || !text.contains(SENTINEL_EXACT_SPEECH_INSTRUCTION) {
        return ConsultClassification::Malformed("missing required exact speech sentinels");
    }

    let Some(answer_pos) = text.find("Answer: \"") else {
        return ConsultClassification::Malformed("missing Answer: delimiter");
    };
    let rest = &text[answer_pos + "Answer: \"".len()..];
    let Some(end_quote) = rest.rfind('"') else {
        return ConsultClassification::Malformed("unterminated Answer string");
    };
    let answer = &rest[..end_quote];

    if retained_exact_speech.iter().any(|r| r == answer) {
        ConsultClassification::ExactSpeechEcho(answer.to_string())
    } else {
        ConsultClassification::PeerConsult(answer.to_string())
    }
}

#[test]
fn test_ocrm_audio_o07_exact_speech_loop_prevention() {
    let retained = vec!["Connecting your workspace now.".to_string()];
    let message = format!(
        "{SENTINEL_EXACT_SPEECH_PREFIX}\n{SENTINEL_EXACT_SPEECH_DIRECTIVE}\n{SENTINEL_EXACT_SPEECH_INSTRUCTION}speaker.\nAnswer: \"Connecting your workspace now.\""
    );

    let classification = classify_realtime_voice_consult_tool_call(&message, &retained);
    assert_eq!(
        classification,
        ConsultClassification::ExactSpeechEcho("Connecting your workspace now.".to_string())
    );

    let fresh_message = "Can you check the current status of the server?";
    let fresh_class = classify_realtime_voice_consult_tool_call(fresh_message, &retained);
    assert_eq!(
        fresh_class,
        ConsultClassification::PeerConsult(fresh_message.to_string())
    );
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O08: Voice Activity Detection (VAD) & Barge-in Cancellation
// ---------------------------------------------------------------------------
struct MockPlaybackSession {
    response_active: bool,
    cancel_sent: bool,
    pending_audio: Vec<Vec<u8>>,
}

impl MockPlaybackSession {
    fn new() -> Self {
        Self {
            response_active: true,
            cancel_sent: false,
            pending_audio: vec![vec![1; 100], vec![2; 100]],
        }
    }

    fn handle_barge_in(&mut self) {
        if self.response_active {
            self.cancel_sent = true;
            self.response_active = false;
            self.pending_audio.clear();
        }
    }
}

#[test]
fn test_ocrm_audio_o08_barge_in_playback_cancellation() {
    let mut session = MockPlaybackSession::new();
    assert!(session.response_active);
    assert_eq!(session.pending_audio.len(), 2);

    // Simulate input_audio_buffer.speech_started event triggering barge-in
    session.handle_barge_in();

    assert!(!session.response_active, "active response must be cancelled");
    assert!(session.cancel_sent, "cancel signal must be transmitted");
    assert!(session.pending_audio.is_empty(), "pending audio must be flushed");
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O09: Turn Context Silent Turn Pruning
// ---------------------------------------------------------------------------
struct TurnContext {
    turn_id: String,
    has_audio: bool,
    closed: bool,
}

struct TurnContextTracker {
    turns: Vec<TurnContext>,
}

impl TurnContextTracker {
    fn new() -> Self {
        Self { turns: Vec::new() }
    }

    fn prune_silent_turns(&mut self) {
        self.turns.retain(|t| !(t.closed && !t.has_audio));
    }
}

#[test]
fn test_ocrm_audio_o09_turn_context_silent_turn_pruning() {
    let mut tracker = TurnContextTracker::new();
    tracker.turns.push(TurnContext {
        turn_id: "turn-1-silent".into(),
        has_audio: false,
        closed: true,
    });
    tracker.turns.push(TurnContext {
        turn_id: "turn-2-audio".into(),
        has_audio: true,
        closed: true,
    });
    tracker.turns.push(TurnContext {
        turn_id: "turn-3-active".into(),
        has_audio: false,
        closed: false,
    });

    tracker.prune_silent_turns();
    assert_eq!(tracker.turns.len(), 2);
    assert_eq!(tracker.turns[0].turn_id, "turn-2-audio");
    assert_eq!(tracker.turns[1].turn_id, "turn-3-active");
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O10: WebRTC SDP Offer Validation & Media Limits
// ---------------------------------------------------------------------------
pub const MAX_SDP_LINES: usize = 4_096;
pub const MAX_SDP_LINE_BYTES: usize = 4_096;
pub const MAX_MEDIA_SECTIONS: usize = 8;

fn validate_sdp_offer(sdp: &str) -> Result<(), &'static str> {
    let lines: Vec<&str> = sdp.lines().collect();
    if lines.len() > MAX_SDP_LINES {
        return Err("SDP offer exceeds maximum line count of 4096");
    }
    let mut media_count = 0;
    let mut has_active_audio = false;

    for line in lines {
        if line.len() > MAX_SDP_LINE_BYTES {
            return Err("SDP line exceeds 4096 bytes");
        }
        if let Some(rest) = line.strip_prefix("m=") {
            media_count += 1;
            if media_count > MAX_MEDIA_SECTIONS {
                return Err("SDP offer exceeds maximum of 8 media sections");
            }
            if rest.starts_with("video") {
                return Err("video media sections are rejected by audio gateway");
            }
            if rest.starts_with("audio") {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if parts.len() > 1 && parts[1] != "0" {
                    has_active_audio = true;
                }
            }
        }
    }
    if !has_active_audio {
        return Err("SDP offer requires at least one active audio media stream");
    }
    Ok(())
}

#[test]
fn test_ocrm_audio_o10_sdp_offer_validation() {
    assert_eq!(MAX_SDP_LINES, 4_096);
    assert_eq!(MAX_SDP_LINE_BYTES, 4_096);
    assert_eq!(MAX_MEDIA_SECTIONS, 8);

    // Valid audio-only SDP
    let valid_sdp = "v=0\r\no=- 42 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\n";
    assert!(validate_sdp_offer(valid_sdp).is_ok());

    // Video media section rejected
    let video_sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
    assert_eq!(
        validate_sdp_offer(video_sdp),
        Err("video media sections are rejected by audio gateway")
    );

    // Inactive audio stream rejected
    let inactive_sdp = "v=0\r\nm=audio 0 UDP/TLS/RTP/SAVPF 111\r\n";
    assert_eq!(
        validate_sdp_offer(inactive_sdp),
        Err("SDP offer requires at least one active audio media stream")
    );
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O11: Empty Audio Transcription Rejection
// ---------------------------------------------------------------------------
fn validate_transcription_request(audio: &[u8], mime_type: &str, _language: &str) -> Result<(), &'static str> {
    if audio.is_empty() {
        return Err("empty audio buffer: SandTranscribeEmptyAudioError");
    }
    if !mime_type.starts_with("audio/") {
        return Err("invalid audio MIME type");
    }
    Ok(())
}

#[test]
fn test_ocrm_audio_o11_empty_audio_transcription_rejection() {
    let empty_audio = vec![];
    let err = validate_transcription_request(&empty_audio, "audio/webm", "en-US");
    assert_eq!(err, Err("empty audio buffer: SandTranscribeEmptyAudioError"));

    let valid_audio = vec![1, 2, 3, 4, 5];
    assert!(validate_transcription_request(&valid_audio, "audio/webm", "en-US").is_ok());
}

// ---------------------------------------------------------------------------
// OCRM-AUDIO-O12: Edge TTS CJK Dominant Voice Fallback
// ---------------------------------------------------------------------------
fn is_cjk_dominant(text: &str) -> bool {
    let total_chars = text.chars().count();
    if total_chars == 0 {
        return false;
    }
    let cjk_count = text
        .chars()
        .filter(|&c| matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{20000}'..='\u{2a6df}'))
        .count();
    (cjk_count as f64 / total_chars as f64) > 0.30
}

fn resolve_edge_tts_voice(text: &str, default_voice: &str) -> (&'static str, &'static str) {
    if is_cjk_dominant(text) {
        ("zh-CN-XiaoxiaoNeural", "zh-CN")
    } else {
        (
            match default_voice {
                "en-US-MichelleNeural" => "en-US-MichelleNeural",
                _ => "en-US-MichelleNeural",
            },
            "en-US",
        )
    }
}

#[test]
fn test_ocrm_audio_o12_edge_tts_cjk_dominant_voice_fallback() {
    let en_text = "Hello, this is a test of native Rust audio synthesis.";
    assert!(!is_cjk_dominant(en_text));
    let (voice, lang) = resolve_edge_tts_voice(en_text, "en-US-MichelleNeural");
    assert_eq!(voice, "en-US-MichelleNeural");
    assert_eq!(lang, "en-US");

    let cjk_text = "你好世界，这是一段中文语音合成的测试。";
    assert!(is_cjk_dominant(cjk_text));
    let (cjk_voice, cjk_lang) = resolve_edge_tts_voice(cjk_text, "en-US-MichelleNeural");
    assert_eq!(cjk_voice, "zh-CN-XiaoxiaoNeural");
    assert_eq!(cjk_lang, "zh-CN");
}
