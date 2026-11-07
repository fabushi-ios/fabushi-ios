use mahayana_node_agent_coordinator::control_port_client::{ClientAction, ControlPortClient};
use mahayana_node_agent_coordinator::gateway::{GatewayHealthDecision, GatewayHostSupervisor};
use mahayana_node_agent_coordinator::mcp::{McpRoute, RoutedMcpBridge};
use mahayana_node_agent_coordinator::protocol::{
    CoordinatorFrame, LifecyclePhase, ReplyOutcome, COORDINATOR_CANCELLED,
    COORDINATOR_PROTOCOL_VERSION,
};
use mahayana_node_agent_coordinator::renderer_port_server::{
    RendererPortPhase, RendererPortServer, RendererPortSettlement, ServerAction,
};
use mahayana_node_agent_coordinator::supervisor::{
    CoordinatorSupervisor, GatewayState, HostGeneration,
};
use mahayana_node_agent_coordinator::inference_router::{
    agent_inbound_failure_gateway_args, agent_inbound_wake_prefers_urgent_queue,
    deleted_agent_ids_for_host_success, prepare_agent_inbound_wake_routes,
    redrive_agent_inbound_after_priority_preemption, PRIORITY_AGENT_WAKE_SUPERSEDE_REASON,
};
use serde_json::json;

#[test]
fn protocol_hello_negotiates_exact_version() {
    let mut server = RendererPortServer::default();
    assert_eq!(server.handle_frame(CoordinatorFrame::hello()), vec![
        ServerAction::Post(CoordinatorFrame::ready())
    ]);
    assert_eq!(server.phase(), RendererPortPhase::Serving);

    let mut wrong = RendererPortServer::default();
    let actions = wrong.handle_frame(CoordinatorFrame::Lifecycle {
        phase: LifecyclePhase::Hello,
        protocol_version: Some(COORDINATOR_PROTOCOL_VERSION + 1),
        reason: None,
        detail: None,
    });
    assert!(matches!(actions.last(), Some(ServerAction::Close)));
    assert!(matches!(wrong.settlement(), Some(RendererPortSettlement::ProtocolBreach(_))));
}

#[test]
fn request_ids_are_unique_for_the_session() {
    let mut server = RendererPortServer::default();
    server.handle_frame(CoordinatorFrame::hello());
    server.handle_frame(CoordinatorFrame::Request {
        request_id: "r-1".into(),
        method: "sendPrompt".into(),
        args: json!({}),
    });
    server.complete_request("r-1", ReplyOutcome::Ok { value: json!(true) });
    let actions = server.handle_frame(CoordinatorFrame::Request {
        request_id: "r-1".into(),
        method: "sendPrompt".into(),
        args: json!({}),
    });
    assert!(matches!(actions.last(), Some(ServerAction::Close)));
}

#[test]
fn cancel_aborts_and_settles_the_request() {
    let mut server = RendererPortServer::default();
    server.handle_frame(CoordinatorFrame::hello());
    server.handle_frame(CoordinatorFrame::Request {
        request_id: "r-2".into(),
        method: "sendPrompt".into(),
        args: json!({}),
    });
    let actions = server.handle_frame(CoordinatorFrame::Cancel { request_id: "r-2".into() });
    assert!(actions.iter().any(|a| matches!(a, ServerAction::Abort { request_id } if request_id == "r-2")));
    assert!(actions.iter().any(|a| matches!(
        a,
        ServerAction::Post(CoordinatorFrame::Reply {
            outcome: ReplyOutcome::Failed { failure },
            ..
        }) if failure.code == COORDINATOR_CANCELLED
    )));
}

#[test]
fn waitable_control_calls_resolve_reject_and_settle_on_disconnect() {
    use std::time::Duration;

    let mut client = ControlPortClient::default();
    client.handle_frame(CoordinatorFrame::ready());

    let success = client
        .call_waiting("resolveGatewayConnection", json!({"slot":"host"}))
        .expect("waitable success call");
    assert!(matches!(
        &success.action,
        ClientAction::Post(CoordinatorFrame::Request { request_id, method, .. })
            if request_id == &success.request_id && method == "resolveGatewayConnection"
    ));
    let success_actions = client.handle_frame(CoordinatorFrame::Reply {
        request_id: success.request_id.clone(),
        outcome: ReplyOutcome::Ok {
            value: json!({"baseUrl":"http://127.0.0.1:1234"}),
        },
    });
    assert!(matches!(
        success_actions.as_slice(),
        [ClientAction::Resolve { request_id, .. }] if request_id == &success.request_id
    ));
    assert_eq!(
        success
            .reply
            .recv_timeout(Duration::from_millis(100))
            .expect("success waiter")
            .expect("success result")["baseUrl"],
        "http://127.0.0.1:1234"
    );

    let failed = client
        .call_waiting("mintLocalExecDaemonCredential", json!({}))
        .expect("waitable failed call");
    client.handle_frame(CoordinatorFrame::Reply {
        request_id: failed.request_id.clone(),
        outcome: ReplyOutcome::Failed {
            failure: mahayana_node_agent_coordinator::protocol::Failure::new(
                "DENIED",
                "credential denied",
            ),
        },
    });
    let failure = failed
        .reply
        .recv_timeout(Duration::from_millis(100))
        .expect("failure waiter")
        .expect_err("expected failed result");
    assert_eq!(failure.code, "DENIED");

    let disconnected = client
        .call_waiting("getProcessIdentity", json!({"pid": 7}))
        .expect("waitable disconnected call");
    client.handle_port_closed();
    let failure = disconnected
        .reply
        .recv_timeout(Duration::from_millis(100))
        .expect("disconnect waiter")
        .expect_err("disconnect must reject");
    assert_eq!(failure.code, mahayana_node_agent_coordinator::protocol::COORDINATOR_DISCONNECTED);
}

#[test]
fn disconnect_rejects_every_pending_control_call() {
    let mut client = ControlPortClient::default();
    client.handle_frame(CoordinatorFrame::ready());
    client.call("one", json!({})).unwrap();
    client.call("two", json!({})).unwrap();
    let actions = client.handle_port_closed();
    let rejects = actions.iter().filter(|a| matches!(a, ClientAction::Reject { .. })).count();
    assert_eq!(rejects, 2);
}

#[test]
fn control_port_preserves_client_event_direction_and_rejects_server_events() {
    let mut client = ControlPortClient::default();
    assert!(matches!(
        client.post_event("renderer-event", json!({"ok": true})),
        Some(ClientAction::Post(CoordinatorFrame::Event { .. }))
    ));

    client.handle_frame(CoordinatorFrame::ready());
    let actions = client.handle_frame(CoordinatorFrame::Event {
        family: "server-event".into(),
        payload: json!({}),
    });
    assert!(matches!(
        actions.first(),
        Some(ClientAction::Post(CoordinatorFrame::Lifecycle {
            phase: LifecyclePhase::Shutdown,
            ..
        }))
    ));
    assert!(matches!(actions.last(), Some(ClientAction::Close)));
}

#[test]
fn reconnect_resync_reports_generation_and_pending_state() {
    let mut supervisor = CoordinatorSupervisor::default();
    let first = supervisor.connect();
    assert_eq!(first.coordinator_generation, 1);
    supervisor.register_request("r", "sendPrompt", json!({})).unwrap();
    let snapshot = supervisor.reconnect(HostGeneration(0));
    assert_eq!(snapshot.coordinator_generation, 2);
    assert_eq!(snapshot.pending_request_ids, vec!["r"]);
}

#[test]
fn host_restart_settles_old_generation_requests() {
    let mut supervisor = CoordinatorSupervisor::default();
    supervisor.connect();
    supervisor.register_request("r", "sendPrompt", json!({})).unwrap();
    let settlements = supervisor.host_restarted(HostGeneration(2));
    assert_eq!(settlements.len(), 1);
    assert!(supervisor.snapshot().pending_request_ids.is_empty());
    assert_eq!(supervisor.snapshot().host_generation, HostGeneration(2));
}

#[test]
fn gateway_down_up_is_resynchronizable() {
    let mut supervisor = CoordinatorSupervisor::default();
    supervisor.connect();
    supervisor.set_gateway_state(GatewayState::Down);
    assert_eq!(supervisor.snapshot().gateway, GatewayState::Down);
    supervisor.set_gateway_state(GatewayState::Up);
    assert_eq!(supervisor.snapshot().gateway, GatewayState::Up);

    let mut gateway = GatewayHostSupervisor::new(5_000);
    assert_eq!(gateway.decision(0), GatewayHealthDecision::Probe);
    gateway.record_health(100, false);
    assert_eq!(gateway.decision(101), GatewayHealthDecision::Reconnect);
    gateway.invalidate();
    assert_eq!(gateway.decision(102), GatewayHealthDecision::Probe);
}

#[test]
fn mcp_routing_is_explicit_and_generation_bound() {
    let mut bridge = RoutedMcpBridge::default();
    bridge.upsert(McpRoute {
        server_name: "filesystem".into(),
        transport: "stdio".into(),
        generation: 7,
    });
    let (route, tool, args) = bridge.route_tool_call("filesystem", "read_file", json!({"path":"a"})).unwrap();
    assert_eq!(route.generation, 7);
    assert_eq!(tool, "read_file");
    assert_eq!(args["path"], "a");
    assert!(bridge.route_tool_call("missing", "x", json!({})).is_err());
}

#[test]
fn server_direction_frames_from_renderer_are_protocol_breaches() {
    let mut server = RendererPortServer::default();
    server.handle_frame(CoordinatorFrame::hello());
    let actions = server.handle_frame(CoordinatorFrame::Event {
        family: "illegal".into(),
        payload: json!({}),
    });
    assert!(matches!(actions.last(), Some(ServerAction::Close)));
    assert!(matches!(server.settlement(), Some(RendererPortSettlement::ProtocolBreach(_))));
}

#[test]
fn crash_is_recorded_in_resync_snapshot() {
    let mut supervisor = CoordinatorSupervisor::default();
    supervisor.connect();
    let crash = supervisor.record_crash("host", "process exited 137");
    let snapshot = supervisor.snapshot();
    assert_eq!(snapshot.last_crash.as_ref(), Some(&crash));
}


#[test]
fn gateway_event_families_match_the_frozen_coordinator_contract() {
    use mahayana_node_agent_coordinator::gateway::gateway_event_families::{
        coordinator_event_family_for_sse_channel, event_family_for_channel, sse_channel_for_family,
    };

    assert_eq!(sse_channel_for_family("agents-workflow"), Some("workflows"));
    assert_eq!(sse_channel_for_family("mcp-servers-updated"), Some("mcp-servers"));
    assert_eq!(sse_channel_for_family("host-settings"), Some("host-settings"));
    assert_eq!(
        sse_channel_for_family("cloud-workers.status"),
        Some("cloud-workers.status")
    );
    assert_eq!(coordinator_event_family_for_sse_channel("automations"), Some("agents-automation"));
    assert_eq!(
        coordinator_event_family_for_sse_channel("cloud-workers.status"),
        Some("cloud-workers.status")
    );
    assert_eq!(coordinator_event_family_for_sse_channel("unknown"), None);
    assert_eq!(event_family_for_channel("cloud-workers.status"), "cloud-workers.status");
    assert_eq!(event_family_for_channel("channel-activity"), "channel-activity");
    assert_eq!(event_family_for_channel("  "), "runtime");
}

#[test]
fn sse_decoder_preserves_chunk_boundaries_and_utf8_payloads() {
    use mahayana_node_agent_coordinator::gateway::sse_block_decoder::SseBlockDecoder;

    let mut decoder = SseBlockDecoder::default();
    assert!(decoder.push_bytes(b"event: transcript\ndata: ").is_empty());
    let unicode = "法布施".as_bytes();
    assert!(decoder.push_bytes(&unicode[..2]).is_empty());
    assert!(decoder.push_bytes(&unicode[2..]).is_empty());
    let blocks = decoder.push_bytes(b"\n\nevent: agents\ndata: ok\n\n");
    assert_eq!(blocks, vec![
        "event: transcript\ndata: 法布施".to_string(),
        "event: agents\ndata: ok".to_string(),
    ]);

    let mut boundary = SseBlockDecoder::default();
    assert!(boundary.push("event: one\n").is_empty());
    assert_eq!(boundary.push("\nevent: two\n\n"), vec![
        "event: one".to_string(),
        "event: two".to_string(),
    ]);
}

