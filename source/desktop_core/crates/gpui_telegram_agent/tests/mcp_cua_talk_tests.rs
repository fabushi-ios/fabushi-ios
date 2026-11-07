use gpui_telegram_agent::computer::{
    CuaAction, CuaExecutor, LeaseState, VncProxyDescriptor,
};
use gpui_telegram_agent::mcp::{
    McpRegistry, McpToolDefinition, RoutedMcpBackend, RoutedTool, RoutedToolCall,
    ToolPermissionDock, ToolPermissionRequest, ToolRiskLevel,
};
use gpui_telegram_agent::talk::{
    decode_waveform_5bit, TalkCoordinator, TalkSessionState, VadBargeInDetector,
    MAX_AUDIO_BUFFER_CHUNKS,
};
use gpui_telegram_agent::ComputerLeaseBroker;
use serde_json::json;
use std::time::Instant;

// ============================================================================
// 1. MCP Registry & Routed Protocol Bridge Tests
// ============================================================================

struct DummyRoutedBackend {
    tools: Vec<RoutedTool>,
}

impl RoutedMcpBackend for DummyRoutedBackend {
    fn list_tools(&mut self) -> Result<Vec<RoutedTool>, mahayana_node_agent_coordinator::protocol::Failure> {
        Ok(self.tools.clone())
    }

    fn call_tool(
        &mut self,
        call: RoutedToolCall,
    ) -> Result<serde_json::Value, mahayana_node_agent_coordinator::protocol::Failure> {
        Ok(json!({
            "echo_tool": call.tool_name,
            "echo_args": call.args
        }))
    }
}

#[test]
fn test_mcp_registry_registration_and_routed_bridge() {
    let mut registry = McpRegistry::new();

    // 1. Register tools with varying risk levels
    let tool_read = McpToolDefinition::new(
        "file_reader",
        "Reads file content from workspace",
        json!({"type": "object"}).to_string(),
        ToolRiskLevel::ReadOnly,
    );
    assert!(!tool_read.requires_approval);

    let tool_write = McpToolDefinition::new(
        "terminal_exec",
        "Executes terminal commands",
        json!({"type": "object"}).to_string(),
        ToolRiskLevel::CriticalDestructive,
    );
    assert!(tool_write.requires_approval);

    registry.register(tool_read);
    registry.register(tool_write);

    assert_eq!(registry.list_tools().len(), 2);
    assert!(registry.get("file_reader").is_some());
    assert!(registry.get("terminal_exec").is_some());
    assert!(registry.get("nonexistent").is_none());

    // 2. Register routed tool directly from coordinator
    let routed = RoutedTool {
        name: "cloud_search".into(),
        provider_identifier: "host_provider".into(),
        tool_name: "search".into(),
        description: Some("Search cloud databases".into()),
        input_schema: Some(json!({"query": "string"})),
    };
    registry.register_routed_tool(routed, false, ToolRiskLevel::Low, Some("server_srv_1".into()));

    let cloud_tool = registry.get("cloud_search").expect("registered tool");
    assert_eq!(cloud_tool.server_id.as_deref(), Some("server_srv_1"));
    assert_eq!(cloud_tool.risk_level, ToolRiskLevel::Low);

    // 3. Test routed protocol bridge path and URL generation
    let bridge = registry.bridge();
    let path = bridge.path();
    assert!(path.starts_with("/mcp/"));

    let local_url = bridge.local_url(9090).expect("valid local url");
    assert!(local_url.starts_with("http://127.0.0.1:9090/mcp/"));

    // 4. Test HTTP outcome dispatch with backend
    let mut backend = DummyRoutedBackend {
        tools: vec![RoutedTool {
            name: "test_tool".into(),
            provider_identifier: "test_prov".into(),
            tool_name: "test_tool".into(),
            description: None,
            input_schema: None,
        }],
    };

    let invalid_path_res = bridge.handle_http("POST", "/invalid", b"{}", &mut backend);
    assert_eq!(
        invalid_path_res,
        gpui_telegram_agent::mcp::RoutedMcpHttpOutcome::HttpError(404)
    );
}

