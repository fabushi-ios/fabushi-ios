use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::time::Duration;

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::local_exec::extension::{
    LOCAL_EXEC_DEPENDENCIES, local_exec_extension_id,
};
use mahayana_host_runtime::extensions::local_exec::local_exec_bridge::{
    DEFAULT_SAND_COMPUTER_ID, LocalExecProviderLifecycleReport, LocalExecRefusalCause,
    LocalExecRefusalReport, SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS, SandLocalExecBridge,
    bounded_local_exec_variant, local_exec_provider_rank,
};
use serde_json::json;

#[test]
fn provider_rank_and_variant_projection_match_frozen_grok() {
    assert_eq!(local_exec_provider_rank(true, Some("sand")), 6);
    assert_eq!(local_exec_provider_rank(true, Some("sand-lab")), 5);
    assert_eq!(local_exec_provider_rank(false, Some("sand")), 2);
    assert_eq!(local_exec_provider_rank(false, Some("sand-dev")), 0);
    assert_eq!(bounded_local_exec_variant(Some("sand")), Some("sand"));
    assert_eq!(
        bounded_local_exec_variant(Some("sand-lab")),
        Some("sand-lab")
    );
    assert_eq!(
        bounded_local_exec_variant(Some("sand-dev")),
        Some("sand-dev")
    );
    assert_eq!(bounded_local_exec_variant(Some("custom")), Some("unknown"));
    assert_eq!(bounded_local_exec_variant(None), None);
    assert_eq!(local_exec_extension_id(), HostExtensionId::LocalExec);
    assert_eq!(
        LOCAL_EXEC_DEPENDENCIES,
        &[
            HostExtensionId::LocalToolPermission,
            HostExtensionId::Telemetry
        ]
    );
}

#[test]
fn provider_registration_hello_liveness_and_selection_follow_frozen_transport() {
    let now = Arc::new(AtomicU64::new(1000));
    let now_for_bridge = Arc::clone(&now);
    let ids = Arc::new(Mutex::new(vec![
        "provider-b".to_string(),
        "provider-a".to_string(),
    ]));
    let ids_for_bridge = Arc::clone(&ids);
    let reports = Arc::new(Mutex::new(Vec::new()));
    let reports_for_bridge = Arc::clone(&reports);
    let bridge = SandLocalExecBridge::with_sources_and_reporter(
        Arc::new(move || now_for_bridge.load(Ordering::SeqCst)),
        Arc::new(move || ids_for_bridge.lock().expect("ids").pop().expect("id")),
        Some(Arc::new(move |report| {
            reports_for_bridge.lock().expect("reports").push(report);
        })),
    );

    let (send, receive) = mpsc::channel();
    let registration = bridge.register_provider(send);
    assert_eq!(
        receive
            .recv_timeout(Duration::from_millis(100))
            .expect("welcome"),
        json!({"kind":"welcome","providerId":"provider-a"})
    );
    assert!(bridge.has_provider());
    assert!(bridge.ever_registered());
    assert_eq!(
        reports.lock().expect("reports").as_slice(),
        &[LocalExecProviderLifecycleReport::Registered {
            provider_id: "provider-a".into(),
            provider_count: 1,
        }]
    );

    bridge.submit_responses(json!({
        "providerId": "provider-a",
        "frames": [{
            "kind": "hello",
            "localRoot": "/Users/test",
            "terminalsFolder": "terminals",
            "computerId": "mac-1",
            "label": "My Mac",
            "supervised": true,
            "variant": "sand"
        }]
    }));
    assert_eq!(bridge.active_computer().expect("active").id, "mac-1");
    assert_eq!(
        bridge.get_provider_info().expect("info").terminals_folder,
        "terminals"
    );
    assert_eq!(
        reports.lock().expect("reports")[1],
        LocalExecProviderLifecycleReport::Hello {
            provider_id: "provider-a".into(),
            provider_count: 1,
            hello_delay_ms: 0,
            computer_id_present: true,
            rehello: false,
            supervised: Some(true),
            variant: Some("sand".into()),
        }
    );

    bridge.submit_responses(json!({
        "providerId": "provider-a",
        "frames": [{
            "kind": "hello",
            "localRoot": "/Users/test",
            "terminalsFolder": "terminals",
            "computerId": "mac-1",
            "label": "My Mac",
            "supervised": true,
            "variant": "sand"
        }]
    }));
    assert_eq!(
        reports.lock().expect("reports")[2],
        LocalExecProviderLifecycleReport::Hello {
            provider_id: "provider-a".into(),
            provider_count: 1,
            hello_delay_ms: 0,
            computer_id_present: true,
            rehello: true,
            supervised: Some(true),
            variant: Some("sand".into()),
        }
    );

    bridge.submit_responses(json!({
        "providerId": "provider-a",
        "frames": [{"kind":"ping","supervised":true}]
    }));
    now.store(
        1000 + SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS + 1,
        Ordering::SeqCst,
    );
    assert!(!bridge.is_computer_live("mac-1"));
    assert!(!bridge.check_live_computer_for_ask());

    drop(registration);
    assert!(!bridge.has_provider());
    assert_eq!(
        reports.lock().expect("reports")[3],
        LocalExecProviderLifecycleReport::Detached {
            provider_id: "provider-a".into(),
            provider_count: 0,
            age_ms: SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS + 1,
            had_hello: true,
            has_heartbeat: true,
            was_live: false,
            emptied: true,
        }
    );
}