#[test]
fn gateway_reachability_matches_http_network_and_base_url_semantics() {
    use mahayana_node_agent_coordinator::gateway::gateway_reachability::{
        classify_base_url_kind, classify_system_error, outcome_for_http_status, BaseUrlKind,
        ReachabilityOutcome,
    };

    assert_eq!(outcome_for_http_status(503), Some(ReachabilityOutcome::Http(503)));
    assert_eq!(outcome_for_http_status(403), Some(ReachabilityOutcome::AccessDenied));
    assert_eq!(outcome_for_http_status(404), None);
    assert_eq!(
        classify_system_error(Some("ECONNREFUSED"), false, ""),
        ReachabilityOutcome::Refused
    );
    assert_eq!(
        classify_system_error(Some("EAI_AGAIN"), false, ""),
        ReachabilityOutcome::Dns
    );
    assert_eq!(
        classify_system_error(None, true, ""),
        ReachabilityOutcome::Timeout
    );
    assert!(!ReachabilityOutcome::AccessDenied.retryable());
    assert!(ReachabilityOutcome::Http(503).retryable());
    assert_eq!(
        classify_base_url_kind(Some("http://127.0.0.1:7777")),
        BaseUrlKind::Loopback
    );
    assert_eq!(
        classify_base_url_kind(Some("https://agent.us8.cursorvm.com")),
        BaseUrlKind::PodProxy
    );
    assert_eq!(classify_base_url_kind(Some("not a url")), BaseUrlKind::Unknown);
}


#[test]
fn box_vnc_proxy_maps_primary_and_fork_viewers_without_touching_foreign_urls() {
    use mahayana_node_agent_coordinator::gateway::box_vnc_proxy::{
        proxify_box_vnc_url, proxify_forever_box_status, VncProxyDescriptor,
    };

    let descriptor = VncProxyDescriptor {
        primary_url: "https://proxy.example/sand-special-treatment-v1/vnc.html?primary=1".into(),
        fork_base_url: "https://fork.example/base".into(),
        network_token: "network-123".into(),
    };
    assert_eq!(
        proxify_box_vnc_url("http://127.0.0.1:6080/vnc.html", &descriptor),
        descriptor.primary_url
    );
    let fork = proxify_box_vnc_url(
        "http://localhost:6081/vnc.html?path=websockify%3Ftoken%3Ddisplay-7",
        &descriptor,
    );
    assert!(fork.starts_with("https://fork.example/base/sand-special-treatment-v1/vnc.html?"));
    assert!(fork.contains("network_token=network-123"));
    assert!(fork.contains("path=websockify%3Ftoken%3Ddisplay-7%26network_token%3Dnetwork-123"));
    assert_eq!(
        proxify_box_vnc_url("https://remote.example:6080/vnc.html", &descriptor),
        "https://remote.example:6080/vnc.html"
    );

    let status = json!({
        "vncUrl": "http://127.0.0.1:6080/vnc.html",
        "windows": [{"vncUrl": "http://127.0.0.1:6081/vnc.html"}],
        "state": "ready"
    });
    let projected = proxify_forever_box_status(&status, Some(&descriptor));
    assert_eq!(projected["vncUrl"], descriptor.primary_url);
    assert!(projected["windows"][0]["vncUrl"].as_str().unwrap().contains("network_token=network-123"));
    assert_eq!(projected["state"], "ready");
}


#[test]
fn local_exec_daemon_files_parse_and_retire_only_the_expected_generation() {
    use mahayana_node_agent_coordinator::local_exec::daemon_files::{
        parse_discovery, read_local_exec_daemon_discovery,
        remove_local_exec_daemon_discovery_if_matches, resolve_local_exec_daemon_paths,
        write_secret_json_file,
    };
    use std::fs;
    use uuid::Uuid;

    let root = std::env::temp_dir().join(format!("fabushi-local-exec-{}", Uuid::new_v4()));
    let paths = resolve_local_exec_daemon_paths(&root);
    assert!(paths.discovery_path.ends_with("local-exec-daemon.json"));
    let value = json!({
        "pid": 4242,
        "startedAt": 1234.5,
        "entryRealpath": "/tmp/daemon",
        "generationToken": "generation-a",
        "inflightCount": 2
    });
    let expected = parse_discovery(value.clone()).expect("valid discovery");
    write_secret_json_file(&paths.discovery_path, &value).expect("write discovery");
    assert_eq!(
        read_local_exec_daemon_discovery(&paths.discovery_path)
            .expect("read discovery")
            .as_ref(),
        Some(&expected)
    );

    let wrong = parse_discovery(json!({
        "pid": 4242,
        "startedAt": 1234.5,
        "entryRealpath": "/tmp/daemon",
        "generationToken": "generation-b"
    })).unwrap();
    assert!(!remove_local_exec_daemon_discovery_if_matches(&paths.discovery_path, &wrong)
        .expect("mismatched generation is restored"));
    assert!(paths.discovery_path.exists());

    assert!(remove_local_exec_daemon_discovery_if_matches(&paths.discovery_path, &expected)
        .expect("matching generation retires"));
    assert!(!paths.discovery_path.exists());
    let _ = fs::remove_dir_all(root);
}


#[test]
fn carrier_bootstrap_and_channels_preserve_grok_process_boundaries() {
    use mahayana_node_agent_coordinator::carrier::{
        parse_bootstrap_argument, Carrier, CarrierChannel, CarrierEnvelope,
    };

    let argument = r#"--bootstrap={"processConfig":{"appVersion":"1.2.75","isPackaged":true,"dataDir":"/tmp/fabushi","localHumanId":"human-local"}}"#;
    let bootstrap = parse_bootstrap_argument([argument]).expect("valid bootstrap");
    assert_eq!(bootstrap.process_config.app_version, "1.2.75");
    assert!(bootstrap.process_config.is_packaged);
    assert_eq!(bootstrap.process_config.local_human_id.as_deref(), Some("human-local"));

    let mut carrier = Carrier::new(bootstrap);
    carrier
        .accept_envelope(CarrierEnvelope::new(
            CarrierChannel::Control,
            json!({"kind":"control"}),
        ))
        .unwrap();
    carrier
        .accept_envelope(CarrierEnvelope::new(
            CarrierChannel::Data,
            json!({"kind":"renderer"}),
        ))
        .unwrap();
    carrier
        .accept_envelope(CarrierEnvelope::new(
            CarrierChannel::MainData,
            json!({"kind":"main"}),
        ))
        .unwrap();
    let messages = carrier.drain();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].channel, CarrierChannel::Control);
    assert_eq!(messages[1].channel, CarrierChannel::Data);
    assert_eq!(messages[2].channel, CarrierChannel::MainData);
    carrier.close();
    assert!(carrier.post(CarrierChannel::Data, json!({})).is_err());
}


#[test]
fn client_side_tool_v2_relay_fences_epochs_sequences_and_replays_wire_payloads() {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use mahayana_node_agent_coordinator::client_side_tool_v2_relay::{
        ClientSideToolV2Relay, EncodedToolMessage, ToolMessageKind, ToolTransportEvent,
        CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT, CLIENT_SIDE_TOOL_V2_WIRE_VERSION,
    };

    fn encoded(kind: ToolMessageKind, call_id: &str) -> EncodedToolMessage {
        let mut wire = Vec::new();
        match kind {
            ToolMessageKind::Call => wire.push((3_u8 << 3) | 2),
            ToolMessageKind::Result => {
                wire.push(0x9a);
                wire.push(0x02);
            }
            ToolMessageKind::Reset => unreachable!(),
        }
        wire.push(call_id.len() as u8);
        wire.extend_from_slice(call_id.as_bytes());
        EncodedToolMessage {
            encoding: "protobuf-base64".into(),
            message_type: match kind {
                ToolMessageKind::Call => "aiserver.v1.ClientSideToolV2Call",
                ToolMessageKind::Result => "aiserver.v1.ClientSideToolV2Result",
                ToolMessageKind::Reset => unreachable!(),
            }.into(),
            bytes: STANDARD.encode(wire),
        }
    }

    fn event(epoch: &str, sequence: u64, kind: ToolMessageKind, call_id: &str) -> ToolTransportEvent {
        ToolTransportEvent {
            version: CLIENT_SIDE_TOOL_V2_WIRE_VERSION,
            kind,
            account_slot: CLIENT_SIDE_TOOL_V2_ACCOUNT_SLOT.into(),
            agent_id: "agent-1".into(),
            epoch: epoch.into(),
            sequence,
            message: (kind != ToolMessageKind::Reset).then(|| encoded(kind, call_id)),
        }
    }

    let mut relay = ClientSideToolV2Relay::default();
    let call = relay.accept(event("epoch-a", 1, ToolMessageKind::Call, "call-7")).unwrap();
    assert_eq!(call.bytes.as_deref(), Some(&[0x1a, 0x06, b'c', b'a', b'l', b'l', b'-', b'7'][..]));
    assert!(relay.accept(event("epoch-a", 1, ToolMessageKind::Call, "duplicate")).is_none());
    assert!(relay.accept(event("epoch-a", 2, ToolMessageKind::Result, "call-7")).is_some());
    assert_eq!(relay.replay().len(), 2);

    assert!(relay.accept(event("epoch-b", 1, ToolMessageKind::Call, "call-8")).is_some());
    assert!(relay.accept(event("epoch-a", 3, ToolMessageKind::Result, "call-7")).is_none());
    assert_eq!(relay.replay().len(), 1);

    assert!(relay.accept(event("epoch-b", 2, ToolMessageKind::Reset, "")).is_some());
    assert!(relay.replay().is_empty());
}


#[test]
fn gateway_dispatch_preserves_command_unreachable_and_transport_failures() {
    use mahayana_node_agent_coordinator::gateway::gateway_errors::SandGatewayCommandError;
    use mahayana_node_agent_coordinator::gateway::gateway_reachability::ReachabilityOutcome;
    use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::{
        GatewayDispatchError, GatewayRequestDispatcher, GATEWAY_COMMAND_FAILED,
        GATEWAY_TRANSPORT_FAILED, GATEWAY_UNREACHABLE,
    };

    let mut dispatcher = GatewayRequestDispatcher::default();
    dispatcher.register("command", Box::new(|_| {
        Err(GatewayDispatchError::Command(SandGatewayCommandError::new("bad command")))
    }));
    dispatcher.register("dns", Box::new(|_| {
        Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: "dns failed".into(),
        })
    }));
    dispatcher.register("transport", Box::new(|_| {
        Err(GatewayDispatchError::Transport("socket closed".into()))
    }));

    let command = dispatcher.dispatch("command", json!({})).unwrap_err();
    assert_eq!(command.code, GATEWAY_COMMAND_FAILED);
    assert_eq!(command.transport_kind, None);

    let dns = dispatcher.dispatch("dns", json!({})).unwrap_err();
    assert_eq!(dns.code, GATEWAY_UNREACHABLE);
    assert_eq!(dns.transport_kind.as_deref(), Some("dns"));

    let transport = dispatcher.dispatch("transport", json!({})).unwrap_err();
    assert_eq!(transport.code, GATEWAY_TRANSPORT_FAILED);

    let serialized = serde_json::to_value(dns).unwrap();
    assert_eq!(serialized["transportKind"], "dns");
}


