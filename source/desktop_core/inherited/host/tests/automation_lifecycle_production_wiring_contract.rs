use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::automations::automation_id::stable_automation_id;
use mahayana_host_runtime::automations::automation_schedule::summarize_schedule_next_7_days;

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunTrigger,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::{
    AutomationLifecycleAction, AutomationLifecycleEvent, AutomationLifecycleSource, AutomationRuntime,
};
use mahayana_host_runtime::extensions::transcript::sand_automation_spend_guard::{
    SPEND_GUARD_IDLE_TTL_MS, SPEND_GUARD_PAUSE_DELAY_MS,
};
use mahayana_host_runtime::extensions::transcript::transcript_manager::TranscriptManager;
use serde_json::json;


fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-row74-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("root");
    root
}

#[test]
fn real_ui_producer_flushes_pending_agent_delta_before_ui_delta() {
    let root = temp_root("producer");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let captured = Arc::new(Mutex::new(Vec::<AutomationLifecycleEvent>::new()));
    runtime.set_lifecycle_reporter(Some({
        let captured = Arc::clone(&captured);
        Arc::new(move |event| captured.lock().expect("events").push(event.clone()))
    }));

    let spec = AutomationSpec {
        name: "Row 74".into(),
        prompt: "Review".into(),
        trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
        is_enabled: Some(true),
    };
    let (records, events) = runtime
        .create_agent_automation(&agent.id, &spec)
        .expect("create");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Created);
    assert_eq!(events[0].source, AutomationLifecycleSource::AutomationsUi);
    assert_eq!(
        events[0].stable_automation_id,
        stable_automation_id(&agent.id, &records[0].id)
    );
    assert!(events[0].scheduled_fires_next_7_days.is_some());

    let store = sessions.automation_store_for(&agent.id).expect("store");
    store.set_enabled(&records[0].id, false).expect("agent edit");
    runtime
        .update_agent_automation(
            &agent.id,
            &records[0].id,
            &AutomationSpec {
                name: "Row 74 renamed".into(),
                ..spec
            },
        )
        .expect("ui update");

    let captured = captured.lock().expect("events");
    assert_eq!(captured.len(), 3);
    assert_eq!(captured[0].action, AutomationLifecycleAction::Created);
    assert_eq!(captured[0].source, AutomationLifecycleSource::AutomationsUi);
    assert_eq!(captured[1].action, AutomationLifecycleAction::Disabled);
    assert_eq!(captured[1].source, AutomationLifecycleSource::Agent);
    assert_eq!(captured[2].action, AutomationLifecycleAction::Updated);
    assert_eq!(captured[2].source, AutomationLifecycleSource::AutomationsUi);
    drop(captured);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn real_workflow_ui_wrapper_attributes_scheduled_workflow_creation() {
    use mahayana_host_runtime::workflows::workflow_library::{WorkflowSpec, WorkflowTrigger};

    let root = temp_root("workflow");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let (_workflows, events) = runtime
        .with_workflow_ui_mutation(&agent.id, |session| {
            session.create_agent_workflow(
                &agent.id,
                &WorkflowSpec {
                    name: "Scheduled workflow".into(),
                    description: String::new(),
                    body: "Check queue".into(),
                    trigger: Some(WorkflowTrigger {
                        schedule: "0 8 * * *".into(),
                        is_enabled: true,
                    }),
                    source_ref: None,
                },
            )
        })
        .expect("workflow create");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Created);
    assert_eq!(events[0].source, AutomationLifecycleSource::WorkflowUi);
    assert_eq!(events[0].trigger_type, "cron");

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn active_store_watcher_reports_agent_change_at_change_time() {
    let root = temp_root("watcher");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let manager = TranscriptManager::new(root.join("transcript"), Arc::clone(&workers));
    let runtime = manager.automation_runtime();
    let captured = Arc::new(Mutex::new(Vec::<AutomationLifecycleEvent>::new()));
    runtime.set_lifecycle_reporter(Some({
        let captured = Arc::clone(&captured);
        Arc::new(move |event| captured.lock().expect("events").push(event.clone()))
    }));
    let (created, _) = runtime
        .create_agent_automation(
            &agent.id,
            &AutomationSpec {
                name: "Watched".into(),
                prompt: "Watch".into(),
                trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    manager.switch_agent(&agent.id, 1_900_000_000_000.0).expect("switch");
    captured.lock().expect("events").clear();

    sessions
        .automation_store_for(&agent.id)
        .expect("store")
        .set_enabled(&created[0].id, false)
        .expect("agent change");

    for _ in 0..40 {
        if !captured.lock().expect("events").is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    let captured = captured.lock().expect("events");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].action, AutomationLifecycleAction::Disabled);
    assert_eq!(captured[0].source, AutomationLifecycleSource::Agent);
    drop(captured);

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}

fn prepare_away_agent(workers: &ProductionSessionWorkers, agent_id: &str, now_ms: f64) {
    let last_viewed = now_ms - SPEND_GUARD_IDLE_TTL_MS - 10_000.0;
    workers.mark_agent_viewed(agent_id, last_viewed, false).expect("viewed");
    for index in 0..15 {
        workers
            .mark_agent_activity(agent_id, last_viewed + index as f64 + 1.0)
            .expect("activity");
    }
}

#[test]
fn real_spend_guard_pause_reports_disabled_after_transition() {
    let root = temp_root("spend-guard");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let captured = Arc::new(Mutex::new(Vec::<AutomationLifecycleEvent>::new()));
    runtime.set_lifecycle_reporter(Some({
        let captured = Arc::clone(&captured);
        Arc::new(move |event| captured.lock().expect("events").push(event.clone()))
    }));
    let (created, _) = runtime
        .create_agent_automation(
            &agent.id,
            &AutomationSpec {
                name: "Guarded".into(),
                prompt: "Do it".into(),
                trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    captured.lock().expect("events").clear();

    let now_ms = 1_900_000_000_000.0;
    prepare_away_agent(&workers, &agent.id, now_ms);
    runtime
        .run_background_automation_with(
            &agent.id,
            &created[0].id,
            AutomationRunTrigger::Schedule,
            Vec::new(),
            Some("row74-first".into()),
            Vec::new(),
            now_ms,
            None,
            |_| Ok(AutomationExecutionResult::Completed),
        )
        .expect("first run");
    captured.lock().expect("events").clear();

    let paused_at = now_ms + SPEND_GUARD_PAUSE_DELAY_MS + 1.0;
    let outcome = runtime
        .run_background_automation_with(
            &agent.id,
            &created[0].id,
            AutomationRunTrigger::Schedule,
            Vec::new(),
            Some("row74-pause".into()),
            Vec::new(),
            paused_at,
            None,
            |_| panic!("paused routine must not execute"),
        )
        .expect("pause");
    assert_eq!(outcome, None);
    let captured = captured.lock().expect("events");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].action, AutomationLifecycleAction::Disabled);
    assert_eq!(captured[0].source, AutomationLifecycleSource::SpendGuard);
    drop(captured);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn frozen_schedule_summary_keeps_subminute_and_timezone_semantics() {
    let dense = summarize_schedule_next_7_days(
        "@every 30s",
        Some("America/Los_Angeles"),
        1_900_000_000_000.0,
    );
    assert_eq!(dense.scheduled_fires_next_7_days, 20_160);
    assert!(dense.fires_on_weekend);
    assert!(dense.fires_overnight);

    let daily = summarize_schedule_next_7_days(
        "0 9 * * *",
        Some("America/Los_Angeles"),
        1_900_000_000_000.0,
    );
    assert_eq!(daily.scheduled_fires_next_7_days, 7);
    assert!(daily.fires_on_weekend);
    assert!(!daily.fires_overnight);
}

#[test]
fn shipping_runtime_owns_all_frozen_lifecycle_sources_and_stable_fields() {
    let runtime = include_str!("../src/extensions/transcript/automation_runtime.rs");
    assert!(runtime.contains("stable_automation_id(agent_id, &snapshot.id)"));
    assert!(runtime.contains("summarize_schedule_next_7_days("));
    assert!(runtime.contains("AutomationLifecycleSource::Agent"));
    assert!(runtime.contains("AutomationLifecycleSource::AutomationsUi"));
    assert!(runtime.contains("AutomationLifecycleSource::WorkflowUi"));
    assert!(runtime.contains("AutomationLifecycleSource::SpendGuard"));
    assert!(runtime.contains("let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);"));
    assert!(runtime.contains("store.set_on_change(Some(Arc::new(move ||"));
    assert!(runtime.contains("runtime.record_agent_store_change(&watched_agent_id)"));

    let snapshot = include_str!("../src/extensions/transcript/automation_snapshot.rs");
    assert!(snapshot.contains("None => Some(AutomationLifecycleAction::Created)") == false);
    assert!(snapshot.contains("AutomationAction::Updated"));
    assert!(snapshot.contains("AutomationAction::Enabled"));
    assert!(snapshot.contains("AutomationAction::Disabled"));

    let workflow = include_str!("../src/extensions/transcript/workflow_commands.rs");
    assert!(workflow.contains(".with_workflow_ui_mutation(agent_id, mutation)"));

    let spend_guard = include_str!("../src/extensions/transcript/automation_spend_guard_runtime.rs");
    assert!(spend_guard.contains("disable_every_enabled_routine"));
    assert!(spend_guard.contains("re_enable_guard_paused_routines"));
    assert!(spend_guard
        .matches("self.sessions.with_agent_admitted_db_mutation(agent_id, |owner|")
        .count()
        >= 2);
    let sessions = include_str!("../src/extensions/session/production.rs");
    let admitted_db = sessions
        .split("pub(crate) fn with_agent_admitted_db_mutation<T>(")
        .nth(1)
        .expect("canonical admitted DB callback")
        .split("pub fn begin_agent_delete(")
        .next()
        .expect("callback body");
    assert!(admitted_db.contains("self.with_agent_filesystem_mutation(agent_id, ||"));
    assert!(admitted_db.contains("self.assert_agent_work_admission_locked(agent_id)?;"));
    assert!(admitted_db.contains("self.open_agent_db_owner_locked(agent_id)?;"));
    assert!(admitted_db.contains("operation(owner.as_ref())"));
}

#[test]
fn production_composition_routes_to_unique_host_structured_log_owner() {
    let manager = include_str!("../src/extensions/transcript/transcript_manager.rs");
    assert!(manager.contains("watch_agent_automations(agent_id)"));
    assert!(manager.contains("previous.set_on_change(None)"));

    let app = include_str!("../app/src/main.rs");
    assert!(app.contains(".set_lifecycle_reporter(Some(Arc::new(move |event|"));
    assert!(app.contains("lifecycle_logs.report_automation_lifecycle(event)"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_automation_lifecycle("));
    assert!(owner.contains("self.report_projection(&automation_lifecycle_telemetry(report))"));

    let mapper = include_str!("../src/extensions/telemetry/automation_lifecycle_telemetry.rs");
    assert_eq!(mapper.matches("sand.automation.lifecycle").count(), 1);
    assert!(mapper.contains(r#"level: Some("info")"#));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportAutomationLifecycle"));
    assert!(!coordinator.contains("report_automation_lifecycle"));
    assert!(!coordinator.contains("sand.automation.lifecycle"));

    let electron = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportAutomationLifecycle"));
    assert!(!electron.contains("sand.automation.lifecycle"));
}
