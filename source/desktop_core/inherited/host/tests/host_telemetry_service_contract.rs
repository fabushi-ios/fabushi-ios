use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use std::sync::Arc;

use mahayana_host_runtime::extensions::telemetry::analytics_service::TelemetryService;
use mahayana_host_runtime::extensions::telemetry::desktop_health_forwarder::DesktopHealthForwardResult;
use mahayana_host_runtime::extensions::telemetry::extension::{
    DESKTOP_HEALTH_EVENT, DESKTOP_HEALTH_HEARTBEAT_MS, DesktopHealthForwardState,
    TELEMETRY_EXTENSION_ID, forward_desktop_health_file_to_logs,
};
use mahayana_host_runtime::extensions::telemetry::disk_pressure_telemetry::DiskPressureReport;
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::extensions::telemetry::host_lifecycle_progress::{
    Disposable, HostLifecycleCompletion, WatchdogArm,
};
use mahayana_host_runtime::extensions::telemetry::auto_review_approval_telemetry::AutoReviewApprovalReport;
use mahayana_host_runtime::extensions::telemetry::journal_outcome_telemetry::JournalOutcomeReport;
use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;
use mahayana_host_runtime::extensions::telemetry::automation_fire_telemetry::AutomationFireDroppedReport;
use mahayana_host_runtime::extensions::telemetry::local_exec_telemetry::{
    LocalExecFailedReport, LocalExecProviderReport, LocalExecRefusalCause, LocalExecRefusedReport,
};
use mahayana_host_runtime::extensions::telemetry::memory_synthesis_telemetry::MemorySynthesisReport;
use mahayana_host_runtime::extensions::telemetry::revival_telemetry_mappers::{
    ShellRevivalReport, SubagentRevivalReport,
};
use mahayana_host_runtime::extensions::telemetry::turn_empty_delivery_telemetry::TurnEmptyDeliveryReport;
use mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::PendingWakeReport;
use mahayana_host_runtime::extensions::telemetry::webauthn_proxy_telemetry::{
    WebAuthnFailureCause, WebAuthnProxyReport,
};
use mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::{
    QueueAcceptedReport, queue_accepted_telemetry,
};
use serde_json::json;

static TEMP_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = TEMP_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabushi-host-telemetry-{}-{suffix}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn host_telemetry_service_owns_box_help_structured_log_and_product_analytics_ingress() {
    assert_eq!(TELEMETRY_EXTENSION_ID, "telemetry");
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("host-events.jsonl"))
        .expect("telemetry service");
    service.start().expect("telemetry start");

    service
        .logs
        .report_box_help(&json!({
            "conversationId": "agent-a",
            "snapshotCaptured": true,
            "reason": "auth"
        }))
        .expect("box help log");
    service.logs.report_projection(&queue_accepted_telemetry(&QueueAcceptedReport {
        conversation_id: "agent-a".into(), lane: "user".into(), source: "turn".into(),
        position: 0, depth_user: 1, depth_agent: 0, depth_background: 0, has_active: false,
    })).expect("queue telemetry");

    service
        .analytics
        .track_event(
            "sand.box_help",
            &json!({
                "agent_id": "agent-a",
                "snapshot_captured": true,
                "reason": "auth",
                "domain": "example.com",
                "ignored": null,
                "nested": {"not": "allowed"}
            }),
        )
        .expect("product analytics event");

    let text = fs::read_to_string(service.records_path()).expect("telemetry jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);

    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, "sand.box_help");
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(
        records[0].payload["metadata"]["conversation_id"],
        "agent-a"
    );
    assert_eq!(
        records[0].payload["metadata"]["snapshot_captured"],
        "true"
    );
    assert_eq!(records[0].payload["metadata"]["meta.reason"], "auth");

    assert_eq!(records[1].channel, "structured_log");
    assert_eq!(records[1].event, "sand.queue.accepted");
    assert_eq!(records[1].payload["level"], "info");
    assert_eq!(records[1].payload["metadata"]["conversation_id"], "agent-a");
    assert_eq!(records[1].payload["metadata"]["lane"], "user");
    assert_eq!(records[2].channel, "product_analytics");
    assert_eq!(records[2].event, "sand.box_help");
    assert_eq!(records[2].payload["agent_id"], "agent-a");
    assert_eq!(records[2].payload["snapshot_captured"], true);
    assert_eq!(records[2].payload["domain"], "example.com");
    assert!(records[2].payload.get("ignored").is_none());
    assert!(records[2].payload.get("nested").is_none());

    let _ = fs::remove_dir_all(root);
}