#[test]
fn gateway_dns_diagnostics_match_cursorvm_target_and_failure_matrix() {
    use mahayana_node_agent_coordinator::gateway::gateway_dns_diagnostics::{
        classify_dns_diagnosis, classify_probe_error, dns_target_from_base_url, DnsCluster,
        DnsDiagnosis, DnsProbeResult, GatewayDnsDiagnosticReporter,
        DNS_PROBE_MIN_INTERVAL_MS,
    };

    let target = dns_target_from_base_url(
        Some("https://agent-7.us8.cursorvm.com/api"),
        "probe-1",
    )
    .expect("cursorvm endpoint");
    assert_eq!(target.cluster, DnsCluster::Us8);
    assert_eq!(target.endpoint_hostname, "agent-7.us8.cursorvm.com");
    assert_eq!(target.wildcard_hostname, "probe-1.us8.cursorvm.com");
    assert!(dns_target_from_base_url(Some("http://agent-7.us8.cursorvm.com"), "probe").is_none());
    assert_eq!(classify_probe_error(Some("ENOTFOUND"), false), DnsProbeResult::NotFound);
    assert_eq!(classify_probe_error(Some("EAI_AGAIN"), false), DnsProbeResult::TemporaryFailure);
    assert_eq!(
        classify_dns_diagnosis(
            DnsProbeResult::NotFound,
            DnsProbeResult::Resolved,
            DnsProbeResult::Resolved,
            DnsProbeResult::Resolved,
        ),
        DnsDiagnosis::SystemPathFailure
    );
    assert_eq!(
        classify_dns_diagnosis(
            DnsProbeResult::NotFound,
            DnsProbeResult::NotFound,
            DnsProbeResult::Resolved,
            DnsProbeResult::Resolved,
        ),
        DnsDiagnosis::EndpointFailure
    );

    let mut reporter = GatewayDnsDiagnosticReporter::default();
    assert!(reporter.diagnose(
        1,
        DnsCluster::Us8,
        DnsProbeResult::NotFound,
        DnsProbeResult::NotFound,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
    ).is_some());
    assert!(reporter.diagnose(
        1 + DNS_PROBE_MIN_INTERVAL_MS - 1,
        DnsCluster::Us8,
        DnsProbeResult::NotFound,
        DnsProbeResult::NotFound,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
    ).is_none());
    assert!(reporter.diagnose(
        1 + DNS_PROBE_MIN_INTERVAL_MS,
        DnsCluster::Us8,
        DnsProbeResult::NotFound,
        DnsProbeResult::NotFound,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
        DnsProbeResult::Resolved,
    ).is_some());
}


#[test]
fn gateway_host_supervisor_rejects_stale_connection_resolution_after_invalidation() {
    use mahayana_node_agent_coordinator::gateway::host_supervisor::{
        GatewayConnection, GatewayHealthDecision, GatewayHostSupervisor,
    };
    use std::collections::BTreeMap;

    let mut supervisor = GatewayHostSupervisor::new(5_000);
    let stale = supervisor.begin_connection_attempt();
    supervisor.invalidate();
    let fresh = supervisor.begin_connection_attempt();
    assert_ne!(stale.health_epoch, fresh.health_epoch);

    assert!(supervisor
        .settle_connection_attempt(
            stale,
            GatewayConnection {
                base_url: "https://stale.example".into(),
                headers: BTreeMap::new(),
            },
        )
        .is_err());

    supervisor
        .settle_connection_attempt(
            fresh,
            GatewayConnection {
                base_url: "https://fresh.example".into(),
                headers: BTreeMap::new(),
            },
        )
        .expect("fresh connection wins");
    assert_eq!(supervisor.connection().unwrap().base_url, "https://fresh.example");
    assert_eq!(supervisor.decision(0), GatewayHealthDecision::Probe);
    supervisor.record_health(100, true);
    assert_eq!(supervisor.decision(101), GatewayHealthDecision::UseCached);
    supervisor.mark_transport_live(true);
    assert_eq!(supervisor.decision(10_000), GatewayHealthDecision::UseCached);
}


#[test]
fn gateway_client_tracks_endpoint_stream_stall_retry_and_permanent_refusal() {
    use mahayana_node_agent_coordinator::gateway::gateway_client::{
        extract_gateway_error_message, CoordinatorGatewayClient, GatewayClientTiming,
        SSE_RECONNECT_MAX_MS, SSE_RECONNECT_MIN_MS,
    };
    use mahayana_node_agent_coordinator::gateway::gateway_reachability::ReachabilityOutcome;
    use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
    use std::collections::BTreeMap;
    use std::time::Duration;

    assert_eq!(
        extract_gateway_error_message(r#"{"error":"gateway said no"}"#).as_deref(),
        Some("gateway said no")
    );

    let mut client = CoordinatorGatewayClient::default();
    client
        .install_connection(GatewayConnection {
            base_url: "https://gateway.example".into(),
            headers: BTreeMap::new(),
        })
        .unwrap();
    client.start(100).unwrap();
    assert!(client.state.stream_live);
    assert!(!client.state.stream_stalled(100, GatewayClientTiming::default()));
    client.state.mark_event(200);
    assert!(client
        .state
        .stream_stalled(200 + GatewayClientTiming::default().sse_stall_timeout_ms, GatewayClientTiming::default()));

    let first = client.transport_down(ReachabilityOutcome::Dns).unwrap();
    assert_eq!(first, Duration::from_millis(SSE_RECONNECT_MIN_MS));
    let mut last = first;
    for _ in 0..8 {
        last = client.transport_down(ReachabilityOutcome::Network).unwrap();
    }
    assert_eq!(last, Duration::from_millis(SSE_RECONNECT_MAX_MS));
    assert_eq!(
        client.transport_down(ReachabilityOutcome::AccessDenied),
        None
    );
}

#[test]
fn local_exec_supervisor_matches_grok_spawn_adopt_replace_policy() {
    use mahayana_node_agent_coordinator::local_exec::{
        decide_local_exec_daemon_action, LocalExecDaemonAction, LocalExecDaemonOrigin,
        LocalExecDaemonState, LocalExecSupervisor, LOCAL_EXEC_DAEMON_LIVENESS_INTERVAL_MS,
        LOCAL_EXEC_DAEMON_READINESS_POLL_MS, LOCAL_EXEC_DAEMON_READINESS_TIMEOUT_MS,
        LOCAL_EXEC_DAEMON_REFRESH_INTERVAL_MS, LOCAL_EXEC_DAEMON_RESPAWN_LIMIT,
    };
    use mahayana_node_agent_coordinator::local_exec::daemon_files::LocalExecDaemonDiscovery;

    assert_eq!(LOCAL_EXEC_DAEMON_REFRESH_INTERVAL_MS, 30_000);
    assert_eq!(LOCAL_EXEC_DAEMON_LIVENESS_INTERVAL_MS, 1_000);
    assert_eq!(LOCAL_EXEC_DAEMON_READINESS_TIMEOUT_MS, 5_000);
    assert_eq!(LOCAL_EXEC_DAEMON_READINESS_POLL_MS, 50);
    assert_eq!(LOCAL_EXEC_DAEMON_RESPAWN_LIMIT, 10);
    assert_eq!(decide_local_exec_daemon_action(None), LocalExecDaemonAction::Spawn);

    let idle = LocalExecDaemonDiscovery {
        pid: 41,
        started_at: 1.0,
        entry_realpath: Some("/tmp/daemon".into()),
        generation_token: Some("g-1".into()),
        inflight_count: Some(0),
    };
    assert_eq!(
        decide_local_exec_daemon_action(Some(&idle)),
        LocalExecDaemonAction::Replace { pid: 41 }
    );

    let busy = LocalExecDaemonDiscovery {
        inflight_count: Some(2),
        ..idle.clone()
    };
    assert_eq!(
        decide_local_exec_daemon_action(Some(&busy)),
        LocalExecDaemonAction::Adopt { pid: 41 }
    );

    let mut supervisor = LocalExecSupervisor::default();
    assert_eq!(
        supervisor.reconcile_discovery(Some(&busy)),
        LocalExecDaemonAction::Adopt { pid: 41 }
    );
    assert_eq!(supervisor.state(), &LocalExecDaemonState::Adopting { pid: 41 });
    supervisor
        .mark_active(
            LocalExecDaemonOrigin::Adopted,
            41,
            10,
            "fabushi-local-exec --generation g-1",
            "/tmp/daemon",
            "g-1",
        )
        .unwrap();
    assert!(matches!(
        supervisor.state(),
        LocalExecDaemonState::Active {
            origin: LocalExecDaemonOrigin::Adopted,
            pid: 41,
            ..
        }
    ));
}


#[test]
fn transport_stage_recorder_bounds_echo_correlation_and_settles_once() {
    use mahayana_node_agent_coordinator::telemetry::transport_stage_recorder::{
        TransportIdentity, TransportReportLimiter, TransportStageRecorder,
        MAX_IN_FLIGHT_TRANSPORT_REPORTS, PENDING_SEND_ECHO_MAX,
        PENDING_SEND_ECHO_TTL_MS, SSE_ECHO_STAGE,
    };

    let mut recorder = TransportStageRecorder::default();
    assert!(recorder
        .begin_send(
            TransportIdentity {
                account_slot: "host".into(),
                client_nonce: None,
                traceparent: Some("00-root".into()),
            },
            0,
        )
        .is_none());

    let trace = recorder
        .begin_send(
            TransportIdentity {
                account_slot: "host".into(),
                client_nonce: Some("nonce-1".into()),
                traceparent: Some("00-root".into()),
            },
            10,
        )
        .expect("send trace");
    let mut stage = trace.begin_stage("gateway-post", 2, 1_000, 20);
    let completed = stage.complete(45).expect("first settlement");
    assert_eq!(completed.duration_ms, 25);
    assert!(!completed.is_error);
    assert!(stage.fail(50).is_none(), "stage settlement is idempotent");

    let echo = recorder
        .record_send_echo("host", "nonce-1", 1_100, 50)
        .expect("echo report");
    assert_eq!(echo.stage, SSE_ECHO_STAGE);
    assert_eq!(echo.traceparent.as_deref(), Some("00-root"));
    assert!(recorder
        .record_send_echo("host", "nonce-1", 1_101, 51)
        .is_none());

    for index in 0..=PENDING_SEND_ECHO_MAX {
        recorder
            .begin_send(
                TransportIdentity {
                    account_slot: "host".into(),
                    client_nonce: Some(format!("nonce-{index}")),
                    traceparent: Some(format!("00-{index}")),
                },
                100 + index as u64,
            )
            .expect("bounded trace");
    }
    assert_eq!(recorder.pending_echo_count(), PENDING_SEND_ECHO_MAX);
    assert!(recorder
        .record_send_echo("host", "nonce-0", 2_000, 200)
        .is_none(), "oldest pending echo is evicted");

    recorder
        .begin_send(
            TransportIdentity {
                account_slot: "host".into(),
                client_nonce: Some("expired".into()),
                traceparent: Some("00-expired".into()),
            },
            500,
        )
        .expect("expiring trace");
    assert!(recorder
        .record_send_echo(
            "host",
            "expired",
            2_500,
            500 + PENDING_SEND_ECHO_TTL_MS + 1,
        )
        .is_none());

    let mut limiter = TransportReportLimiter::default();
    for _ in 0..MAX_IN_FLIGHT_TRANSPORT_REPORTS {
        assert!(limiter.try_begin());
    }
    assert!(!limiter.try_begin());
    limiter.settle();
    assert!(limiter.try_begin());
}


#[test]
fn routed_mcp_bridge_serves_private_json_rpc_and_routes_cached_tools() {
    use mahayana_node_agent_coordinator::routed_mcp_bridge::{
        RoutedMcpBackend, RoutedMcpHttpOutcome, RoutedMcpProtocolBridge, RoutedTool,
        RoutedToolCall, ROUTED_MCP_MAX_BODY_BYTES, ROUTED_MCP_PROTOCOL_VERSION,
    };
    use mahayana_node_agent_coordinator::Failure;
    use serde_json::{Value, json};

    #[derive(Default)]
    struct Backend {
        calls: Vec<RoutedToolCall>,
    }

    impl RoutedMcpBackend for Backend {
        fn list_tools(&mut self) -> Result<Vec<RoutedTool>, Failure> {
            Ok(vec![
                RoutedTool {
                    name: "github_search".into(),
                    provider_identifier: "github".into(),
                    tool_name: "search".into(),
                    description: Some("Search repositories".into()),
                    input_schema: Some(json!({ "type": "object" })),
                },
                RoutedTool {
                    name: "github_create_issue".into(),
                    provider_identifier: "github".into(),
                    tool_name: "create".into(),
                    description: Some("Create issue".into()),
                    input_schema: None,
                },
            ])
        }

        fn call_tool(&mut self, call: RoutedToolCall) -> Result<Value, Failure> {
            self.calls.push(call);
            Ok(json!({
                "result": {
                    "case": "success",
                    "value": {
                        "isError": false,
                        "content": [{
                            "content": {
                                "case": "text",
                                "value": { "text": "tool-result" }
                            }
                        }]
                    }
                }
            }))
        }
    }

    fn result(outcome: RoutedMcpHttpOutcome) -> Value {
        match outcome {
            RoutedMcpHttpOutcome::Json(value) => value,
            other => panic!("expected JSON reply, got {other:?}"),
        }
    }

    let mut bridge = RoutedMcpProtocolBridge::with_secret("secret").expect("bridge");
    let mut backend = Backend::default();
    assert_eq!(bridge.path(), "/mcp/secret");
    assert_eq!(
        bridge.local_url(43123).expect("url"),
        "http://127.0.0.1:43123/mcp/secret"
    );
    assert!(matches!(
        bridge.handle_http("GET", "/mcp/secret", b"{}", &mut backend),
        RoutedMcpHttpOutcome::HttpError(404)
    ));
    assert!(matches!(
        bridge.handle_http(
            "POST",
            "/mcp/secret",
            &vec![b'x'; ROUTED_MCP_MAX_BODY_BYTES + 1],
            &mut backend,
        ),
        RoutedMcpHttpOutcome::HttpError(413)
    ));

    let initialized = result(bridge.handle_http(
        "POST",
        "/mcp/secret",
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
        &mut backend,
    ));
    assert_eq!(
        initialized["result"]["protocolVersion"],
        ROUTED_MCP_PROTOCOL_VERSION
    );
    assert!(matches!(
        bridge.handle_http(
            "POST",
            "/mcp/secret",
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut backend,
        ),
        RoutedMcpHttpOutcome::Accepted
    ));

    let tools = result(bridge.handle_http(
        "POST",
        "/mcp/secret",
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        &mut backend,
    ));
    assert_eq!(bridge.cached_tool_count(), 2);
    let rows = tools["result"]["tools"].as_array().expect("tools");
    let search = rows
        .iter()
        .find(|tool| tool["name"] == "github_search")
        .expect("search tool");
    assert_eq!(search["annotations"]["readOnlyHint"], true);
    let create = rows
        .iter()
        .find(|tool| tool["name"] == "github_create_issue")
        .expect("create tool");
    assert_eq!(create["annotations"]["readOnlyHint"], false);

    let called = result(bridge.handle_http(
        "POST",
        "/mcp/secret",
        br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"github_search","arguments":{"q":"rust"}}}"#,
        &mut backend,
    ));
    assert_eq!(called["result"]["isError"], false);
    assert_eq!(called["result"]["content"][0]["text"], "tool-result");
    assert_eq!(backend.calls.len(), 1);
    assert_eq!(backend.calls[0].provider_identifier, "github");
    assert_eq!(backend.calls[0].args["q"], "rust");
    assert!(!backend.calls[0].tool_call_id.is_empty());

    let unknown = result(bridge.handle_http(
        "POST",
        "/mcp/secret",
        br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"missing"}}"#,
        &mut backend,
    ));
    assert_eq!(unknown["result"]["isError"], true);
}


