use mahayana_host_runtime::host_gateway_api::{
    CREATE_AGENT_NONCE_LEDGER_CAP, CreateAgentNonceLedger, sanitize_create_agent_args,
};
use mahayana_host_runtime::sand_host::{
    BOX_READY_REPORT_ATTEMPTS, BOX_READY_REPORT_RETRY_MS, BOX_READY_STAGE_MARKER_PATH,
    box_ready_duration_ms, compute_host_health, should_report_box_ready,
};
use serde_json::json;

const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const TRANSCRIPT_RUNTIME: &str =
    include_str!("../src/extensions/transcript/production_runtime.rs");
const TRANSCRIPT_MANAGER: &str =
    include_str!("../src/extensions/transcript/transcript_manager.rs");

#[test]
fn shipping_gateway_uses_the_grok_host_health_owner() {
    let health = SHIPPING_HOST
        .find("fn health(&self) -> GatewayHealth")
        .expect("shipping Host health owner");
    let prepare_upgrade = SHIPPING_HOST[health..]
        .find("fn prepare_for_upgrade(&self)")
        .map(|offset| health + offset)
        .expect("health owner must finish before upgrade preparation");
    let block = &SHIPPING_HOST[health..prepare_upgrade];

    assert!(block.contains("compute_host_health("));
    assert!(block.contains("self.transcript_manager.live_running_agent_ids()"));
    assert!(block.contains("self.transcript_manager.has_carryable_pending_wake()"));
    assert!(block.contains("self.transcript_manager.active_agent_id()"));
    assert!(block.contains("generated_agent_runtime"));
    assert!(block.contains("runtime.has_running_subagents()"));
    assert!(block.contains("completion_revivals.has_mid_drain_revival()"));
    assert!(block.contains("background_shell_watches.has_running_background_shell_work()"));
    assert!(block.contains("agent_ids_with_pending_approvals()"));
    assert!(
        !block.contains("active_agent_id(&self.session_workers)"),
        "shipping Gateway health must consume the TranscriptManager facade rather than bypass it",
    );

    assert!(TRANSCRIPT_MANAGER.contains("pub fn active_agent_id(&self) -> Option<String>"));
    assert!(
        TRANSCRIPT_MANAGER.contains(
            "self.transcript_runtime.active_agent_id(&self.session_workers)"
        ),
        "TranscriptManager must preserve the production Session/Transcript active-agent owner",
    );
    assert!(TRANSCRIPT_RUNTIME.contains("pub fn live_running_agent_ids"));
    assert!(TRANSCRIPT_RUNTIME.contains("pub fn has_carryable_pending_wake"));
}