#[test]
fn test_tool_permission_dock_risk_evaluation_and_approval() {
    let mut dock = ToolPermissionDock::new();
    assert_eq!(dock.pending_count(), 0);

    let tool_def = McpToolDefinition::new(
        "delete_database",
        "Drops production table",
        "{}",
        ToolRiskLevel::CriticalDestructive,
    );

    let assessed_risk = dock.evaluate_risk(&tool_def, &json!({"table": "users"}));
    assert_eq!(assessed_risk, ToolRiskLevel::CriticalDestructive);
    assert!(assessed_risk.requires_explicit_confirmation());

    let req = ToolPermissionRequest::new(
        "req_drop_01",
        "delete_database",
        json!({"table": "users"}),
        assessed_risk,
        "subagent_42",
    );

    dock.submit_request(req.clone());
    assert_eq!(dock.pending_count(), 1);
    assert_eq!(dock.list_pending().len(), 1);
    assert_eq!(
        dock.get_request("req_drop_01").unwrap().tool_name,
        "delete_database"
    );

    // Approve resolution
    let resolved = dock.resolve("req_drop_01", true);
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().request_id, "req_drop_01");
    assert_eq!(dock.pending_count(), 0);
    assert!(dock.get_request("req_drop_01").is_none());

    // Check resolution history recorded
    assert_eq!(dock.resolved_history.len(), 1);
    assert_eq!(dock.resolved_history[0].0, "req_drop_01");
    assert!(dock.resolved_history[0].1);

    // Dynamic risk escalation test: Low-risk tool with destructive keyword "rm"
    let low_risk_tool = McpToolDefinition::new(
        "run_shell",
        "Executes a command",
        "{}",
        ToolRiskLevel::Low,
    );
    // Destructive command escalates to CriticalDestructive
    let escalated = dock.evaluate_risk(&low_risk_tool, &json!({"command": "rm -rf /tmp/test"}));
    assert_eq!(escalated, ToolRiskLevel::CriticalDestructive);

    // Benign word containing "rm" inside substring (e.g. terminal, information) does NOT escalate
    let benign = dock.evaluate_risk(&low_risk_tool, &json!({"output": "terminal_information"}));
    assert_eq!(benign, ToolRiskLevel::Low);

    // Other destructive keywords escalate (delete, drop, kill, exec, write)
    let escalated_delete = dock.evaluate_risk(&low_risk_tool, &json!({"action": "delete"}));
    assert_eq!(escalated_delete, ToolRiskLevel::CriticalDestructive);
}

// ============================================================================
// 2. Remote Computer CUA Lease Broker & Executor Tests
// ============================================================================

#[test]
fn test_cua_lease_lifecycle_and_generation_fencing() {
    let mut broker = ComputerLeaseBroker::new();
    assert_eq!(broker.current_generation(), 0);
    assert!(!broker.is_lease_active());

    // 1. Acquire control lease
    let lease = broker
        .acquire_lease("agent_worker_1", "remote_box_ubuntu")
        .expect("lease should be granted");

    assert_eq!(lease.state, LeaseState::ActiveControl);
    assert!(!lease.is_preempted);
    assert!(lease.is_valid());
    assert_eq!(lease.target_device_id, "remote_box_ubuntu");
    assert_eq!(broker.current_generation(), lease.generation);
    assert!(broker.is_lease_active());

    // 2. Execute fenced CUA actions
    let click_action = CuaAction::Click {
        x: 640,
        y: 480,
        button: "left".into(),
    };
    let exec_res = CuaExecutor::execute_action_fenced(
        &click_action,
        &lease,
        broker.current_generation(),
    );
    assert!(exec_res.is_ok());

    // 3. Stale generation fencing: rejecting outdated lease generation
    let stale_exec_res = CuaExecutor::execute_action_fenced(
        &click_action,
        &lease,
        broker.current_generation() + 1,
    );
    assert!(stale_exec_res.is_err());

    // 4. Protocol mapping verification
    let proto_click = CuaExecutor::action_to_protocol(&click_action);
    assert_eq!(
        proto_click.action,
        gpui_telegram_agent::computer::ComputerActionKind::Click
    );
    assert_eq!(proto_click.x, Some(640));
    assert_eq!(proto_click.y, Some(480));
    assert_eq!(
        proto_click.button,
        Some(gpui_telegram_agent::computer::ComputerMouseButton::Left)
    );

    let wait_action = CuaAction::Wait { wait_ms: 500 };
    let proto_wait = CuaExecutor::action_to_protocol(&wait_action);
    assert_eq!(
        proto_wait.action,
        gpui_telegram_agent::computer::ComputerActionKind::Wait
    );
    assert_eq!(proto_wait.wait_ms, Some(500));

    // 5. RFB packet encoding
    let rfb_pointer = CuaExecutor::to_rfb_pointer_event(640, 480, 1);
    assert_eq!(rfb_pointer[0], 5); // PointerEvent message-type
    assert_eq!(rfb_pointer[1], 1); // Left button pressed
    assert_eq!(u16::from_be_bytes([rfb_pointer[2], rfb_pointer[3]]), 640);
    assert_eq!(u16::from_be_bytes([rfb_pointer[4], rfb_pointer[5]]), 480);

    let rfb_key = CuaExecutor::to_rfb_key_event(true, 0xFF0D);
    assert_eq!(rfb_key[0], 4); // KeyEvent message-type
    assert_eq!(rfb_key[1], 1); // down-flag
    assert_eq!(u32::from_be_bytes([rfb_key[4], rfb_key[5], rfb_key[6], rfb_key[7]]), 0xFF0D);

    // 6. VNC Fork URL builder
    let proxy_desc = VncProxyDescriptor {
        primary_url: "http://vnc.internal:6080/vnc.html".into(),
        fork_base_url: "http://vnc.internal:6081".into(),
        network_token: "net_token_secret_123".into(),
    };
    let fork_url = CuaExecutor::build_vnc_fork_url(&proxy_desc, Some("tok_abc"));
    assert!(fork_url.contains("http://vnc.internal:6081/vnc.html?token=tok_abc&network_token=net_token_secret_123"));
}