#[test]
fn oauth_forwarder_scopes_loopback_state_ttl_and_listener_lifecycle() {
    use mahayana_node_agent_coordinator::oauth::mcp_oauth_callback_listener::{
        OAuthCallbackDisposition, classify_callback,
    };
    use mahayana_node_agent_coordinator::oauth::mcp_oauth_forwarder::{
        McpOAuthForwarderState, McpOAuthPendingPayload, OAuthForwarderAction,
        MCP_OAUTH_PENDING_TTL_MS,
    };
    use mahayana_node_agent_coordinator::oauth::mcp_oauth_loopback_registry::{
        McpOAuthLoopbackRegistry, loopback_bind_hosts, parse_loopback_redirect,
    };

    let parsed = parse_loopback_redirect("http://localhost:43124/oauth/callback")
        .expect("loopback redirect");
    assert_eq!(parsed.origin, "http://localhost:43124");
    assert_eq!(parsed.path, "/oauth/callback");
    assert_eq!(loopback_bind_hosts("localhost"), vec!["127.0.0.1", "::1"]);
    assert!(parse_loopback_redirect("https://localhost:43124/oauth/callback").is_err());
    assert!(parse_loopback_redirect("http://example.com:43124/oauth/callback").is_err());
    assert!(parse_loopback_redirect("http://localhost/oauth/callback").is_err());

    let mut pool = McpOAuthLoopbackRegistry::default();
    let first = pool
        .acquire("http://localhost:43124/oauth/callback")
        .expect("first lease");
    let second = pool
        .acquire("http://localhost:43124/other")
        .expect("shared origin lease");
    assert_eq!(pool.active_origin_count(), 1);
    assert_eq!(pool.lease_count(&first.origin), 2);
    assert!(pool.release(&first));
    assert_eq!(pool.lease_count(&second.origin), 1);
    assert!(pool.release(&second));
    assert_eq!(pool.active_origin_count(), 0);

    let mut forwarder = McpOAuthForwarderState::default();
    let actions = forwarder
        .handle_pending(
            McpOAuthPendingPayload {
                redirect_url: "http://localhost:43124/oauth/callback".into(),
                state: "state-1".into(),
                server_name: "github".into(),
            },
            1_000,
        )
        .expect("pending");
    assert!(matches!(
        actions.as_slice(),
        [OAuthForwarderAction::StartListener { origin, .. }]
            if origin == "http://localhost:43124"
    ));
    assert!(forwarder
        .listener_started("http://localhost:43124", 1_001)
        .is_empty());
    assert_eq!(forwarder.listener_count(), 1);
    assert_eq!(
        forwarder.resolve_server("http://localhost:43124", "state-1", 1_002),
        Some("github")
    );

    let disposition = classify_callback(
        "/oauth/callback?state=state-1&code=abc%2B123",
        "/oauth/callback",
        |state| forwarder
            .resolve_server("http://localhost:43124", state, 1_003)
            .map(str::to_string),
    )
    .expect("callback classification");
    let OAuthCallbackDisposition::Complete(callback) = disposition else {
        panic!("expected completed callback");
    };
    assert_eq!(callback.code, "abc+123");
    assert_eq!(callback.server_name, "github");

    let completion = forwarder
        .begin_callback(
            "http://localhost:43124",
            callback.code,
            callback.state.clone(),
            1_004,
        )
        .expect("completion");
    assert!(matches!(
        completion,
        OAuthForwarderAction::Complete { callback }
            if callback.server_name == "github" && callback.state == "state-1"
    ));
    let settled = forwarder.settle_callback(
        "http://localhost:43124",
        &callback.state,
        1_005,
    );
    assert!(matches!(
        settled.as_slice(),
        [OAuthForwarderAction::CloseListener { origin }]
            if origin == "http://localhost:43124"
    ));
    assert_eq!(forwarder.pending_count(), 0);
    assert_eq!(forwarder.listener_count(), 0);

    forwarder
        .handle_pending(
            McpOAuthPendingPayload {
                redirect_url: "http://127.0.0.1:43125/callback".into(),
                state: "expiring".into(),
                server_name: "drive".into(),
            },
            10_000,
        )
        .expect("expiring pending");
    forwarder.listener_started("http://127.0.0.1:43125", 10_001);
    let expired = forwarder.expire(10_000 + MCP_OAUTH_PENDING_TTL_MS);
    assert!(matches!(
        expired.as_slice(),
        [OAuthForwarderAction::CloseListener { origin }]
            if origin == "http://127.0.0.1:43125"
    ));
    assert_eq!(forwarder.pending_count(), 0);
}