#[test]
fn box_store_sync_facade_preserves_frozen_event_level_and_metadata() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("box-store-sync-events.jsonl"))
        .expect("telemetry service");

    service
        .logs
        .report_box_store_sync_cycle(
            "info",
            &BTreeMap::from([
                ("ok".into(), "true".into()),
                ("reason".into(), "enabled".into()),
                ("phase".into(), "startup".into()),
            ]),
        )
        .expect("box store startup telemetry");
    service
        .logs
        .report_box_store_sync_cycle(
            "warn",
            &BTreeMap::from([
                ("ok".into(), "false".into()),
                ("reason".into(), "locked".into()),
                ("duration_ms".into(), "17".into()),
                ("manifest_entries".into(), "4".into()),
                ("store_db_entries".into(), "2".into()),
            ]),
        )
        .expect("box store locked telemetry");

    let records = fs::read_to_string(service.records_path())
        .expect("box store sync jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();

    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(record.channel, "structured_log");
        assert_eq!(record.event, "sand.box_store_sync");
    }
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["ok"], "true");
    assert_eq!(records[0].payload["metadata"]["reason"], "enabled");
    assert_eq!(records[0].payload["metadata"]["phase"], "startup");
    assert_eq!(records[1].payload["level"], "warn");
    assert_eq!(records[1].payload["metadata"]["ok"], "false");
    assert_eq!(records[1].payload["metadata"]["reason"], "locked");
    assert_eq!(records[1].payload["metadata"]["duration_ms"], "17");
    assert_eq!(records[1].payload["metadata"]["manifest_entries"], "4");
    assert_eq!(records[1].payload["metadata"]["store_db_entries"], "2");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn frozen_facade_methods_route_mapper_semantics_through_single_host_owner() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("facade-events.jsonl"))
        .expect("telemetry service");

    service.logs.report_auto_review_approval(&AutoReviewApprovalReport {
        event_type: "settled".into(),
        conversation_id: "agent-facade".into(),
        approval_id: "approval-1".into(),
        surface: "turn".into(),
        status: "approved".into(),
        age_ms: 12.6,
        ttl_ms: Some(50.2),
        cause: Some("user".into()),
    }).expect("auto review facade");
    service.logs.report_automation_fire_dropped(&AutomationFireDroppedReport {
        conversation_id: "agent-facade".into(),
        trigger: "schedule".into(),
        reason: "stale".into(),
        scheduled_for_ms: Some(10.0),
        lateness_ms: Some(20.0),
        error_type: None,
        error_code: None,
        run_uuid: Some("run-1".into()),
        fire_age_ms: Some(30.0),
        has_definition_revision: Some(true),
        box_uptime_ms: None,
    }).expect("automation dropped facade");
    service.logs.report_local_exec_refused(&LocalExecRefusedReport {
        cause: LocalExecRefusalCause::NoProviders,
        site: "exec".into(),
        conversation_id: "agent-facade".into(),
        provider_count: 0,
        live_provider_count: 0,
        ever_registered: false,
        empty_for_ms: Some(99.6),
    }).expect("local exec refused facade");
    service.logs.report_local_exec_provider(&LocalExecProviderReport::Registered {
        provider_id: "provider-1".into(),
        provider_count: 1,
    }).expect("local exec provider facade");
    service.logs.report_local_exec_failed(&LocalExecFailedReport {
        error_class: "spawn_enoent".into(),
        errno: Some("ENOENT".into()),
        site: "exec".into(),
        conversation_id: "agent-facade".into(),
    }).expect("local exec failed facade");
    service.logs.report_webauthn_proxy(&WebAuthnProxyReport {
        outcome: "failed".into(),
        stage: "sign".into(),
        origin_class: "https".into(),
        ceremony_kind: "get".into(),
        request_id: "request-1".into(),
        elapsed_ms: 41.6,
        provider_count: Some(1),
        live_provider_count: Some(0),
        cause: Some(WebAuthnFailureCause::Timeout),
        raw_dom_error_name: None,
        raw_sign_error_class: None,
    }).expect("webauthn facade");
    service.logs.report_memory_synthesis(&MemorySynthesisReport::SkippedGate)
        .expect("memory facade");
    service.logs.report_subagent_revival(&SubagentRevivalReport {
        parent_agent_id: "agent-facade".into(),
        outcome: "sent".into(),
        completion_count: 2,
        subagent_type: Some("task".into()),
        subagent_agent_id: Some("child-1".into()),
        reason: None,
        sent_message_count: Some(1),
        is_quiet_origin: Some(false),
    }).expect("subagent facade");
    service.logs.report_shell_revival(&ShellRevivalReport {
        conversation_id: "agent-facade".into(),
        outcome: "sent".into(),
        completion_count: 1,
        sent_message_count: Some(1),
        is_quiet_origin: Some(true),
        reason: None,
    }).expect("shell revival facade");
    service.logs.report_pending_wake(&PendingWakeReport {
        conversation_id: "agent-facade".into(),
        outcome: "lost".into(),
        kind: Some("shell".into()),
        work_id: Some("work-1".into()),
        age_ms: Some(10.6),
        reason: Some("agent_gone".into()),
        is_quiet_origin: Some(true),
    }).expect("pending wake facade");
    service.logs.report_box_disk_pressure(&DiskPressureReport {
        level: "hard".into(),
        volume: "/".into(),
        trigger: "heartbeat".into(),
        total_bytes: 1000.0,
        available_bytes: 100.0,
        used_percent: 90.04,
    }).expect("disk pressure facade");
    service.logs.report_queue_accepted(&QueueAcceptedReport {
        conversation_id: "agent-facade".into(),
        lane: "user".into(),
        source: "turn".into(),
        position: 0,
        depth_user: 1,
        depth_agent: 0,
        depth_background: 0,
        has_active: false,
    }).expect("queue accepted facade");
    service.logs.report_queue_dequeued(&mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::QueueDequeuedReport {
        conversation_id: "agent-facade".into(),
        lane: "user".into(),
        source: "turn".into(),
        queue_wait_ms: 5.6,
        accepted_to_run_ms: Some(7.4),
        jumped_background: 0,
        depth_user: 0,
        depth_agent: 0,
        depth_background: 0,
    }).expect("queue dequeued facade");
    service.logs.report_queue_watchdog(&mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::QueueWatchdogReport {
        conversation_id: "agent-facade".into(),
        stage: "trip".into(),
        active_lane: Some("user".into()),
        active_source: Some("turn".into()),
        active_runtime_ms: 100.4,
        waiting_user_age_ms: Some(9.6),
        interrupted: Some(true),
    }).expect("queue watchdog facade");
    service.logs.report_turn_empty_delivery(&TurnEmptyDeliveryReport {
        conversation_id: "agent-facade".into(),
        request_id: Some("request-1".into()),
        source: "turn".into(),
        request_source: Some("composer".into()),
        reply_nudge_attempts: Some(1),
        redrive_attempts: Some(0),
        tool_call_count: 2,
        stream_output_produced: false,
        duration_ms: 200.7,
        ack_outstanding: true,
    }).expect("turn empty facade");

    let text = fs::read_to_string(service.records_path()).expect("facade jsonl");
    let records = text.lines().map(|line| {
        serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("facade record")
    }).collect::<Vec<_>>();
    assert_eq!(records.len(), 15);
    assert_eq!(records.iter().map(|record| record.event.as_str()).collect::<Vec<_>>(), vec![
        "sand.auto_review.approval",
        "sand.automation.fire_dropped",
        "sand.local_exec.refused",
        "sand.local_exec.provider",
        "sand.local_exec.exec_failed",
        "sand.webauthn_proxy",
        "sand.memory.synthesis",
        "sand.subagent.revival",
        "sand.shell.revival",
        "sand.pending_wake",
        "sand.box.disk_pressure",
        "sand.queue.accepted",
        "sand.queue.dequeued",
        "sand.queue.watchdog",
        "sand.turn.empty_delivery",
    ]);
    assert_eq!(records[0].payload["metadata"]["age_ms"], "13");
    assert_eq!(records[2].payload["metadata"]["error_code"], "SAND-E0111");
    assert_eq!(records[5].payload["level"], "warn");
    assert_eq!(records[5].payload["metadata"]["error_code"], "SAND-E0209");
    assert_eq!(records[8].event, "sand.shell.revival");
    assert_eq!(records[8].payload["metadata"]["quiet_origin"], "true");
    assert_eq!(records[9].event, "sand.pending_wake");
    assert_eq!(records[9].payload["metadata"]["age_ms"], "11");
    assert_eq!(records[10].event, "sand.box.disk_pressure");
    assert_eq!(records[10].payload["level"], "error");
    assert_eq!(records[10].payload["metadata"]["used_percent"], "90.0");
    assert_eq!(records[12].payload["metadata"]["queue_wait_ms"], "6");
    assert_eq!(records[13].payload["metadata"]["active_runtime_ms"], "100");
    assert_eq!(records[14].payload["metadata"]["duration_ms"], "201");
    assert_eq!(records[14].payload["metadata"]["ack_outstanding"], "true");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn agent_error_facade_preserves_frozen_summary_detail_and_truncation() {
    use mahayana_host_runtime::extensions::telemetry::agent_error_telemetry::AgentErrorReport;
    use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;
    use mahayana_host_runtime::ports::telemetry::SandErrorDetail;

    let root = temp_root();
    let service = HostTelemetryService::open(root.join("agent-error-events.jsonl"))
        .expect("telemetry service");
    let long_message = "m".repeat(1_100);
    let long_stack = "s".repeat(4_200);
    service.logs.report_agent_error(&AgentErrorReport {
        source: "ack_redrive".into(),
        conversation_id: "agent-error".into(),
        request_id: Some("request-error".into()),
        error: SandErrorValue::new("SAND-E0402"),
        detail: Some(SandErrorDetail {
            message: long_message,
            stack: Some(long_stack),
        }),
    }).expect("agent error facade");

    let text = fs::read_to_string(service.records_path()).expect("agent error jsonl");
    let records = text.lines().map(|line| {
        serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("agent error record")
    }).collect::<Vec<_>>();
    assert_eq!(records.len(), 2, "summary + detail must both ship through the Host owner");
    assert_eq!(records[0].event, "sand.agent.error");
    assert_eq!(records[0].payload["level"], "error");
    assert_eq!(records[0].payload["metadata"]["source"], "ack_redrive");
    assert_eq!(records[0].payload["metadata"]["conversation_id"], "agent-error");
    assert_eq!(records[0].payload["metadata"]["request_id"], "request-error");
    assert_eq!(records[0].payload["metadata"]["error_code"], "SAND-E0402");
    assert_eq!(records[1].event, "sand.agent.error.detail");
    assert_eq!(records[1].payload["level"], "error");
    assert_eq!(records[1].payload["metadata"]["error_code"], "SAND-E0402");
    assert_eq!(records[1].payload["metadata"]["error_message"].as_str().unwrap().chars().count(), 1_024);
    assert_eq!(records[1].payload["metadata"]["error_stack"].as_str().unwrap().chars().count(), 4_096);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn desktop_health_file_forwards_through_shipping_structured_log_with_frozen_heartbeat() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("telemetry root");
    let health_path = root.join("desktop-health.json");
    fs::write(
        &health_path,
        r#"{
          "updatedAtMs": 100,
          "revision": 7,
          "supervisionEnabled": true,
          "components": [
            {
              "name": "d1/xvfb",
              "up": false,
              "crashloop": false,
              "restartsInWindow": 2,
              "downReason": "oom"
            }
          ]
        }"#,
    )
    .expect("desktop health fixture");

    let service =
        HostTelemetryService::open(root.join("desktop-health-events.jsonl"))
            .expect("telemetry service");
    let state = Mutex::new(DesktopHealthForwardState::default());

    assert_eq!(
        forward_desktop_health_file_to_logs(
            &health_path,
            &service.logs,
            &state,
            1_000,
        ),
        DesktopHealthForwardResult::Emitted
    );
    assert_eq!(
        forward_desktop_health_file_to_logs(
            &health_path,
            &service.logs,
            &state,
            1_000 + DESKTOP_HEALTH_HEARTBEAT_MS - 1,
        ),
        DesktopHealthForwardResult::Skipped
    );
    assert_eq!(
        forward_desktop_health_file_to_logs(
            &health_path,
            &service.logs,
            &state,
            1_000 + DESKTOP_HEALTH_HEARTBEAT_MS,
        ),
        DesktopHealthForwardResult::Emitted
    );

    let text = fs::read_to_string(service.records_path()).expect("desktop health jsonl");
    let records = text
        .lines()
        .map(|line| {
            serde_json::from_str::<PersistedHostTelemetryRecord>(line)
                .expect("desktop health record")
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(record.channel, "structured_log");
        assert_eq!(record.event, DESKTOP_HEALTH_EVENT);
        assert_eq!(record.payload["level"], "warn");
        assert_eq!(record.payload["metadata"]["overall"], "degraded");
        assert_eq!(record.payload["metadata"]["down"], "1");
        assert_eq!(
            record.payload["metadata"]["down_reason"],
            "primary/xvfb=oom"
        );
    }

    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_structured_logs_consume_canonical_box_identity_tags_with_event_precedence() {
    let root = temp_root();
    let service = HostTelemetryService::open_with_identity_tags(
        root.join("identity-events.jsonl"),
        BTreeMap::from([
            ("auth_id".to_string(), "box-auth".to_string()),
            ("box_store_id".to_string(), "store-a".to_string()),
            ("cluster".to_string(), "cluster-a".to_string()),
            ("empty".to_string(), String::new()),
        ]),
    )
    .expect("telemetry service");

    service
        .logs
        .report_projection(
            &mahayana_host_runtime::extensions::telemetry::HostTelemetryProjection {
                level: Some("info"),
                event: Some("sand.identity.contract"),
                metadata: BTreeMap::from([
                    ("auth_id".to_string(), "event-auth".to_string()),
                    ("detail".to_string(), "ok".to_string()),
                ]),
            },
        )
        .expect("identity-tagged structured log");

    let text = fs::read_to_string(service.records_path()).expect("identity jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("identity record"))
            .expect("identity record json");
    assert_eq!(record.event, "sand.identity.contract");
    assert_eq!(record.payload["metadata"]["auth_id"], "event-auth");
    assert_eq!(record.payload["metadata"]["box_store_id"], "store-a");
    assert_eq!(record.payload["metadata"]["cluster"], "cluster-a");
    assert_eq!(record.payload["metadata"]["detail"], "ok");
    assert!(record.payload["metadata"].get("empty").is_none());

    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_telemetry_brain_routes_automation_run_to_product_and_structured_channels() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("brain-events.jsonl"))
        .expect("telemetry service");
    service.start().expect("telemetry start");
    service.brain.report_automation_run(&json!({
        "conversationId": "agent-auto",
        "automationId": "auto-7",
        "trigger": "schedule",
        "outcome": "ok",
        "isGroup": false,
        "sentMessageCount": 2
    }));

    let text = fs::read_to_string(service.records_path()).expect("telemetry jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].channel, "product_analytics");
    assert_eq!(records[0].event, "sand.automation.run");
    assert_eq!(records[0].payload["agent_id"], "agent-auto");
    assert_eq!(records[0].payload["automation_id"], "auto-7");
    assert_eq!(records[0].payload["sent_message_count"], 2);
    assert_eq!(records[1].channel, "structured_log");
    assert_eq!(records[1].event, "sand.automation.run");
    assert_eq!(records[1].payload["conversationId"], "agent-auto");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn structured_log_turn_handle_preserves_frozen_start_retry_and_finalize_semantics() {
    use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;

    let root = temp_root();
    let service = HostTelemetryService::open(root.join("turn-events.jsonl"))
        .expect("telemetry service");
    service.start().expect("telemetry start");

    let mut turn = service
        .logs
        .start_turn("agent-turn", "user", None);
    turn.set_request_id("request-first");
    turn.set_request_id("request-ignored");
    turn.note_retry(Some(125), Some("transient"));
    turn.note_retry(Some(75), None);
    turn.set_model("model-a");
    turn.set_model("model-b");
    let error = SandErrorValue::new("SAND-E0402");
    turn.finalize(
        "failed",
        Some(&error),
        Some(("provider failed", Some("synthetic stack"))),
    );
    turn.finalize("completed", None, None);

    let text = fs::read_to_string(service.records_path()).expect("turn telemetry jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3, "start + outcome + detail must each emit once");

    let start = &records[0];
    assert_eq!(start.event, "sand.turn.start");
    assert_eq!(start.payload["metadata"]["conversation_id"], "agent-turn");
    assert_eq!(start.payload["metadata"]["turn_type"], "user");
    assert_eq!(start.payload["metadata"]["request_id"], "request-first");
    assert_eq!(start.payload["metadata"]["model_intent"], "model-a");

    let outcome = &records[1];
    assert_eq!(outcome.event, "sand.turn.outcome");
    assert_eq!(outcome.payload["metadata"]["outcome"], "error");
    assert_eq!(outcome.payload["metadata"]["request_id"], "request-first");
    assert_eq!(outcome.payload["metadata"]["model_intent"], "model-b");
    assert_eq!(outcome.payload["metadata"]["retry_count"], "2");
    assert_eq!(outcome.payload["metadata"]["backoff_total_ms"], "200");
    assert_eq!(outcome.payload["metadata"]["retry_cause"], "transient");
    assert_eq!(outcome.payload["metadata"]["error_code"], "SAND-E0402");
    assert!(outcome.payload["metadata"]["duration_ms"].as_str().is_some());

    let detail = &records[2];
    assert_eq!(detail.event, "sand.turn.outcome_detail");
    assert_eq!(detail.payload["metadata"]["error_code"], "SAND-E0402");
    assert_eq!(detail.payload["metadata"]["error_message"], "provider failed");
    assert_eq!(detail.payload["metadata"]["error_stack"], "synthetic stack");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn direct_frozen_facade_helpers_preserve_teach_upgrade_and_box_help_semantics() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("direct-facade-events.jsonl"))
        .expect("telemetry service");

    service.logs.report_teach_recording_cap_stop_failed(&json!({
        "errorClass": "stop_failed"
    })).expect("teach cap telemetry");
    service.logs.report_teach_recording_start_failed(&json!({
        "kind": "window",
        "errorClass": "capture_failed",
        "windowIndex": 2,
        "entryPoint": "toolbar"
    })).expect("teach start telemetry");
    service.logs.report_host_upgrade(BTreeMap::from([
        ("outcome".into(), "failed".into()),
        ("from_version".into(), "1".into()),
        ("to_version".into(), "2".into()),
    ])).expect("host upgrade telemetry");
    service.logs.report_box_help(&json!({
        "conversationId": "agent-direct",
        "snapshotCaptured": false,
        "reason": "auth"
    })).expect("box help telemetry");

    let records = fs::read_to_string(service.records_path())
        .expect("direct facade jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].event, "sand.teach.cap_stop_failed");
    assert_eq!(records[0].payload["level"], "warn");
    assert_eq!(records[0].payload["metadata"]["error_class"], "stop_failed");
    assert_eq!(records[1].event, "sand.teach.recording_start_failed");
    assert_eq!(records[1].payload["metadata"]["window_index"], "2");
    assert_eq!(records[1].payload["metadata"]["entry_point"], "toolbar");
    assert_eq!(records[2].event, "sand.host.upgrade");
    assert_eq!(records[2].payload["level"], "warn");
    assert_eq!(records[2].payload["metadata"]["outcome"], "failed");
    assert_eq!(records[3].event, "sand.box_help");
    assert_eq!(records[3].payload["metadata"]["conversation_id"], "agent-direct");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn mcp_plugin_and_permission_facades_preserve_frozen_direct_event_semantics() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("mcp-plugin-facade-events.jsonl"))
        .expect("telemetry service");

    service.logs.report_mcp_auth_cleanup("error", 2).expect("mcp cleanup");
    service.logs.report_local_tool_permission_stranded_retirement().expect("stranded permission");
    service.logs.report_skill_publish_edge_failed("upload", "timeout").expect("skill edge");
    service.logs.report_plugin_skills_sync(
        "auth-renewal",
        "failed",
        false,
        3,
        Some("load_failed"),
        41,
    ).expect("plugin skills sync");

    let records = fs::read_to_string(service.records_path())
        .expect("direct facade jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].event, "sand.mcp_auth_cleanup");
    assert_eq!(records[0].payload["level"], "warn");
    assert_eq!(records[0].payload["metadata"]["removed_count"], "2");
    assert_eq!(records[1].event, "sand.client_resource");
    assert_eq!(records[1].payload["metadata"]["failure_code"], "permissions/stranded-ask-retired");
    assert_eq!(records[2].event, "sand.skill_publish.edge_failed");
    assert_eq!(records[2].payload["metadata"]["stage"], "upload");
    assert_eq!(records[3].event, "sand.plugin_skills.sync");
    assert_eq!(records[3].payload["level"], "warn");
    assert_eq!(records[3].payload["metadata"]["duration_ms"], "41");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_host_lifecycle_progress_routes_through_single_structured_log_owner() {
    struct CountingDisposable(Arc<AtomicU64>);
    impl Disposable for CountingDisposable {
        fn dispose(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let root = temp_root();
    let service = Arc::new(
        HostTelemetryService::open(root.join("host-events.jsonl")).expect("telemetry service"),
    );
    service.start().expect("telemetry start");
    let api = service.api();
    let disposed = Arc::new(AtomicU64::new(0));
    let watchdog: WatchdogArm = {
        let disposed = Arc::clone(&disposed);
        Arc::new(move |_callback| Box::new(CountingDisposable(Arc::clone(&disposed))))
    };

    let mut completed = api.create_host_lifecycle_progress(
        api.monotonic_now_ms(),
        Arc::clone(&watchdog),
    );
    completed
        .complete(HostLifecycleCompletion {
            phase: "plugin_graph".into(),
            plugin_count: Some(4),
            entry_count: None,
        })
        .expect("complete plugin graph lifecycle");
    drop(completed);

    let mut failed = api.create_host_lifecycle_progress(
        api.monotonic_now_ms(),
        Arc::clone(&watchdog),
    );
    failed.fail();
    drop(failed);

    let text = fs::read_to_string(service.records_path()).expect("lifecycle jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);

    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, "sand.host.lifecycle");
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["phase"], "plugin_graph");
    assert_eq!(records[0].payload["metadata"]["outcome"], "completed");
    assert_eq!(records[0].payload["metadata"]["plugin_count"], "4");

    assert_eq!(records[1].channel, "structured_log");
    assert_eq!(records[1].event, "sand.host.lifecycle");
    assert_eq!(records[1].payload["level"], "error");
    assert_eq!(records[1].payload["metadata"]["phase"], "plugin_graph");
    assert_eq!(records[1].payload["metadata"]["outcome"], "failed");
    assert_eq!(records[1].payload["metadata"]["error_code"], "SAND-E0303");

    assert!(
        disposed.load(Ordering::SeqCst) >= 3,
        "completion/failure/drop must dispose or re-arm lifecycle watchdogs"
    );

    let _ = fs::remove_dir_all(root);
}


#[test]
fn ordinary_host_crash_uses_fire_and_forget_structured_owner() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("host-crash.jsonl")).expect("telemetry");
    service.start().expect("start");

    service
        .logs
        .report_host_crash("panic")
        .expect("ordinary host crash");

    let text = fs::read_to_string(service.records_path()).expect("crash jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, "sand.host.crash");
    assert_eq!(records[0].payload["level"], "error");
    assert_eq!(records[0].payload["metadata"]["kind"], "panic");
    assert!(records[0].payload["metadata"]["error_code"].is_null());

    service.dispose().expect("dispose");
    let _ = fs::remove_dir_all(root);
}


#[test]
fn box_store_db_capture_facade_preserves_frozen_event_level_and_metadata() {
    let root = temp_root();
    let service =
        HostTelemetryService::open(root.join("box-store-db-capture.jsonl")).expect("telemetry");
    service.start().expect("start");

    service
        .logs
        .report_box_store_db_capture(
            "warn",
            &BTreeMap::from([
                ("outcome".into(), "error".into()),
                ("trigger".into(), "turn_end".into()),
                ("failure_phase".into(), "manifest_commit".into()),
                ("committed".into(), "false".into()),
                ("agent_count".into(), "1".into()),
                ("files_scanned".into(), "2".into()),
                ("files_uploaded".into(), "1".into()),
                ("bytes".into(), "4096".into()),
                ("duration_ms".into(), "23".into()),
                ("queue_duration_ms".into(), "5".into()),
                ("capture_duration_ms".into(), "7".into()),
                ("blob_upload_duration_ms".into(), "8".into()),
                ("manifest_commit_duration_ms".into(), "3".into()),
                ("store_id".into(), "store-a".into()),
            ]),
        )
        .expect("store db capture telemetry");

    let text = fs::read_to_string(service.records_path()).expect("store db capture jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("store db capture record"))
            .expect("store db capture json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, "sand.box_store_db_capture");
    assert_eq!(record.payload["level"], "warn");
    assert_eq!(record.payload["metadata"]["outcome"], "error");
    assert_eq!(record.payload["metadata"]["trigger"], "turn_end");
    assert_eq!(record.payload["metadata"]["failure_phase"], "manifest_commit");
    assert_eq!(record.payload["metadata"]["committed"], "false");
    assert_eq!(record.payload["metadata"]["agent_count"], "1");
    assert_eq!(record.payload["metadata"]["files_scanned"], "2");
    assert_eq!(record.payload["metadata"]["files_uploaded"], "1");
    assert_eq!(record.payload["metadata"]["bytes"], "4096");
    assert_eq!(record.payload["metadata"]["duration_ms"], "23");
    assert_eq!(record.payload["metadata"]["queue_duration_ms"], "5");
    assert_eq!(record.payload["metadata"]["capture_duration_ms"], "7");
    assert_eq!(record.payload["metadata"]["blob_upload_duration_ms"], "8");
    assert_eq!(record.payload["metadata"]["manifest_commit_duration_ms"], "3");
    assert_eq!(record.payload["metadata"]["store_id"], "store-a");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn box_store_manifest_conflict_facade_preserves_frozen_warn_event_and_metadata() {
    let root = temp_root();
    let service =
        HostTelemetryService::open(root.join("box-store-manifest-conflict.jsonl")).expect("telemetry");
    service.start().expect("start");

    service
        .logs
        .report_box_store_manifest_conflict(
            "warn",
            &BTreeMap::from([
                ("store_id".into(), "store-a".into()),
                ("attempts".into(), "3".into()),
                ("accepted".into(), "false".into()),
                ("covered".into(), "true".into()),
                ("canonical_matches_attempt".into(), "false".into()),
                ("live_view_changed".into(), "false".into()),
                ("attempted_entries".into(), "7".into()),
                ("last_base_etag".into(), "etag-a".into()),
                ("last_baseline_source".into(), "manifest-cas".into()),
                ("last_conflict_rel_path".into(), "conflicts/lost.json".into()),
                ("canonical_readable".into(), "true".into()),
                ("canonical_entry_count".into(), "8".into()),
                ("canonical_updated_at_ms".into(), "42".into()),
                ("canonical_writer_window_id".into(), "winner".into()),
                ("our_window_id".into(), "loser".into()),
            ]),
        )
        .expect("manifest conflict telemetry");

    let text =
        fs::read_to_string(service.records_path()).expect("manifest conflict telemetry jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("manifest conflict record"))
            .expect("manifest conflict json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, "sand.box_store_manifest_conflict");
    assert_eq!(record.payload["level"], "warn");
    assert_eq!(record.payload["metadata"]["store_id"], "store-a");
    assert_eq!(record.payload["metadata"]["attempts"], "3");
    assert_eq!(record.payload["metadata"]["accepted"], "false");
    assert_eq!(record.payload["metadata"]["covered"], "true");
    assert_eq!(
        record.payload["metadata"]["canonical_matches_attempt"],
        "false"
    );
    assert_eq!(record.payload["metadata"]["live_view_changed"], "false");
    assert_eq!(record.payload["metadata"]["attempted_entries"], "7");
    assert_eq!(record.payload["metadata"]["last_base_etag"], "etag-a");
    assert_eq!(
        record.payload["metadata"]["last_baseline_source"],
        "manifest-cas"
    );
    assert_eq!(
        record.payload["metadata"]["last_conflict_rel_path"],
        "conflicts/lost.json"
    );
    assert_eq!(record.payload["metadata"]["canonical_readable"], "true");
    assert_eq!(record.payload["metadata"]["canonical_entry_count"], "8");
    assert_eq!(record.payload["metadata"]["canonical_updated_at_ms"], "42");
    assert_eq!(
        record.payload["metadata"]["canonical_writer_window_id"],
        "winner"
    );
    assert_eq!(record.payload["metadata"]["our_window_id"], "loser");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn chrome_session_stage_facade_preserves_frozen_event_level_and_metadata() {
    let root = temp_root();
    let service =
        HostTelemetryService::open(root.join("chrome-session-stage.jsonl")).expect("telemetry");
    service.start().expect("start");

    service
        .logs
        .report_chrome_session_stage(
            "warn",
            &BTreeMap::from([
                ("staged".into(), "1".into()),
                ("skipped".into(), "1".into()),
                ("skipped_dbs".into(), "Web Data".into()),
                ("error_class".into(), "Error".into()),
                ("failure_db".into(), "Web Data".into()),
                ("failure_phase".into(), "raw_copy".into()),
                ("failure_operation".into(), "copy_source_sidecars".into()),
                (
                    "failure_path_stage".into(),
                    "source_and_staged_sidecars".into(),
                ),
                ("failure_cause".into(), "io".into()),
                ("errno".into(), "ENOSPC".into()),
                ("sqlite_code".into(), "13".into()),
            ]),
        )
        .expect("chrome session stage telemetry");

    let text = fs::read_to_string(service.records_path()).expect("chrome session stage jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("chrome session stage record"))
            .expect("chrome session stage json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, "sand.chrome_session_stage");
    assert_eq!(record.payload["level"], "warn");
    assert_eq!(record.payload["metadata"]["staged"], "1");
    assert_eq!(record.payload["metadata"]["skipped"], "1");
    assert_eq!(record.payload["metadata"]["skipped_dbs"], "Web Data");
    assert_eq!(record.payload["metadata"]["error_class"], "Error");
    assert_eq!(record.payload["metadata"]["failure_db"], "Web Data");
    assert_eq!(record.payload["metadata"]["failure_phase"], "raw_copy");
    assert_eq!(
        record.payload["metadata"]["failure_operation"],
        "copy_source_sidecars"
    );
    assert_eq!(
        record.payload["metadata"]["failure_path_stage"],
        "source_and_staged_sidecars"
    );
    assert_eq!(record.payload["metadata"]["failure_cause"], "io");
    assert_eq!(record.payload["metadata"]["errno"], "ENOSPC");
    assert_eq!(record.payload["metadata"]["sqlite_code"], "13");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn auto_review_display_recheck_facade_matches_frozen_structured_log_contract() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("auto-review-display-recheck.jsonl"))
        .expect("telemetry service");

    service
        .logs
        .report_auto_review_display_recheck_failed("agent-display")
        .expect("display recheck telemetry");

    let text = fs::read_to_string(service.records_path()).expect("telemetry jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, "sand.auto_review.display_recheck_failed");
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(
        records[0].payload["metadata"]["conversation_id"],
        "agent-display"
    );
    assert_eq!(records[0].payload["metadata"]["surface"], "computer");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn tool_call_facades_match_frozen_started_and_error_contracts() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("tool-call-events.jsonl"))
        .expect("telemetry service");

    service
        .logs
        .report_tool_call_started(
            "agent-tool",
            Some("request-tool"),
            "Shell",
            "call-started",
            "box",
        )
        .expect("tool started");
    service
        .logs
        .report_tool_call_error(
            "agent-tool",
            Some("request-tool"),
            "Read",
            "call-failed",
            "task_error_result",
            91,
            "unknown",
        )
        .expect("tool error");

    let records = fs::read_to_string(service.records_path())
        .expect("tool telemetry jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].event, "sand.tool_call.started");
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["conversation_id"], "agent-tool");
    assert_eq!(records[0].payload["metadata"]["request_id"], "request-tool");
    assert_eq!(records[0].payload["metadata"]["tool_name"], "Shell");
    assert_eq!(records[0].payload["metadata"]["tool_call_id"], "call-started");
    assert_eq!(records[0].payload["metadata"]["surface"], "box");

    assert_eq!(records[1].event, "sand.tool_call.error");
    assert_eq!(records[1].payload["level"], "error");
    assert_eq!(records[1].payload["metadata"]["conversation_id"], "agent-tool");
    assert_eq!(records[1].payload["metadata"]["request_id"], "request-tool");
    assert_eq!(records[1].payload["metadata"]["tool_name"], "Read");
    assert_eq!(records[1].payload["metadata"]["tool_call_id"], "call-failed");
    assert_eq!(records[1].payload["metadata"]["error_class"], "task_error_result");
    assert_eq!(records[1].payload["metadata"]["duration_ms"], "91");
    assert_eq!(records[1].payload["metadata"]["connector"], "unknown");

    service
        .logs
        .report_tool_call_stalled(
            "agent-tool",
            Some("request-tool"),
            "mcpToolCall",
            "call-stalled",
            "github",
            900_000,
        )
        .expect("tool stalled");

    let records = fs::read_to_string(service.records_path())
        .expect("tool telemetry jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].event, "sand.tool_call.stalled");
    assert_eq!(records[2].payload["level"], "warn");
    assert_eq!(records[2].payload["metadata"]["conversation_id"], "agent-tool");
    assert_eq!(records[2].payload["metadata"]["request_id"], "request-tool");
    assert_eq!(records[2].payload["metadata"]["tool_name"], "mcpToolCall");
    assert_eq!(records[2].payload["metadata"]["tool_call_id"], "call-stalled");
    assert_eq!(records[2].payload["metadata"]["connector"], "github");
    assert_eq!(records[2].payload["metadata"]["elapsed_ms"], "900000");

    let _ = fs::remove_dir_all(root);
}


#[test]
fn journal_outcome_facade_matches_frozen_jsonl_contract() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("journal-outcome.jsonl"))
        .expect("telemetry service");

    service
        .logs
        .report_journal_outcome(&JournalOutcomeReport {
            outcome: "failed".into(),
            op: "append".into(),
            conversation_id: "agent-journal".into(),
            entry_count: Some(3),
            bytes: Some(120),
            duration_ms: 17.6,
            cause: Some(SandErrorValue::new("SAND-E0720").with_string("errno", "EIO")),
        })
        .expect("journal outcome");

    let text = fs::read_to_string(service.records_path()).expect("journal outcome jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("record")).expect("json");

    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, "sand.journal.outcome");
    assert_eq!(record.payload["level"], "error");
    assert_eq!(record.payload["metadata"]["op"], "append");
    assert_eq!(record.payload["metadata"]["outcome"], "failed");
    assert_eq!(record.payload["metadata"]["conversation_id"], "agent-journal");
    assert_eq!(record.payload["metadata"]["entry_count"], "3");
    assert_eq!(record.payload["metadata"]["bytes"], "120");
    assert_eq!(record.payload["metadata"]["duration_ms"], "18");
    assert_eq!(record.payload["metadata"]["error_code"], "SAND-E0720");
    assert_eq!(record.payload["metadata"]["error_domain"], "storage");
    assert_eq!(record.payload["metadata"]["error_retryable"], "true");
    assert_eq!(record.payload["metadata"]["errno"], "EIO");

    let _ = fs::remove_dir_all(root);
}