#[test]
fn test_cua_emergency_takeover_sub_10ms_killswitch() {
    let mut broker = ComputerLeaseBroker::new();
    let mut lease = broker
        .acquire_lease("agent_worker_2", "remote_box_workstation")
        .expect("lease granted");

    assert!(lease.is_valid());
    let initial_gen = broker.current_generation();

    // Human operator presses Esc / hits Kill-Switch
    let start_time = Instant::now();
    let bumped_gen = broker.emergency_takeover();
    let elapsed = start_time.elapsed();

    // 1. Assert sub-10ms takeover execution
    assert!(
        elapsed.as_millis() < 10,
        "Emergency takeover must complete under 10ms, took {:?}",
        elapsed
    );

    // 2. Assert generation counter strictly monotonic
    assert_eq!(bumped_gen, initial_gen + 1);
    assert_eq!(broker.current_generation(), bumped_gen);

    // 3. Assert active lease cleared and preempted
    assert!(!broker.is_lease_active());
    assert!(broker.active_lease().is_none());

    // 4. Assert previous lease cannot execute any CUA actions
    lease.preempt();
    assert!(!lease.is_valid());
    assert_eq!(lease.state, LeaseState::Preempted);

    let action = CuaAction::TypeString {
        text: "sudo rm -rf /".into(),
    };
    let reject_res = CuaExecutor::execute_action_fenced(&action, &lease, broker.current_generation());
    assert!(
        reject_res.is_err(),
        "Preempted lease action must be immediately rejected"
    );
}

// ============================================================================
// 3. Full-Duplex Talk Mode & VAD Barge-In Tests
// ============================================================================

#[test]
fn test_talk_fsm_transitions_and_ocrm_invariants() {
    let mut coord = TalkCoordinator::new();
    assert_eq!(coord.state, TalkSessionState::Idle);
    assert_eq!(coord.generation, 0);
    assert_eq!(coord.signal_seq, 0);
    assert_eq!(coord.buffered_chunks_count(), 0);

    // FSM progression: Idle -> Listening -> Thinking -> Speaking
    assert!(coord.transition(TalkSessionState::Listening).is_ok());
    assert_eq!(coord.state, TalkSessionState::Listening);

    assert!(coord.transition(TalkSessionState::Thinking).is_ok());
    assert_eq!(coord.state, TalkSessionState::Thinking);

    assert!(coord.transition(TalkSessionState::Speaking).is_ok());
    assert_eq!(coord.state, TalkSessionState::Speaking);

    // Enqueue audio chunks
    for i in 0..10 {
        assert!(coord.enqueue_audio(vec![i as u8; 3200]).is_ok());
    }
    assert_eq!(coord.buffered_chunks_count(), 10);

    // Signal sequencing per OCRM-AUDIO-O02
    assert_eq!(coord.next_signal_seq(), 1);
    assert_eq!(coord.next_signal_seq(), 2);

    // Reconnect lifecycle per OCRM-AUDIO-O01
    let new_gen = coord.reconnect();
    assert_eq!(new_gen, 1);
    assert_eq!(coord.state, TalkSessionState::Reconnecting);
    assert_eq!(coord.signal_seq, 0);

    // Close session per OCRM-AUDIO-O04
    coord.close();
    assert_eq!(coord.state, TalkSessionState::Closed);
    assert_eq!(coord.buffered_chunks_count(), 0);

    // Invariant: Closed session cannot transition or enqueue
    assert!(coord.transition(TalkSessionState::Listening).is_err());
    assert!(coord.enqueue_audio(vec![0u8; 100]).is_err());
}