#[test]
fn webauthn_provider_models_welcome_consent_sign_and_failure_frames() {
    use std::cell::Cell;
    use std::rc::Rc;

    use mahayana_node_agent_coordinator::webauthn::provider::{
        WebAuthnProvider, WebAuthnRequestFrame, WebAuthnResponseFrame,
    };
    use mahayana_node_agent_coordinator::webauthn::signer::{
        SignerEvent, describe_signer_event_as_status, parse_signer_event_line,
        SIGNER_EVENT_PREFIX,
    };
    use mahayana_node_agent_coordinator::webauthn::{
        ApprovedWebAuthnConsent, WebAuthnCeremony, WebAuthnSigner, WebAuthnSignerResult,
    };
    use serde_json::json;

    struct Signer {
        calls: Rc<Cell<usize>>,
    }

    impl WebAuthnSigner for Signer {
        fn sign(
            &mut self,
            ceremony: &WebAuthnCeremony,
            _approved: Option<&ApprovedWebAuthnConsent>,
        ) -> WebAuthnSignerResult {
            self.calls.set(self.calls.get() + 1);
            WebAuthnSignerResult::Success {
                credential_json: json!({
                    "origin": ceremony.origin,
                    "kind": ceremony.kind,
                }),
            }
        }
    }

    let calls = Rc::new(Cell::new(0));
    let mut provider = WebAuthnProvider::new(
        Signer { calls: calls.clone() },
        Some("computer-1".into()),
        Some("Fabushi".into()),
    );

    let welcome = provider.handle_frame(
        WebAuthnRequestFrame::Welcome {
            provider_id: "provider-1".into(),
        },
        None,
    );
    assert_eq!(provider.provider_id(), Some("provider-1"));
    assert!(matches!(
        welcome.as_slice(),
        [WebAuthnResponseFrame::Hello { computer_id, label }]
            if computer_id.as_deref() == Some("computer-1")
                && label.as_deref() == Some("Fabushi")
    ));
    assert!(matches!(provider.heartbeat(), WebAuthnResponseFrame::Ping));

    let ceremony = WebAuthnCeremony {
        kind: "get".into(),
        origin: "https://example.test".into(),
        payload: json!({ "challenge": "abc" }),
    };
    let approved = provider.handle_frame(
        WebAuthnRequestFrame::Ceremony {
            request_id: "request-1".into(),
            ceremony: ceremony.clone(),
        },
        Some(ApprovedWebAuthnConsent::approved()),
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(provider.in_flight_count(), 0);
    assert!(matches!(
        approved.as_slice(),
        [
            WebAuthnResponseFrame::Stage { stage: "grant", outcome: "ok", .. },
            WebAuthnResponseFrame::Stage { stage: "sign", outcome: "ok", .. },
            WebAuthnResponseFrame::Result { credential_json, .. },
        ] if credential_json["origin"] == "https://example.test"
    ));

    let declined = provider.handle_frame(
        WebAuthnRequestFrame::Ceremony {
            request_id: "request-2".into(),
            ceremony,
        },
        Some(ApprovedWebAuthnConsent::declined()),
    );
    assert_eq!(calls.get(), 1, "declined consent must not invoke the signer");
    assert!(matches!(
        declined.as_slice(),
        [
            WebAuthnResponseFrame::Stage { stage: "grant", outcome: "declined", .. },
            WebAuthnResponseFrame::Error { .. },
        ]
    ));

    assert!(provider
        .handle_frame(
            WebAuthnRequestFrame::Cancel {
                request_id: "missing".into(),
            },
            None,
        )
        .is_empty());
    provider.reset_transport();
    assert_eq!(provider.provider_id(), None);

    let presence = parse_signer_event_line(&format!(
        "{SIGNER_EVENT_PREFIX}{{\"kind\":\"presence-required\"}}"
    ))
    .expect("signer event");
    assert_eq!(presence, SignerEvent::PresenceRequired);
    assert_eq!(
        describe_signer_event_as_status(&presence),
        Some("Touch your security key now")
    );
    let pin = parse_signer_event_line(&format!(
        "{SIGNER_EVENT_PREFIX}{{\"kind\":\"pin-invalid\",\"retries\":2}}"
    ))
    .expect("pin event");
    assert_eq!(pin, SignerEvent::PinInvalid { retries: Some(2) });
    assert_eq!(describe_signer_event_as_status(&pin), None);
}


#[test]
fn coordinator_inference_router_refreshes_default_provider_without_losing_agent_overrides() {
    use std::fs;

    use mahayana_node_agent_coordinator::inference_router::{
        CoordinatorInferenceRouter, InferenceRoute,
    };
    use uuid::Uuid;

    let root = std::env::temp_dir().join(format!(
        "fabushi-coordinator-inference-router-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("router temp dir");
    let settings = root.join("settings.json");
    fs::write(&settings, r#"{"inferenceProvider":"codex"}"#).expect("codex settings");

    let router = CoordinatorInferenceRouter::new(&settings);
    let default = router.resolve("builder");
    assert_eq!(default.provider, "codex");
    assert_eq!(default.host_slot, "host");

    router
        .bind_agent(
            "research",
            InferenceRoute {
                provider: "claude-code".into(),
                host_slot: "host".into(),
            },
        )
        .expect("bind agent route");

    fs::write(&settings, r#"{"router":{"provider":"openrouter"}}"#)
        .expect("openrouter settings");
    assert_eq!(router.resolve("builder").provider, "openrouter");
    assert_eq!(router.resolve("research").provider, "claude-code");

    router.unbind_agent("research").expect("unbind agent route");
    assert_eq!(router.resolve("research").provider, "openrouter");

    fs::remove_dir_all(root).expect("remove router temp dir");
}


#[test]
fn coordinator_inference_router_uses_product_fallback_until_settings_override_it() {
    use std::fs;

    use mahayana_node_agent_coordinator::inference_router::{
        CoordinatorInferenceRouter, InferenceProvider,
    };
    use uuid::Uuid;

    let root = std::env::temp_dir().join(format!(
        "fabushi-coordinator-product-inference-router-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("router temp dir");
    let settings = root.join("settings.json");

    let router =
        CoordinatorInferenceRouter::new_with_fallback(&settings, InferenceProvider::Codex);
    let product_default = router.resolve("builder");
    assert_eq!(product_default.provider, "codex");
    assert_eq!(product_default.host_slot, "host");

    fs::write(&settings, r#"{"inferenceProvider":"cursor"}"#).expect("cursor settings");
    assert_eq!(router.resolve("builder").provider, "cursor");

    fs::remove_dir_all(root).expect("remove router temp dir");
}


#[test]
fn provider_history_projects_host_send_messages_as_assistant_turns() {
    use mahayana_node_agent_coordinator::inference_router::provider_messages_from_host_transcript;

    let messages = provider_messages_from_host_transcript(&json!({
        "entries": [
            {"kind":"message","id":"t1u","role":"user","content":"first"},
            {"kind":"send-message","id":"t1s0","message":{"type":"text","content":"answer first"}},
            {"kind":"widget","id":"ignore"},
            {"kind":"message","id":"t2u","role":"user","content":"second"},
            {"kind":"send-message","id":"t2s0","message":{"type":"image","content":"ignore"}},
            {"kind":"send-message","id":"t2s1","message":{"type":"text","content":"answer second"}}
        ]
    }));

    assert_eq!(
        messages,
        vec![
            json!({"role":"user","content":"first"}),
            json!({"role":"assistant","content":"answer first"}),
            json!({"role":"user","content":"second"}),
            json!({"role":"assistant","content":"answer second"}),
        ]
    );
}


#[test]
fn shipping_local_inference_uses_host_canonical_history_for_provider_messages() {
    let main = include_str!("../src/main.rs");
    let history_projection = main
        .find("let mut messages = provider_messages_from_host_transcript(&remote);")
        .expect("shipping local inference projects Host transcript");
    let current_prompt = main[history_projection..]
        .find("\"role\": \"user\",")
        .map(|offset| history_projection + offset)
        .expect("current prompt follows canonical history projection");
    let runner_start = main[history_projection..]
        .find("\"runner.startRoutedProvider\"")
        .map(|offset| history_projection + offset)
        .expect("canonical history reaches shipping Runner");
    assert!(history_projection < current_prompt);
    assert!(current_prompt < runner_start);
}


#[test]
fn inference_router_persists_bounded_transcripts_reactions_and_turn_ids() {
    use std::fs;

    use mahayana_node_agent_coordinator::inference_router::{
        InferenceRoute, InferenceRouter, InferenceTranscriptFile, StoredEntry, StoredRole,
        TranscriptStore, INFERENCE_TRANSCRIPT_LIMIT, project_transcript_entry, turn_number_from_id,
    };
    use serde_json::json;
    use uuid::Uuid;

    let mut router = InferenceRouter::default();
    router.set_default(InferenceRoute {
        provider: "codex".into(),
        host_slot: "host".into(),
    });
    router.bind_agent(
        "research",
        InferenceRoute {
            provider: "claude-code".into(),
            host_slot: "host".into(),
        },
    );
    assert_eq!(router.resolve("research").map(|route| route.provider.as_str()), Some("claude-code"));
    assert_eq!(router.resolve("other").map(|route| route.provider.as_str()), Some("codex"));
    router.unbind_agent("research");
    assert_eq!(router.resolve("research").map(|route| route.provider.as_str()), Some("codex"));

    assert_eq!(turn_number_from_id("t17u"), Some(17));
    assert_eq!(turn_number_from_id("t17s0"), Some(17));
    assert_eq!(turn_number_from_id("request-17"), None);

    let rows = (0..(INFERENCE_TRANSCRIPT_LIMIT + 5))
        .map(|index| json!({
            "provider": "codex",
            "role": if index % 2 == 0 { "user" } else { "assistant" },
            "content": format!("entry-{index}"),
            "id": format!("t{}{}", index, if index % 2 == 0 { "u" } else { "s0" }),
            "timestampMs": index,
        }))
        .collect::<Vec<_>>();
    let mut store = TranscriptStore::parse(json!({
        "schemaVersion": 2,
        "agents": {
            "a": rows,
            "bad": [{
                "provider": "unknown",
                "role": "user",
                "content": "invalid",
                "id": "t1u",
                "timestampMs": 1,
            }]
        }
    }));
    assert_eq!(store.entries("a").len(), INFERENCE_TRANSCRIPT_LIMIT);
    assert_eq!(store.entries("a").first().map(|entry| entry.content.as_str()), Some("entry-5"));
    assert!(store.entries("bad").is_empty());
    assert_eq!(
        store.next_turn_number("a", vec!["t500s0".to_string(), "noise".to_string()]),
        501
    );

    let reaction_target = store.entries("a").last().expect("entry").id.clone();
    let reacted = store
        .toggle_local_reaction("a", &reaction_target, "👍")
        .expect("reaction");
    assert_eq!(reacted.reactions.len(), 1);
    let reacted = store
        .toggle_local_reaction("a", &reaction_target, "👍")
        .expect("reaction removed");
    assert!(reacted.reactions.is_empty());

    let user = StoredEntry {
        provider: "openrouter".into(),
        role: StoredRole::User,
        content: "hello".into(),
        rich_text: Some("{\"type\":\"doc\"}".into()),
        id: "t501u".into(),
        client_nonce: Some("nonce-501".into()),
        attachments: Vec::new(),
        reactions: Vec::new(),
        timestamp_ms: 501,
    };
    let projected = project_transcript_entry(&user);
    assert_eq!(projected["kind"], "message");
    assert_eq!(projected["clientNonce"], "nonce-501");

    let path = std::env::temp_dir().join(format!(
        "fabushi-inference-router-{}.json",
        Uuid::new_v4()
    ));
    let file = InferenceTranscriptFile::new(&path);
    let persisted = file.append("agent:file", [user]).expect("persist");
    assert_eq!(persisted.entries("agent:file").len(), 1);
    assert_eq!(file.load().entries("agent:file")[0].content, "hello");
    file.toggle_local_reaction("agent:file", "t501u", "✅")
        .expect("persist reaction");
    assert_eq!(
        file.load().entries("agent:file")[0].reactions[0].emoji,
        "✅"
    );
    let _ = fs::remove_file(path);
}


#[test]
fn gateway_dns_reporter_models_failure_episode_cooldown_and_recovery() {
    use std::cell::Cell;

    use mahayana_node_agent_coordinator::gateway::gateway_dns_diagnostics::{
        DnsDiagnosis, DnsProbeResult, GatewayDnsDiagnosticReporter,
        DNS_PROBE_MIN_INTERVAL_MS,
    };

    let system_calls = Cell::new(0_u32);
    let independent_calls = Cell::new(0_u32);
    let mut reporter = GatewayDnsDiagnosticReporter::default();

    let first = reporter
        .observe_with_probes(
            1_000,
            "dns",
            Some("lookup failed: ENOTFOUND"),
            Some("https://agent-7.us8.cursorvm.com/api"),
            "probe-episode",
            |_| {
                system_calls.set(system_calls.get() + 1);
                DnsProbeResult::NotFound
            },
            |hostname| {
                independent_calls.set(independent_calls.get() + 1);
                if hostname == "agent-7.us8.cursorvm.com" {
                    DnsProbeResult::Resolved
                } else {
                    DnsProbeResult::NotFound
                }
            },
        )
        .expect("first DNS failure starts a diagnostic episode");
    assert_eq!(first.trigger, DnsProbeResult::NotFound);
    assert_eq!(first.diagnosis, DnsDiagnosis::SystemPathFailure);
    assert_eq!(system_calls.get(), 1);
    assert_eq!(independent_calls.get(), 3);
    assert!(reporter.episode_active());
    assert!(!reporter.probe_in_flight());

    assert!(reporter
        .observe_with_probes(
            2_000,
            "dns",
            Some("EAI_AGAIN"),
            Some("https://agent-7.us8.cursorvm.com/api"),
            "probe-duplicate",
            |_| DnsProbeResult::Resolved,
            |_| DnsProbeResult::Resolved,
        )
        .is_none(), "duplicate failures in the same episode are suppressed");
    assert_eq!(system_calls.get(), 1);
    assert_eq!(independent_calls.get(), 3);

    assert!(reporter
        .observe_with_probes(
            3_000,
            "ok",
            None,
            Some("https://agent-7.us8.cursorvm.com/api"),
            "probe-reset",
            |_| DnsProbeResult::Resolved,
            |_| DnsProbeResult::Resolved,
        )
        .is_none());
    assert!(!reporter.episode_active());

    assert!(reporter
        .observe_with_probes(
            3_001,
            "dns",
            None,
            Some("https://agent-7.us8.cursorvm.com/api"),
            "probe-too-soon",
            |_| DnsProbeResult::Resolved,
            |_| DnsProbeResult::Resolved,
        )
        .is_none(), "recovered episodes still honor the global probe cooldown");
    assert!(!reporter.episode_active());

    let after_cooldown = reporter
        .observe_with_probes(
            1_000 + DNS_PROBE_MIN_INTERVAL_MS,
            "dns",
            Some("EAI_AGAIN"),
            Some("https://agent-7.us8.cursorvm.com/api"),
            "probe-after-cooldown",
            |_| DnsProbeResult::TemporaryFailure,
            |_| DnsProbeResult::TemporaryFailure,
        )
        .expect("a recovered episode can probe again after the cooldown");
    assert_eq!(after_cooldown.trigger, DnsProbeResult::TemporaryFailure);
    assert_eq!(after_cooldown.diagnosis, DnsDiagnosis::GeneralDnsFailure);
}


#[test]
fn oauth_loopback_listener_binds_dispatches_and_releases_real_http_callback() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use mahayana_node_agent_coordinator::oauth::mcp_oauth_callback_listener::{
        OAuthCallback, start_mcp_oauth_callback_listener,
    };
    use mahayana_node_agent_coordinator::oauth::mcp_oauth_loopback_registry::{
        McpOAuthLoopbackRegistry,
    };

    let reservation = TcpListener::bind("127.0.0.1:0").expect("reserve callback port");
    let port = reservation.local_addr().expect("local address").port();
    drop(reservation);
    let redirect_url = format!("http://127.0.0.1:{port}/oauth/callback");

    let callbacks = Arc::new(Mutex::new(Vec::<OAuthCallback>::new()));
    let settled = Arc::new(Mutex::new(Vec::<String>::new()));
    let callback_sink = Arc::clone(&callbacks);
    let settled_sink = Arc::clone(&settled);
    let mut registry = McpOAuthLoopbackRegistry::default();

    let listener = start_mcp_oauth_callback_listener(
        &redirect_url,
        &mut registry,
        |state| (state == "state-1").then(|| "github".to_string()),
        move |callback| {
            callback_sink.lock().expect("callback sink").push(callback);
            Ok(())
        },
        move |state| {
            settled_sink
                .lock()
                .expect("settled sink")
                .push(state.to_string());
        },
    )
    .expect("OAuth listener");
    assert_eq!(registry.active_origin_count(), 1);
    assert_eq!(registry.lease_count(&format!("http://127.0.0.1:{port}")), 1);
    assert_eq!(registry.bound_listener_count(&format!("http://127.0.0.1:{port}")), 1);

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect callback listener");
    write!(
        stream,
        "GET /oauth/callback?state=state-1&code=abc%2B123 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("write callback request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read callback response");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("Authorization complete"));

    let recorded = callbacks.lock().expect("callbacks");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].code, "abc+123");
    assert_eq!(recorded[0].state, "state-1");
    assert_eq!(recorded[0].server_name, "github");
    drop(recorded);
    assert_eq!(
        settled.lock().expect("settled").as_slice(),
        &["state-1".to_string()]
    );

    assert!(listener.close(&mut registry));
    assert_eq!(registry.active_origin_count(), 0);
    assert_eq!(registry.bound_listener_count(&format!("http://127.0.0.1:{port}")), 0);
}


#[test]
fn host_gateway_discovery_resolves_local_endpoint_and_bearer_header() {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use mahayana_node_agent_coordinator::gateway::host_supervisor::read_gateway_discovery;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "mahayana-gateway-discovery-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("create discovery root");
    let path = root.join("gateway.json");
    fs::write(
        &path,
        r#"{
          "port": 43123,
          "pid": 77,
          "startedAt": 1234,
          "scheme": "http",
          "host": "127.0.0.1",
          "token": "secret-token"
        }"#,
    )
    .expect("write discovery");

    let connection = read_gateway_discovery(&path).expect("read discovery");
    assert_eq!(connection.base_url, "http://127.0.0.1:43123");
    assert_eq!(
        connection.headers.get("authorization").map(String::as_str),
        Some("Bearer secret-token")
    );

    fs::remove_dir_all(root).expect("remove discovery root");
}

#[test]
fn gateway_request_dispatcher_posts_real_http_json_and_classifies_errors() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::{
        GatewayDispatchError, dispatch_http_json,
    };
    use mahayana_node_agent_coordinator::gateway::gateway_reachability::ReachabilityOutcome;
    use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn serve_once(listener: TcpListener, status: u16, response_body: &'static str) -> thread::JoinHandle<String> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept gateway request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("request line");
            let mut headers = Vec::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("request header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                {
                    content_length = value.parse().expect("content length");
                }
                headers.push(line);
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).expect("request body");
            let reason = if status == 200 { "OK" } else { "Internal Server Error" };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .expect("gateway response");
            stream.flush().expect("gateway flush");
            format!(
                "{request_line}{}{}",
                headers.concat(),
                String::from_utf8_lossy(&body)
            )
        })
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway");
    let port = listener.local_addr().expect("gateway addr").port();
    let observed = serve_once(listener, 200, r#"{"echoed":true}"#);
    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer secret".into());
    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers,
    };
    let value = dispatch_http_json(&connection, "sendPrompt", json!({"prompt":"hello"}))
        .expect("HTTP gateway dispatch");
    assert_eq!(value, json!({"echoed":true}));
    let request = observed.join().expect("gateway observer");
    assert!(request.starts_with("POST /api/sendPrompt HTTP/1.1\r\n"), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer secret\r\n"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-sand-slim-avatars: 1\r\n"),
        "{request}"
    );
    assert!(request.ends_with(r#"{"prompt":"hello"}"#), "{request}");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing gateway");
    let port = listener.local_addr().expect("gateway addr").port();
    let observed = serve_once(listener, 500, r#"{"error":"host exploded"}"#);
    let error = dispatch_http_json(
        &GatewayConnection {
            base_url: format!("http://127.0.0.1:{port}"),
            headers: BTreeMap::new(),
        },
        "sendPrompt",
        json!({}),
    )
    .expect_err("HTTP 500 is unreachable");
    observed.join().expect("failing gateway observer");
    assert!(matches!(
        error,
        GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Http(500),
            ..
        }
    ));
}


#[test]
fn gateway_client_streams_real_sse_and_honors_bearer_auth() {
    use std::cell::{Cell, RefCell};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    use mahayana_node_agent_coordinator::gateway::gateway_client::stream_http_events;
    use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
    use serde_json::json;
    use std::collections::BTreeMap;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind SSE gateway");
    let port = listener.local_addr().expect("SSE address").port();
    let observed = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept SSE request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone SSE stream"));
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("SSE request line");
            if line.is_empty() || line == "\r\n" {
                break;
            }
            request.push_str(&line);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\nretry: 1000\n\ndata: {{\"channel\":\"runtime\",\"payload\":{{\"value\":9}}}}\n\n"
        )
        .expect("write SSE response");
        stream.flush().expect("flush SSE response");
        request
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer stream-secret".into());
    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers,
    };
    let connected = Cell::new(false);
    let keep_streaming = Cell::new(true);
    let events = RefCell::new(Vec::new());
    stream_http_events(
        &connection,
        || connected.set(true),
        |event| {
            events.borrow_mut().push(event);
            keep_streaming.set(false);
        },
        || keep_streaming.get(),
    )
    .expect("SSE stream");
    assert!(connected.get());
    assert_eq!(
        events.borrow().as_slice(),
        &[json!({"channel":"runtime","payload":{"value":9}})]
    );
    let request = observed.join().expect("SSE observer");
    assert!(request.starts_with("GET /events HTTP/1.1\r\n"), "{request}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer stream-secret\r\n"),
        "{request}"
    );
    assert!(
        request.to_ascii_lowercase().contains("accept: text/event-stream\r\n"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("x-sand-slim-avatars: 1\r\n"),
        "{request}"
    );
}


#[test]
fn gateway_client_pause_and_dev_offline_gate_dispatch_without_closing_client() {
    use mahayana_node_agent_coordinator::gateway::gateway_client::CoordinatorGatewayClient;
    use mahayana_node_agent_coordinator::gateway::gateway_reachability::ReachabilityOutcome;
    use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::GatewayDispatchError;
    use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
    use std::collections::BTreeMap;

    let mut client = CoordinatorGatewayClient::default();
    client
        .install_connection(GatewayConnection {
            base_url: "http://127.0.0.1:43123".into(),
            headers: BTreeMap::new(),
        })
        .unwrap();
    client.start(1).unwrap();

    assert!(client.set_client_paused(true));
    assert!(client.is_transport_suppressed());
    assert!(matches!(
        client.connection_for_dispatch(),
        Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::BoxBlocked,
            ..
        })
    ));

    assert!(!client.set_client_paused(false));
    client.start(2).unwrap();
    assert!(client.set_dev_induced_offline(true));
    assert!(matches!(
        client.connection_for_dispatch(),
        Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Network,
            ..
        })
    ));

    assert!(!client.set_dev_induced_offline(false));
    client.start(3).unwrap();
    assert!(client.connection_for_dispatch().is_ok());
}