#[test]
fn unavailable_computer_reports_frozen_refusal_causes() {
    let now = Arc::new(AtomicU64::new(1000));
    let now_for_bridge = Arc::clone(&now);
    let reports = Arc::new(Mutex::new(Vec::new()));
    let reports_for_bridge = Arc::clone(&reports);
    let bridge = SandLocalExecBridge::with_sources_and_reporters(
        Arc::new(move || now_for_bridge.load(Ordering::SeqCst)),
        Arc::new(|| "provider-1".to_string()),
        None,
        Some(Arc::new(move |report| {
            reports_for_bridge.lock().expect("refusals").push(report);
        })),
    );

    assert!(!bridge.check_live_computer_for_ask_with_agent(Some("agent-empty")));
    assert_eq!(
        reports.lock().expect("refusals")[0],
        LocalExecRefusalReport {
            cause: LocalExecRefusalCause::NoProviders,
            site: "ask_gate".into(),
            conversation_id: Some("agent-empty".into()),
            provider_count: 0,
            live_provider_count: 0,
            ever_registered: false,
            empty_for_ms: Some(0),
        }
    );

    let (send, receive) = mpsc::channel();
    let _registration = bridge.register_provider(send);
    let _ = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("welcome");
    bridge.submit_responses(json!({
        "providerId": "provider-1",
        "frames": [{
            "kind": "hello",
            "localRoot": "/tmp",
            "terminalsFolder": "terminals",
            "computerId": "mac-1",
            "label": "My Mac"
        }]
    }));
    let error = bridge
        .assert_computer_available(Some("missing"), "download", Some("agent-1"))
        .expect_err("unknown computer");
    assert_eq!(error.message, mahayana_host_runtime::extensions::local_exec::local_exec_bridge::SAND_NO_LOCAL_MACHINE_MESSAGE);
    assert_eq!(
        reports.lock().expect("refusals")[1].cause,
        LocalExecRefusalCause::ComputerUnknown
    );

    bridge.submit_responses(json!({
        "providerId": "provider-1",
        "frames": [{"kind":"ping"}]
    }));
    now.store(
        1000 + SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS + 1,
        Ordering::SeqCst,
    );
    let error = bridge
        .assert_computer_available(Some("mac-1"), "upload", Some("agent-1"))
        .expect_err("stale computer");
    assert!(error.message.contains("My Mac"));
    let report = reports.lock().expect("refusals")[2].clone();
    assert_eq!(report.cause, LocalExecRefusalCause::StaleHeartbeat);
    assert_eq!(report.site, "upload");
    assert_eq!(report.provider_count, 1);
    assert_eq!(report.live_provider_count, 0);
    assert!(report.ever_registered);
}

#[test]
fn request_response_and_retire_approval_flow_through_registered_provider() {
    let bridge = SandLocalExecBridge::with_sources(
        Arc::new(|| 5000),
        Arc::new({
            let next = Arc::new(AtomicU64::new(1));
            move || format!("id-{}", next.fetch_add(1, Ordering::SeqCst))
        }),
    );
    let (send, receive) = mpsc::channel();
    let _registration = bridge.register_provider(send);
    let _ = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("welcome");

    bridge.submit_responses(json!({
        "frames": [{
            "kind": "hello",
            "localRoot": "/tmp",
            "terminalsFolder": "terminals"
        }]
    }));
    assert_eq!(
        bridge.active_computer().expect("active").id,
        DEFAULT_SAND_COMPUTER_ID
    );

    let mut request = bridge
        .request(json!({"kind":"download","path":"/tmp/a"}), None)
        .expect("request");
    let outbound = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("request frame");
    let request_id = outbound
        .get("requestId")
        .and_then(|value| value.as_str())
        .expect("request id");
    assert_eq!(outbound["kind"], "download");

    bridge.submit_responses(json!({
        "frames": [{
            "kind": "file",
            "requestId": request_id,
            "bytesBase64": "YQ=="
        }]
    }));
    assert_eq!(
        request
            .recv_timeout(Duration::from_millis(100))
            .expect("response")["kind"],
        "file"
    );
    request.close();
    assert_eq!(
        receive
            .recv_timeout(Duration::from_millis(100))
            .expect("cancel")["kind"],
        "cancel"
    );

    bridge.retire_approval("approval-1");
    let retired = receive
        .recv_timeout(Duration::from_millis(100))
        .expect("retire");
    assert_eq!(retired["kind"], "retire-approval");
    assert_eq!(retired["approvalId"], "approval-1");
}

#[test]
fn refusal_duration_uses_injected_clock_and_saturates_on_clock_rollback() {
    let now = Arc::new(AtomicU64::new(1000));
    let now_for_bridge = Arc::clone(&now);
    let reports = Arc::new(Mutex::new(Vec::<LocalExecRefusalReport>::new()));
    let reports_for_bridge = Arc::clone(&reports);
    let bridge = SandLocalExecBridge::with_sources_and_reporters(
        Arc::new(move || now_for_bridge.load(Ordering::SeqCst)),
        Arc::new(|| "unused-provider-id".to_string()),
        None,
        Some(Arc::new(move |report| {
            reports_for_bridge.lock().expect("reports").push(report);
        })),
    );

    now.store(1037, Ordering::SeqCst);
    assert!(!bridge.check_live_computer_for_ask_with_agent(Some("agent-clock")));
    now.store(999, Ordering::SeqCst);
    assert!(!bridge.check_live_computer_for_ask_with_agent(Some("agent-clock")));

    let reports = reports.lock().expect("reports");
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].empty_for_ms, Some(37));
    assert_eq!(reports[1].empty_for_ms, Some(0));
    for report in reports.iter() {
        assert_eq!(report.cause, LocalExecRefusalCause::NoProviders);
        assert_eq!(report.site, "ask_gate");
        assert_eq!(report.conversation_id.as_deref(), Some("agent-clock"));
        assert_eq!(report.provider_count, 0);
        assert_eq!(report.live_provider_count, 0);
        assert!(!report.ever_registered);
    }
}