#[test]
fn test_talk_realtime_buffer_overflow_limit_ocrm_audio_o05() {
    let mut coord = TalkCoordinator::new();
    coord.state = TalkSessionState::Speaking;

    // Fill buffer up to MAX_AUDIO_BUFFER_CHUNKS (320 chunks / 1 MiB boundary)
    for i in 0..MAX_AUDIO_BUFFER_CHUNKS {
        let chunk = vec![(i % 256) as u8; 3200];
        assert!(
            coord.enqueue_audio(chunk).is_ok(),
            "Chunk {} should enqueue successfully",
            i
        );
    }
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);

    // Chunk 321 pushes successfully by dropping the oldest chunk (chunk 0)
    let overflow_chunk = vec![0xFF; 3200];
    let overflow_res = coord.enqueue_audio(overflow_chunk);
    assert!(
        overflow_res.is_ok(),
        "Enqueue beyond 320 chunks must succeed by dropping the oldest chunk"
    );
    assert_eq!(coord.buffered_chunks_count(), MAX_AUDIO_BUFFER_CHUNKS);
    // Oldest chunk 0 was dropped, so next dequeued chunk is chunk 1
    assert_eq!(coord.dequeue_audio().unwrap()[0], 1);
}

#[test]
fn test_vad_bargein_detection_and_buffer_flush() {
    let mut detector = VadBargeInDetector::new(500.0);
    let mut coord = TalkCoordinator::new();
    coord.state = TalkSessionState::Speaking;

    // Pre-populate coordinator with 120 audio chunks
    for i in 0..120 {
        coord.enqueue_audio(vec![(i % 128) as u8; 1600]).unwrap();
    }
    assert_eq!(coord.buffered_chunks_count(), 120);

    // 1. Silent PCM frames (< threshold)
    let silent_frame = vec![0i16; 320]; // 20ms @ 16kHz
    assert!(!detector.process_pcm_frame(&silent_frame));
    assert_eq!(detector.consecutive_speech_frames, 0);

    // 2. High-energy PCM speech frame (> threshold)
    let speech_frame = vec![1500i16; 320];

    // Frame 1: 20ms of speech (not yet >= 60ms)
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 1);

    // Frame 2: 40ms of speech
    assert!(!detector.process_pcm_frame(&speech_frame));
    assert_eq!(detector.consecutive_speech_frames, 2);

    // Frame 3: 60ms of speech -> barge-in triggered!
    let barge_in_detected = detector.process_pcm_frame(&speech_frame);
    assert!(
        barge_in_detected,
        "Speech exceeding threshold for 3 frames (60ms) must trigger barge-in"
    );
    assert_eq!(detector.consecutive_speech_frames, 3);

    // 3. Coordinator handles barge-in: instant 320-chunk/1MB playback buffer flush
    let flushed_count = coord.handle_barge_in();
    assert_eq!(flushed_count, 120);
    assert_eq!(coord.buffered_chunks_count(), 0);
    assert_eq!(coord.state, TalkSessionState::Interrupted);

    // 4. Resume listening
    coord.resume_listening();
    assert_eq!(coord.state, TalkSessionState::Listening);

    // 5. Silence detection for turn finalization (>= 25 frames @ 20ms = 500ms)
    detector.reset();
    for _ in 0..24 {
        assert!(!detector.is_silence_detected());
        detector.process_pcm_frame(&silent_frame);
    }
    detector.process_pcm_frame(&silent_frame);
    assert!(
        detector.is_silence_detected(),
        "Silence for 25 frames (500ms) must detect turn completion"
    );
}

#[test]
fn test_waveform_5bit_codec_integration() {
    let detector = VadBargeInDetector::new(300.0);

    // Synthesize 1000 PCM samples
    let samples: Vec<i16> = (0..1000)
        .map(|i| ((i as f64 * 0.1).sin() * 12000.0) as i16)
        .collect();

    // Encode to Telegram standard 5-bit packed waveform
    let encoded = detector.encode_pcm_to_5bit_waveform(&samples);

    // Telegram waveform specification: 100 bars packed at 5 bits per bar = 63 bytes
    assert_eq!(
        encoded.len(),
        63,
        "Packed waveform must be exactly 63 bytes for 100 bars"
    );

    // Decode 5-bit packed bytes back to quantized bars
    let decoded_bars = decode_waveform_5bit(&encoded);
    assert_eq!(decoded_bars.len(), 100);

    // Each bar value must be clamped within 5-bit unsigned integer range (0..=31)
    for &bar in &decoded_bars {
        assert!(bar <= 31, "Waveform bar value {} exceeds 5-bit max 31", bar);
    }
}