#[test]
fn inference_router_serializes_each_agent_but_allows_parallel_agents() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use mahayana_node_agent_coordinator::inference_router::InferenceTaskQueue;

    let queue = Arc::new(InferenceTaskQueue::default());
    let (same_tx, same_rx) = mpsc::channel::<&'static str>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    queue
        .enqueue("agent-a", {
            let same_tx = same_tx.clone();
            move || {
                same_tx.send("first-start").expect("first start");
                release_rx.recv().expect("release first");
                same_tx.send("first-end").expect("first end");
            }
        })
        .expect("first enqueue");
    assert_eq!(
        same_rx.recv_timeout(Duration::from_secs(1)).expect("first starts"),
        "first-start"
    );

    queue
        .enqueue("agent-a", {
            let same_tx = same_tx.clone();
            move || {
                same_tx.send("second").expect("second");
            }
        })
        .expect("second enqueue");
    assert!(
        same_rx.recv_timeout(Duration::from_millis(75)).is_err(),
        "same-agent second task must wait for the first task"
    );

    queue
        .enqueue_urgent("agent-a", {
            let same_tx = same_tx.clone();
            move || {
                same_tx.send("urgent").expect("urgent");
            }
        })
        .expect("urgent enqueue");
    assert!(
        same_rx.recv_timeout(Duration::from_millis(75)).is_err(),
        "same-agent urgent task must not overlap the active task"
    );

    let (other_tx, other_rx) = mpsc::channel::<&'static str>();
    queue
        .enqueue("agent-b", move || {
            other_tx.send("parallel").expect("other agent");
        })
        .expect("parallel enqueue");
    assert_eq!(
        other_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("different agent runs"),
        "parallel"
    );

    release_tx.send(()).expect("release first");
    assert_eq!(
        same_rx.recv_timeout(Duration::from_secs(1)).expect("first ends"),
        "first-end"
    );
    assert_eq!(
        same_rx.recv_timeout(Duration::from_secs(1)).expect("urgent runs"),
        "urgent"
    );
    assert_eq!(
        same_rx.recv_timeout(Duration::from_secs(1)).expect("second runs"),
        "second"
    );

    queue
        .enqueue("agent-a", || panic!("simulated provider failure"))
        .expect("panic task enqueue");
    queue
        .enqueue("agent-a", {
            let same_tx = same_tx.clone();
            move || {
                same_tx.send("after-failure").expect("after failure");
            }
        })
        .expect("post-failure enqueue");
    assert_eq!(
        same_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("queue survives provider failure"),
        "after-failure"
    );

    assert_eq!(queue.worker_count(), 2);
    queue.dispose();
    assert_eq!(queue.worker_count(), 0);
}

#[test]
fn agent_inbound_direct_wake_forwards_selected_images_without_group_fanout() {
    let selected = json!([{
        "data": [1, 2, 3],
        "path": "/tmp/peer.png",
        "mimeType": "image/png"
    }]);
    let routes = prepare_agent_inbound_wake_routes(&json!({
        "agentId": "agent-target",
        "sourceAgentId": "agent-source",
        "prompt": "[agent] peer message",
        "priority": true,
        "selectedImages": selected,
        "inbound": {
            "from": {"id":"agent-source","name":"Source"},
            "text":"peer message",
            "timestampMs":123,
            "replyContext":{"targetId":"outbound-1","quote":"peer message"}
        }
    }))
    .expect("direct peer wake");

    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.agent_id, "agent-target");
    assert_eq!(route.send_args["requestSource"], "agent-inbound");
    assert_eq!(route.send_args["hidden"], true);
    assert_eq!(route.send_args["isSilenceAllowed"], true);
    assert_eq!(route.send_args["appendUserMessage"], false);
    assert_eq!(route.send_args["selectedImages"], selected);
    assert_eq!(route.send_args["agentWake"]["priority"], true);
    assert_eq!(route.send_args["agentWake"]["inbound"]["text"], "peer message");
    assert_eq!(route.send_args["replyContext"]["targetId"], "outbound-1");
    assert_eq!(route.send_args["replyContext"]["quote"], "peer message");
    assert!(route.send_args.get("groupContext").is_none());

    assert!(prepare_agent_inbound_wake_routes(&json!({
        "agentId": "agent-target",
        "sourceAgentId": "agent-source",
        "prompt": "[agent] peer message",
        "selectedImages": "not-an-array"
    }))
    .is_err());
    assert!(prepare_agent_inbound_wake_routes(&json!({
        "agentId": "agent-target",
        "sourceAgentId": "agent-source",
        "prompt": "[agent] peer message",
        "inbound": {
            "replyContext": {"targetId": "outbound-1", "quote": " "}
        }
    }))
    .is_err());
}