#[test]
fn shipping_agent_deletion_wires_tray_and_durable_recovery_cleanup_owners() {
    let lifecycle = SHIPPING_HOST
        .find("let agent_deletion_runtime = AgentDeletionRuntimeDeps {")
        .expect("shipping Host must compose lifecycle deletion owners");
    let gateway = SHIPPING_HOST[lifecycle..]
        .find("let gateway_config =")
        .map(|offset| lifecycle + offset)
        .expect("lifecycle composition must finish before gateway start");
    let block = &SHIPPING_HOST[lifecycle..gateway];
    assert!(block.contains("mark_deleting: Some({"));
    assert!(block.contains("clear_deleting: Some({"));
    assert!(block.contains("session_runtime().mark_agent_deleted(agent_id)"));
    assert!(block.contains("session_runtime().clear_agent_deleted(agent_id)"));
    assert!(block.contains("clear_trays: Some({"));
    assert!(block.contains("production_extensions.trays"));
    assert!(block.contains("trays.clear_for_agent(agent_id)"));
    assert!(block.contains("dispose_background_work: Some({"));
    assert!(block.contains("cloud_agent_deletion_watches"));
    assert!(block.contains("background_shell_deletion_watches"));
    assert!(SHIPPING_HOST.contains("watches.dispose_parent(agent_id)"));
    assert!(SHIPPING_HOST.contains("Arc::downgrade(&cloud_agent_watches)"));
    assert!(SHIPPING_HOST.contains("Arc::downgrade(&background_shell_watches)"));
    assert!(block.contains("drain_runner: Some({"));
    assert!(block.contains("abort_running_subagents_for_parent(agent_id)"));
    assert!(block.contains("runner_registry.cancel_agent(child_id, \"parent agent deleted\")"));
    assert!(block.contains("running_subagent_ids_for_parent(agent_id).is_empty()"));
    assert!(block.contains("!transcript_runtime.is_agent_running(agent_id)"));
    assert!(block.contains("clear_pending_wakes: Some({"));
    assert!(block.contains("transcript_runtime.clear_agent_durable_recovery(agent_id)"));
    assert!(TRANSCRIPT_RUNTIME.contains("pub fn clear_agent_durable_recovery"));
    assert!(TRANSCRIPT_RUNTIME.contains("store.clear_agent(agent_id)"));

    assert!(
        TRANSCRIPT_MANAGER.contains("pub fn dispatch_agent_lifecycle_gateway_call("),
        "TranscriptManager must own the lifecycle Gateway facade",
    );
    assert!(
        TRANSCRIPT_MANAGER.contains(
            "dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes("
        ),
        "TranscriptManager must preserve the all-runtimes production lifecycle dispatcher",
    );
    for needle in [
        "&self.session_workers",
        "deletion_runtime",
        "self.roster_emit()",
        "kickstart_created_agent",
        "method",
        "args",
    ] {
        assert!(
            TRANSCRIPT_MANAGER.contains(needle),
            "TranscriptManager lifecycle facade missing production dependency: {needle}",
        );
    }

    let lifecycle_dispatch = SHIPPING_HOST
        .find(".dispatch_agent_lifecycle_gateway_call(")
        .expect("shipping Host must consume the TranscriptManager lifecycle facade");
    let session_dispatch = SHIPPING_HOST[lifecycle_dispatch..]
        .find(".dispatch_session_gateway_call(method, &args)")
        .map(|offset| lifecycle_dispatch + offset)
        .expect("session dispatcher follows lifecycle dispatcher");
    let post_lifecycle = &SHIPPING_HOST[lifecycle_dispatch..session_dispatch];
    assert!(
        !post_lifecycle.contains("session_runtime().mark_agent_deleted"),
        "shipping Host must not recreate deletion state transitions after the canonical lifecycle facade",
    );
}

#[test]
fn health_keeps_approval_only_waits_from_refreshing_busy_clock() {
    let health = compute_host_health(
        ["agent-a".to_string()],
        ["agent-a".to_string()],
        false,
        Some("agent-a".into()),
        900,
        500,
    );
    assert!(health.is_busy);
    assert!(health.busy_only_awaiting_approval);
    assert_eq!(health.last_busy_at_ms, 500);

    let active = compute_host_health(
        ["agent-a".to_string()],
        Vec::<String>::new(),
        false,
        Some("agent-a".into()),
        900,
        500,
    );
    assert!(active.is_busy);
    assert!(!active.busy_only_awaiting_approval);
    assert_eq!(active.last_busy_at_ms, 900);
}

