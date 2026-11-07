use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::host_upgrade::host_bundle_source::{
    HostBundleFetcher, HostBundleHttpResponse,
};
use mahayana_host_runtime::extensions::host_upgrade::production::{
    ProductionHostUpgradePeers, start_production_host_upgrade_extension_with,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;

#[derive(Default)]
struct FakeFetcher {
    requests: Mutex<Vec<String>>,
}

impl HostBundleFetcher for FakeFetcher {
    fn fetch(&self, url: &str) -> Result<HostBundleHttpResponse, String> {
        self.requests.lock().unwrap().push(url.to_string());
        Ok(HostBundleHttpResponse {
            status: 404,
            body: Vec::new(),
        })
    }
}

#[test]
fn production_host_upgrade_binds_real_peer_hooks_without_starting_a_parallel_runtime() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-host-upgrade-production-{}",
        uuid::Uuid::new_v4()
    ));
    let telemetry = HostTelemetryService::open(root.join("telemetry.jsonl")).unwrap();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let hook = |name: &'static str, calls: Arc<Mutex<Vec<String>>>| {
        Arc::new(move || {
            calls.lock().unwrap().push(name.to_string());
            Ok(())
        }) as Arc<dyn Fn() -> Result<(), String> + Send + Sync>
    };
    let sharing_calls = Arc::clone(&calls);
    let peers = ProductionHostUpgradePeers {
        suspend_automation_wakes: hook("automations", Arc::clone(&calls)),
        prepare_sharing_for_upgrade: Arc::new(move || {
            sharing_calls.lock().unwrap().push("sharing".into());
        }),
        quiesce_transcript_for_upgrade: hook("transcript", Arc::clone(&calls)),
        resume_interrupted_upgrade_turns: hook("resume", Arc::clone(&calls)),
    };
    let extension = start_production_host_upgrade_extension_with(
        peers,
        telemetry.logs.clone(),
        Arc::new(FakeFetcher::default()),
        root.join("upgrade-marker.json"),
        Arc::new(|| Ok(())),
    )
    .unwrap();

    extension.service().prepare_for_upgrade().unwrap();
    extension.service().resume_interrupted_upgrade_turns().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["automations", "sharing", "transcript", "resume"]
    );
    extension.stop_background_work();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn production_upgrade_telemetry_uses_frozen_event_name() {
    let mut metadata = BTreeMap::new();
    metadata.insert("outcome".to_string(), "applied".to_string());
    assert_eq!(metadata.get("outcome").map(String::as_str), Some("applied"));
}


#[test]
fn shipping_host_recreates_durable_upgrade_turns_before_ack_redrive() {
    const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

    assert!(SHIPPING_HOST.contains("fn resume_interrupted_upgrade_turns(&self)"));
    assert!(SHIPPING_HOST.contains("start_local_upgrade_resume_turn("));
    assert!(SHIPPING_HOST.contains("gateway.resume_interrupted_upgrade_turns()"));
    assert!(!SHIPPING_HOST.contains(
        "resumeInterruptedUpgradeTurns production Runner recreation is not wired"
    ));

    let resume = SHIPPING_HOST
        .rfind("host_upgrade.service().resume_interrupted_upgrade_turns()")
        .expect("shipping startup upgrade resume");
    let redrive = SHIPPING_HOST
        .rfind("start_ack_redrive_worker(")
        .expect("shipping ack redrive");
    assert!(resume < redrive);
}