#[test]
fn deleted_agent_purge_skips_queued_inference_and_preserves_other_agents() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use mahayana_node_agent_coordinator::inference_router::InferenceTaskQueue;

    assert_eq!(
        deleted_agent_ids_for_host_success("deleteAgent", &json!({"id":"agent-a"})),
        vec!["agent-a".to_string()]
    );
    assert_eq!(
        deleted_agent_ids_for_host_success(
            "deleteAgents",
            &json!({"ids":["agent-a","agent-b"]}),
        ),
        vec!["agent-a".to_string(), "agent-b".to_string()]
    );
    assert!(
        deleted_agent_ids_for_host_success("updateAgent", &json!({"id":"agent-a"})).is_empty()
    );

    let queue = Arc::new(InferenceTaskQueue::default());
    let (agent_tx, agent_rx) = mpsc::channel::<&'static str>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    queue
        .enqueue("agent-a", {
            let agent_tx = agent_tx.clone();
            move || {
                agent_tx.send("active-start").expect("active start");
                release_rx.recv().expect("release active");
                agent_tx.send("active-end").expect("active end");
            }
        })
        .expect("active enqueue");
    assert_eq!(
        agent_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("active task starts"),
        "active-start"
    );

    queue
        .enqueue("agent-a", {
            let agent_tx = agent_tx.clone();
            move || {
                agent_tx.send("normal-queued").expect("normal queued");
            }
        })
        .expect("normal enqueue");
    queue
        .enqueue_urgent("agent-a", {
            let agent_tx = agent_tx.clone();
            move || {
                agent_tx.send("urgent-queued").expect("urgent queued");
            }
        })
        .expect("urgent enqueue");

    let (other_tx, other_rx) = mpsc::channel::<&'static str>();
    queue
        .enqueue("agent-b", move || {
            other_tx.send("other-agent").expect("other agent");
        })
        .expect("other enqueue");
    assert_eq!(
        other_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("other agent remains live"),
        "other-agent"
    );

    assert_eq!(queue.clear_agent("agent-a"), 1);
    release_tx.send(()).expect("release active");
    assert_eq!(
        agent_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("active task may finish"),
        "active-end"
    );
    assert!(
        agent_rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "normal and urgent queued work for the deleted agent must be invalidated"
    );

    queue
        .enqueue("agent-a", {
            let agent_tx = agent_tx.clone();
            move || {
                agent_tx.send("recreated-agent").expect("recreated agent");
            }
        })
        .expect("recreated enqueue");
    assert_eq!(
        agent_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("fresh generation runs"),
        "recreated-agent"
    );

    queue.dispose();
}

#[test]
fn coordinator_shipping_delete_success_wires_agent_queue_purge() {
    let main = include_str!("../src/main.rs");
    assert!(main.contains("deleted_agent_ids_for_host_success(&method, &args)"));
    assert!(main.contains("dispatch_state.inference_queue.clear_agent(agent_id)"));
}


#[test]
fn priority_peer_cancellation_redrives_agent_wake_exactly_once() {
    let args = json!({
        "agentId": "agent-target",
        "prompt": "[agent] older peer message",
        "appendUserMessage": false,
        "hidden": true,
        "requestSource": "agent-inbound",
        "agentWake": {
            "sourceAgentId": "agent-source",
            "priority": false,
            "isDisplayed": true
        }
    });
    let redrive = redrive_agent_inbound_after_priority_preemption(
        &args,
        PRIORITY_AGENT_WAKE_SUPERSEDE_REASON,
    )
    .expect("priority peer cancellation redrives");
    assert_eq!(redrive["agentWake"]["isRedriven"], true);
    assert_eq!(redrive["agentWake"]["isDisplayed"], true);
    assert_eq!(redrive["appendUserMessage"], false);

    assert!(
        redrive_agent_inbound_after_priority_preemption(
            &redrive,
            PRIORITY_AGENT_WAKE_SUPERSEDE_REASON,
        )
        .is_none(),
        "redrive marker prevents loops"
    );
    assert!(
        redrive_agent_inbound_after_priority_preemption(
            &args,
            "superseded by a newer user message",
        )
        .is_none(),
        "ordinary user supersede must not redrive a peer wake"
    );
    assert!(
        redrive_agent_inbound_after_priority_preemption(
            &json!({"requestSource":"turn","agentWake":{"priority":false}}),
            PRIORITY_AGENT_WAKE_SUPERSEDE_REASON,
        )
        .is_none(),
        "non-agent turns never enter peer redrive"
    );
}


#[test]
fn redriven_peer_wake_prefers_urgent_queue_without_becoming_priority() {
    let ordinary = json!({
        "requestSource": "agent-inbound",
        "agentWake": {"priority": false}
    });
    assert!(!agent_inbound_wake_prefers_urgent_queue(&ordinary));

    let priority = json!({
        "requestSource": "agent-inbound",
        "agentWake": {"priority": true}
    });
    assert!(agent_inbound_wake_prefers_urgent_queue(&priority));

    let redriven = redrive_agent_inbound_after_priority_preemption(
        &json!({
            "requestSource": "agent-inbound",
            "agentWake": {"priority": false, "isDisplayed": true}
        }),
        PRIORITY_AGENT_WAKE_SUPERSEDE_REASON,
    )
    .expect("redrive");
    assert_eq!(redriven["agentWake"]["priority"], false);
    assert_eq!(redriven["agentWake"]["isRedriven"], true);
    assert!(agent_inbound_wake_prefers_urgent_queue(&redriven));

    assert!(!agent_inbound_wake_prefers_urgent_queue(&json!({
        "requestSource": "turn",
        "agentWake": {"priority": false, "isRedriven": true}
    })));
}

#[test]
fn redriven_peer_wake_runs_after_new_priority_but_before_later_normal_work() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use mahayana_node_agent_coordinator::inference_router::InferenceTaskQueue;

    let queue = Arc::new(InferenceTaskQueue::default());
    let (events_tx, events_rx) = mpsc::channel::<&'static str>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    queue.enqueue("agent-a", {
        let events_tx = events_tx.clone();
        move || {
            events_tx.send("active").expect("active");
            release_rx.recv().expect("release");
        }
    }).expect("active enqueue");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(1)).expect("active starts"), "active");

    queue.enqueue("agent-a", {
        let events_tx = events_tx.clone();
        move || events_tx.send("later-normal").expect("later normal")
    }).expect("later normal enqueue");

    let priority = json!({
        "requestSource": "agent-inbound",
        "agentWake": {"priority": true}
    });
    assert!(agent_inbound_wake_prefers_urgent_queue(&priority));
    queue.enqueue_urgent("agent-a", {
        let events_tx = events_tx.clone();
        move || events_tx.send("new-priority").expect("priority")
    }).expect("priority enqueue");

    let redriven = json!({
        "requestSource": "agent-inbound",
        "agentWake": {"priority": false, "isDisplayed": true, "isRedriven": true}
    });
    assert!(agent_inbound_wake_prefers_urgent_queue(&redriven));
    queue.enqueue_urgent("agent-a", {
        let events_tx = events_tx.clone();
        move || events_tx.send("redriven-old").expect("redrive")
    }).expect("redrive enqueue");

    release_tx.send(()).expect("release active");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(1)).expect("priority runs"), "new-priority");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(1)).expect("redrive runs"), "redriven-old");
    assert_eq!(events_rx.recv_timeout(Duration::from_secs(1)).expect("normal runs"), "later-normal");

    queue.dispose();
}


#[test]
fn shipping_agent_inbound_execution_admission_is_host_owned_and_precedes_provider_dispatch() {
    let main = include_str!("../src/main.rs");
    let admission = main
        .find("accept_agent_inbound_message_for_execution(&worker_state, &agent_id, &mut args)")
        .expect("agent inbound execution admission");
    let cursor = main[admission..]
        .find("if provider == InferenceProvider::Cursor")
        .map(|offset| admission + offset)
        .expect("Cursor provider branch");
    assert!(admission < cursor);
    assert!(main.contains("\"acceptAgentInboundMessage\""));
    assert!(main.contains("wake.insert(\"isDisplayed\".into(), Value::Bool(true))"));
}

#[test]
fn shipping_cursor_agent_inbound_failure_redrives_priority_supersede_before_reporting_error() {
    let main = include_str!("../src/main.rs");
    let cursor_start = main
        .find("if provider == InferenceProvider::Cursor")
        .expect("shipping Cursor agent-inbound branch");
    let local_start = main[cursor_start..]
        .find("if let Err(error) =\n            execute_local_inference")
        .map(|offset| cursor_start + offset)
        .expect("shipping local inference branch boundary");
    let cursor_branch = &main[cursor_start..local_start];
    assert!(cursor_branch.contains(
        "redrive_agent_inbound_after_priority_preemption(&args, &error.message)"
    ));
    assert!(cursor_branch.contains("enqueue_agent_inbound_wake("));
    assert!(cursor_branch.contains("report_agent_inbound_failure_best_effort"));
}

#[test]
fn agent_inbound_failure_gateway_report_preserves_host_owned_error_context() {
    let args = agent_inbound_failure_gateway_args(
        "agent-target",
        "INFERENCE_PROVIDER_FAILED",
        "provider exploded",
    );
    assert_eq!(args["agentId"], "agent-target");
    assert_eq!(args["errorCode"], "INFERENCE_PROVIDER_FAILED");
    assert_eq!(args["message"], "provider exploded");
}

#[test]
fn tdrp_folders_filters_lifecycle_and_matching() {
    use mahayana_node_agent_coordinator::tdrp::folders_filters::{FolderFilterRule, FolderFilterState};

    let mut state = FolderFilterState::new();
    let folder = state
        .create_folder(
            "Work".to_string(),
            Some("💼".to_string()),
            FolderFilterRule {
                include_groups: true,
                include_channels: true,
                exclude_muted: true,
                ..Default::default()
            },
            vec!["chat_vip".to_string()],
            vec!["chat_spam".to_string()],
        )
        .expect("create folder");

    assert_eq!(folder.title, "Work");
    assert_eq!(state.list_folders().len(), 1);

    // Matching tests
    assert!(state.matches_chat(&folder.id, "chat_vip", false, false, false, false, true, false, false));
    assert!(!state.matches_chat(&folder.id, "chat_spam", false, true, false, false, false, false, false));
    assert!(state.matches_chat(&folder.id, "group_1", false, true, false, false, false, false, false));
    assert!(!state.matches_chat(&folder.id, "group_2", false, true, false, false, true, false, false));

    // Pin chat
    let pinned = state.pin_chat(&folder.id, "group_pinned".to_string(), true).expect("pin chat");
    assert!(pinned.pinned_chat_ids.contains(&"group_pinned".to_string()));

    // Delete folder
    assert!(state.delete_folder(&folder.id).expect("delete folder"));
    assert_eq!(state.list_folders().len(), 0);
}

#[test]
fn tdrp_forum_topics_lifecycle_pin_close_hide() {
    use mahayana_node_agent_coordinator::tdrp::forum_topics::ForumTopicsState;

    let mut state = ForumTopicsState::new();
    let topic = state
        .create_topic("community_1".to_string(), "General Discussions".to_string(), Some("💬".to_string()), 0x6FB9F0, 1000)
        .expect("create topic");

    assert_eq!(topic.topic_id, 1);
    assert!(!topic.is_pinned);

    let pinned = state.toggle_pin("community_1", topic.topic_id, true).expect("pin topic");
    assert!(pinned.is_pinned);

    let closed = state.toggle_close("community_1", topic.topic_id, true).expect("close topic");
    assert!(closed.is_closed);

    let hidden = state.toggle_hide("community_1", topic.topic_id, true).expect("hide topic");
    assert!(hidden.is_hidden);

    let unread = state.update_unread("community_1", topic.topic_id, 5, Some(999)).expect("update unread");
    assert_eq!(unread.unread_count, 5);
    assert_eq!(unread.top_message_id, Some(999));

    assert!(state.delete_topic("community_1", topic.topic_id).expect("delete topic"));
    assert!(state.list_topics("community_1").is_empty());
}