#[test]
fn shipping_create_agent_path_consumes_nonce_and_input_policy() {
    assert!(SHIPPING_HOST.contains(r#"method == "createAgent""#));
    assert!(SHIPPING_HOST.contains("sanitize_create_agent_args"));
    assert!(SHIPPING_HOST.contains("create_agent_nonces"));
    let create = SHIPPING_HOST
        .find(r#"if method == "createAgent" {"#)
        .expect("shipping createAgent branch");
    let secrets = SHIPPING_HOST[create..]
        .find("dispatch_secrets_gateway_call")
        .map(|offset| create + offset)
        .expect("createAgent branch must finish before secrets dispatch");
    let create_block = &SHIPPING_HOST[create..secrets];
    assert!(create_block.contains(".transcript_manager"));
    assert!(create_block.contains(".dispatch_agent_lifecycle_gateway_call("));
    assert!(create_block.contains("&self.agent_deletion_runtime"));
    assert!(create_block.contains("Some(self.created_agent_kickstart_hook())"));
    assert!(!create_block.contains("dispatch_production_session_gateway_call("));
    assert!(
        !create_block.contains("dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes("),
        "shipping Host must consume the TranscriptManager facade instead of rebuilding lifecycle dispatch",
    );
    assert!(
        TRANSCRIPT_MANAGER.contains(
            "dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes("
        ),
        "TranscriptManager must keep the canonical all-runtimes createAgent path",
    );

    let sanitized = sanitize_create_agent_args(&json!({
        "name": "Agent",
        "purpose": "not-sand",
        "templateId": "../escape"
    }));
    assert!(sanitized.get("purpose").is_none());
    assert!(sanitized.get("templateId").is_none());

    let mut ledger = CreateAgentNonceLedger::default();
    for index in 0..(CREATE_AGENT_NONCE_LEDGER_CAP + 3) {
        ledger.insert(format!("nonce-{index}"), json!({ "id": index }));
    }
    assert_eq!(ledger.len(), CREATE_AGENT_NONCE_LEDGER_CAP);
    assert!(ledger.get("nonce-0").is_none());
    assert_eq!(
        ledger
            .get(&format!("nonce-{}", CREATE_AGENT_NONCE_LEDGER_CAP + 2))
            .and_then(|value| value.get("id"))
            .and_then(serde_json::Value::as_u64),
        Some((CREATE_AGENT_NONCE_LEDGER_CAP + 2) as u64),
    );
}

#[test]
fn shipping_host_reports_box_ready_only_after_gateway_discovery() {
    let discovery = SHIPPING_HOST.find("write_gateway_discovery(").unwrap();
    let ready = SHIPPING_HOST
        .find("should_report_box_ready(")
        .expect("shipping Host must wire frozen box-ready report");
    assert!(discovery < ready);
    assert!(SHIPPING_HOST.contains("report_box_boot_stage_confirmed"));
    assert!(SHIPPING_HOST.contains("BOX_READY_REPORT_ATTEMPTS"));
    assert!(SHIPPING_HOST.contains("BOX_READY_REPORT_RETRY_MS"));
    assert!(SHIPPING_HOST.contains("BOX_READY_STAGE_MARKER_PATH"));

    assert_eq!(BOX_READY_REPORT_ATTEMPTS, 3);
    assert_eq!(BOX_READY_REPORT_RETRY_MS, 30_000);
    assert_eq!(BOX_READY_STAGE_MARKER_PATH, "/tmp/sand-box-ready-stage");
    assert!(should_report_box_ready(Some("boot-a"), Some(100), None));
    assert!(!should_report_box_ready(
        Some("boot-a"),
        Some(100),
        Some("boot-a"),
    ));
    assert_eq!(box_ready_duration_ms(250, 100), 150);
}

#[test]
fn shipping_host_copy_in_mode_precedes_long_lived_runtime_bootstrap() {
    let copy_in = SHIPPING_HOST
        .find("std::env::args().any(|arg| arg == BOX_COPY_IN_ARG)")
        .expect("shipping Host must recognize frozen --box-copy-in mode");
    let crash_guard = SHIPPING_HOST
        .find("install_process_crash_guard")
        .expect("shipping Host must install process crash guards");
    let host_lock = SHIPPING_HOST
        .find("let host_lock = match acquire_host_lock(")
        .expect("shipping Host must acquire the single-host lock");
    assert!(copy_in < crash_guard);
    assert!(copy_in < host_lock);
    assert!(SHIPPING_HOST.contains("execute_production_box_copy_in_from_env"));
    assert!(SHIPPING_HOST.contains("std::process::exit(exit_code)"));
}

#[test]
fn shipping_host_composes_one_durable_background_shell_rewatch_owner() {
    assert!(SHIPPING_HOST.contains("RunnerBackgroundShellWatches::new("));
    assert!(SHIPPING_HOST.contains("read_background_shell_terminal(shell_id)"));
    assert!(SHIPPING_HOST.contains("poll_shell_terminal_file("));
    assert!(SHIPPING_HOST.contains("kind: PendingWakeKind::Shell"));
    assert!(SHIPPING_HOST.contains("handle_background_shell_completion(ShellCompletion"));
    assert!(SHIPPING_HOST.contains("background_shell_watches: Arc<RunnerBackgroundShellWatches>"));
    assert!(SHIPPING_HOST.contains(".with_background_shell_watches(Arc::clone(&background_shell_watches))"));
    assert!(SHIPPING_HOST.contains("deps.background_shell_watches"));
    assert!(!SHIPPING_HOST.contains("production shell pending-wake rearm is not wired yet"));
}