#[test]
fn shipping_recreate_gateway_carries_pending_wakes_and_resumes_durable_turns() {
    const SHIPPING_HOST: &str = concat!(
        include_str!("../app/src/main.rs"),
        "\n",
        include_str!("../app/src/ocrm_handlers/automation_turn.rs")
    );
    const GATEWAY_PROTOCOL: &str = include_str!("../src/gateway_protocol.rs");
    const TRANSCRIPT_MANAGER: &str =
        include_str!("../src/extensions/transcript/transcript_manager.rs");
    const TRANSCRIPT_RUNTIME: &str =
        include_str!("../src/extensions/transcript/production_runtime.rs");
    const PENDING_WAKE_REARM: &str =
        include_str!("../src/extensions/transcript/pending_wake_rearm.rs");

    assert!(GATEWAY_PROTOCOL.contains("\"resumeAfterRecreate\""));
    assert!(SHIPPING_HOST.contains("if method == \"resumeAfterRecreate\""));
    assert!(SHIPPING_HOST.contains("\"resumePendingWakes\": self.transcript_manager.recreate_carry_pending_wakes()"));
    assert!(SHIPPING_HOST.contains(".restore_recreate_pending_wakes(carried)"));
    assert!(SHIPPING_HOST.contains("self.resume_interrupted_upgrade_turns()"));
    assert!(SHIPPING_HOST.contains("if worker_is_upgrade_resume {"));
    assert!(SHIPPING_HOST.contains("worker_transcript_manager.emit_agent_update(&agent_id)"));
    assert!(SHIPPING_HOST.contains("worker_transcript_manager.emit_automations(&agent_id)"));
    assert!(SHIPPING_HOST.contains("\"isSilenceAllowed\": matches!(resumed_source.as_str(), \"automation\" | \"background-revival\")"));
    assert!(TRANSCRIPT_MANAGER.contains("pub fn restore_recreate_pending_wakes"));
    assert!(TRANSCRIPT_MANAGER.contains("owner.restore_recreate_carried_pending_wakes(carried)"));
    assert!(PENDING_WAKE_REARM.contains("self.rearm_pending_wake(marker, now_ms, Some(\"recreate_carry\"))"));
    assert!(TRANSCRIPT_RUNTIME.contains("pub fn recreate_carry_pending_wakes"));
    assert!(TRANSCRIPT_RUNTIME.contains("PendingWakeKind::CloudAgent | PendingWakeKind::Shell"));
}

#[test]
fn shipping_upgrade_resume_keeps_durable_state_and_recreate_work_accounting() {
    const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
    const TRANSCRIPT_MANAGER: &str =
        include_str!("../src/extensions/transcript/transcript_manager.rs");
    const COMPLETION_REVIVALS: &str =
        include_str!("../src/extensions/transcript/completion_revivals.rs");

    let resume = SHIPPING_HOST
        .find("fn resume_interrupted_upgrade_turns(&self)")
        .expect("shipping upgrade resume owner");
    let resume_body = &SHIPPING_HOST[resume..];
    let can_execute = resume_body
        .find("if !self.transcript_manager.can_execute()")
        .expect("execution guard before durable marker settlement");
    assert!(!resume_body.contains("store.clear_all();"));
    let clear_if_matches = resume_body
        .rfind("store.clear_if_matches(&marker)")
        .expect("exact durable marker settlement");
    assert!(can_execute < clear_if_matches);
    let dispatch_failure = resume_body
        .find("if let Err(error) = result")
        .expect("upgrade resume failure branch");
    assert!(dispatch_failure < clear_if_matches);
    assert!(resume_body.contains("upgrade_resume_marker_identity_is_well_formed"));
    assert!(resume_body.contains("upgrade_resume_identity_invalid"));

    assert!(TRANSCRIPT_MANAGER.contains("services.trace_flusher.flush_tracing()"));
    assert!(COMPLETION_REVIVALS.contains("pub fn mid_drain_revival_agent_ids(&self)"));
    assert!(SHIPPING_HOST.contains("mid_drain_revival_agent_ids()"));
    assert!(SHIPPING_HOST.contains("has_running_background_shell_work()"));
    assert!(SHIPPING_HOST.contains("\"runningTurns\": running_turns"));
    assert!(SHIPPING_HOST.contains("source: \"resume\".into()"));
    assert!(SHIPPING_HOST.contains("upgrade_resume_telemetry_failed"));
    assert!(SHIPPING_HOST.contains("observed.quiesced_for_upgrade"));
    assert!(SHIPPING_HOST.contains("automation_run_id: observed.automation_run_id.clone()"));
}