#[test]
fn tdrp_polls_and_todo_checklists_lifecycle() {
    use mahayana_node_agent_coordinator::tdrp::polls_todo::{PollKind, PollsTodoState};

    let mut state = PollsTodoState::new();
    let poll = state
        .create_poll(
            "What is your favorite stack?".to_string(),
            vec!["Rust".to_string(), "TypeScript".to_string(), "Python".to_string()],
            PollKind::Regular { multiple_answers: false },
            1000,
        )
        .expect("create poll");

    let voted = state.vote_poll(&poll.poll_id, "user_1".to_string(), vec![0]).expect("vote");
    assert_eq!(voted.total_voters, 1);
    assert_eq!(voted.options[0].voter_count, 1);

    let retracted = state.retract_vote(&poll.poll_id, "user_1").expect("retract");
    assert_eq!(retracted.total_voters, 0);
    assert_eq!(retracted.options[0].voter_count, 0);

    // Todo lists
    let list = state.create_todo_list("conv_1".to_string(), "Sprint Checklist".to_string()).expect("create list");
    let item = state.add_todo_item(&list.list_id, "Decompose engine".to_string(), Some("alice".to_string()), None, 1000).expect("add item");
    assert!(!item.completed);

    let toggled = state.toggle_todo_item(&list.list_id, &item.item_id, true).expect("toggle item");
    assert!(toggled.completed);

    assert!(state.delete_todo_item(&list.list_id, &item.item_id).expect("delete item"));
}

#[test]
fn tdrp_stories_gifts_and_stars_balance_transfer() {
    use mahayana_node_agent_coordinator::tdrp::stories_gifts_stars::{StoriesGiftsStarsState, StoryPrivacy};

    let mut state = StoriesGiftsStarsState::new();
    let story = state
        .post_story("user_alice".to_string(), "https://cdn.example.com/story.mp4".to_string(), Some("Sunset!".to_string()), StoryPrivacy::Public, 86400, 1000)
        .expect("post story");

    let viewed = state.view_story(&story.story_id, "user_bob".to_string(), 1050).expect("view story");
    assert_eq!(viewed.views_count, 1);

    let reacted = state.react_story(&story.story_id, "user_bob".to_string(), "🔥".to_string()).expect("react story");
    assert_eq!(reacted.reactions.get("user_bob").map(String::as_str), Some("🔥"));

    // Stars balance & transfer
    state.purchase_stars("user_alice", 500, "Purchase 500 stars".to_string(), 1000);
    assert_eq!(state.get_star_balance("user_alice"), 500);

    let (alice_bal, bob_bal) = state.transfer_stars("user_alice", "user_bob", 200, "For tips".to_string(), 1010).expect("transfer");
    assert_eq!(alice_bal, 300);
    assert_eq!(bob_bal, 200);

    // Send Gift
    let gift = state.send_gift("user_alice", "user_bob", "Star Trophy".to_string(), 50, Some("Great job!".to_string()), false, 1020).expect("send gift");
    assert_eq!(gift.stars_price, 50);
    assert_eq!(state.get_star_balance("user_alice"), 250);
    assert_eq!(state.list_received_gifts("user_bob").len(), 1);
}

#[test]
fn tdrp_chat_admin_moderation_rights_ban_restrictions_audit() {
    use mahayana_node_agent_coordinator::tdrp::chat_admin_moderation::{ChatAdminModerationState, ChatAdminRights, RestrictedPermissions};

    let mut state = ChatAdminModerationState::new();
    let rights = state.set_admin_rights(
        "supergroup_1",
        "owner_id",
        "admin_bob",
        ChatAdminRights {
            delete_messages: true,
            ban_users: true,
            manage_topics: true,
            ..Default::default()
        },
        1000,
    ).expect("set rights");
    assert!(rights.ban_users);

    // Ban member
    let banned = state.ban_member("supergroup_1", "admin_bob", "bad_actor", Some(3600), Some("Spamming".to_string()), 1000).expect("ban member");
    assert_eq!(banned.user_id, "bad_actor");
    assert!(state.is_banned("supergroup_1", "bad_actor", 1010));

    // Restrict member
    let restricted = state.restrict_member(
        "supergroup_1",
        "admin_bob",
        "spammer_2",
        RestrictedPermissions {
            send_messages: true,
            send_media: false,
            ..Default::default()
        },
        Some(1800),
        1000,
    ).expect("restrict member");
    assert!(!restricted.send_media);

    // Unban
    assert!(state.unban_member("supergroup_1", "admin_bob", "bad_actor", 1020).expect("unban"));
    assert!(!state.is_banned("supergroup_1", "bad_actor", 1030));

    // Audit log
    let log = state.get_audit_log("supergroup_1", 10);
    assert!(log.len() >= 3);
}

#[test]
fn tdrp_calls_and_group_calls_lifecycle_signaling() {
    use mahayana_node_agent_coordinator::tdrp::calls_group_calls::{CallState, CallsGroupCallsState};

    let mut state = CallsGroupCallsState::new();
    let call = state.create_call("alice".to_string(), "bob".to_string(), true, 1000).expect("create call");
    assert_eq!(call.state, CallState::Dialing);

    let accepted = state.accept_call(&call.call_id, "bob", 1010).expect("accept call");
    assert!(matches!(accepted.state, CallState::Active { .. }));

    assert!(state.send_signaling(&call.call_id, "{\"sdp\":\"offer\"}".to_string()).expect("signaling"));

    let ended = state.end_call(&call.call_id, "user_hangup".to_string(), 1050).expect("end call");
    assert!(matches!(ended.state, CallState::Ended { duration_ms: 40, .. }));

    // Group call
    let gc = state.create_group_call("chat_general".to_string(), "Daily Standup".to_string(), 1000).expect("create gc");
    let p = state.join_group_call(&gc.group_call_id, "alice".to_string(), 1001, 1005).expect("join gc");
    assert!(p.is_muted);

    let unmuted = state.toggle_mute_group_call(&gc.group_call_id, "alice", false).expect("unmute");
    assert!(!unmuted.is_muted);

    let hand = state.raise_hand_group_call(&gc.group_call_id, "alice", true).expect("raise hand");
    assert!(hand.is_hand_raised);

    assert!(state.leave_group_call(&gc.group_call_id, "alice").expect("leave gc"));
}

#[test]
fn tdrp_settings_cloud_password_and_passkeys_auth() {
    use mahayana_node_agent_coordinator::tdrp::settings_cloud_passkeys::{PrivacyRuleValue, PrivacySettings, SettingsCloudPasskeysState};

    let mut state = SettingsCloudPasskeysState::new();
    let mut priv_set = PrivacySettings::default();
    priv_set.phone_number = PrivacyRuleValue::ContactsOnly;
    priv_set.last_seen = PrivacyRuleValue::DisallowAll;
    state.update_privacy("user_alice", priv_set);

    let fetched = state.get_privacy("user_alice");
    assert_eq!(fetched.phone_number, PrivacyRuleValue::ContactsOnly);
    assert_eq!(fetched.last_seen, PrivacyRuleValue::DisallowAll);

    // 2FA Cloud Password
    state.set_cloud_password("user_alice", "hash123".to_string(), "salt456".to_string(), Some("hint".to_string()), None).expect("set 2fa");
    assert!(state.verify_cloud_password("user_alice", "hash123").expect("verify 2fa"));
    assert!(state.verify_cloud_password("user_alice", "wrong_hash").is_err());

    // Passkeys
    let chal = state.create_auth_challenge("user_alice", 1000);
    let reg = state.register_passkey("user_alice", "cred_1".to_string(), "pubkey_base64".to_string(), &chal, 1010).expect("register passkey");
    assert_eq!(reg.credential_id, "cred_1");

    let chal_login = state.create_auth_challenge("user_alice", 1020);
    assert!(state.verify_passkey("user_alice", "cred_1", &chal_login, "sig_valid").expect("verify passkey"));
}

#[test]
fn tdrp_export_payments_and_inline_bots_lifecycle() {
    use mahayana_node_agent_coordinator::tdrp::export_payments_bots::{ExportFormat, ExportPaymentsBotsState, ExportStatus, InlineQueryResult};

    let mut state = ExportPaymentsBotsState::new();
    let job = state.start_chat_export("user_alice".to_string(), "chat_123".to_string(), ExportFormat::Json, 1000).expect("start export");
    assert!(matches!(job.status, ExportStatus::Queued));

    let updated = state.update_export_progress(&job.job_id, ExportStatus::Completed { download_url: "https://dl.example.com/export.json".to_string(), total_bytes: 4096 }).expect("update progress");
    assert!(matches!(updated.status, ExportStatus::Completed { .. }));

    // Payments
    let invoice = state.create_invoice("Premium Subscription".to_string(), "1 Year access".to_string(), "USD".to_string(), 4999, 1000).expect("create invoice");
    assert_eq!(invoice.total_amount_cents, 4999);
    assert!(!invoice.is_paid);

    let settled = state.settle_payment(&invoice.invoice_id, "charge_stripe_123".to_string()).expect("settle payment");
    assert!(settled.is_paid);
    assert_eq!(settled.provider_payment_charge_id.as_deref(), Some("charge_stripe_123"));

    // Inline Bots
    state.register_inline_results("bot_calc", vec![
        InlineQueryResult {
            id: "res_1".to_string(),
            title: "Calculate 2+2".to_string(),
            description: Some("Result: 4".to_string()),
            result_type: "article".to_string(),
            send_message_text: "4".to_string(),
        }
    ]);
    let query_res = state.query_inline_bot("bot_calc", "2+2");
    assert_eq!(query_res.len(), 1);
    assert_eq!(query_res[0].title, "Calculate 2+2");
}

#[test]
fn tdrp_coordinator_route_dispatch_all_slices() {
    use mahayana_node_agent_coordinator::tdrp::TdrpCoordinatorState;

    let mut state = TdrpCoordinatorState::new();

    // 1. Folders route
    let res = state.dispatch("folders.create", &json!({
        "title": "Crypto",
        "filterRule": { "include_channels": true }
    })).expect("handled").expect("ok");
    assert!(res["folder"]["id"].as_str().is_some());

    // 2. Topics route (with feature. prefix)
    let res = state.dispatch("feature.topics.create", &json!({
        "communityId": "comm_alpha",
        "title": "Tech Talk"
    })).expect("handled").expect("ok");
    assert_eq!(res["topic"]["title"], "Tech Talk");

    // 3. Polls route
    let res = state.dispatch("polls.create", &json!({
        "question": "Release today?",
        "options": ["Yes", "Tomorrow"]
    })).expect("handled").expect("ok");
    assert_eq!(res["poll"]["question"], "Release today?");

    // 4. Stories & Stars route
    let res = state.dispatch("stars.purchase", &json!({
        "userId": "user_dev",
        "amount": 100
    })).expect("handled").expect("ok");
    assert_eq!(res["balance"], 100);

    // 5. Admin Moderation route
    let res = state.dispatch("admin.setRights", &json!({
        "chatId": "chat_mod",
        "actorId": "admin_1",
        "targetId": "mod_1",
        "rights": { "delete_messages": true }
    })).expect("handled").expect("ok");
    assert!(res["rights"]["delete_messages"].as_bool().unwrap());

    // 6. Calls route
    let res = state.dispatch("calls.create", &json!({
        "callerId": "alice",
        "calleeId": "bob",
        "isVideo": true
    })).expect("handled").expect("ok");
    assert!(res["call"]["id"].as_str().is_some() || res["call"]["call_id"].as_str().is_some());

    // 7. Settings route
    let res = state.dispatch("settings.getPrivacy", &json!({
        "userId": "alice"
    })).expect("handled").expect("ok");
    assert!(res["privacy"].is_object());

    // 8. Export route
    let res = state.dispatch("export.startChatExport", &json!({
        "userId": "alice",
        "chatId": "chat_history",
        "format": "json"
    })).expect("handled").expect("ok");
    assert!(res["exportJob"]["job_id"].as_str().is_some());

    // Unhandled route returns None
    assert!(state.dispatch("unknown.method", &json!({})).is_none());
}
