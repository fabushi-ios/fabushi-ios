use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunObservation, AutomationRunTrigger, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::AutomationRuntime;
use serde_json::json;

fn root() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-run-production-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn real_shipping_runtime_reports_after_terminal_settlement() {
    let root = root();
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let (created, _) = runtime
        .create_agent_automation(
            &agent.id,
            &AutomationSpec {
                name: "Telemetry run".into(),
                prompt: "Inspect the queue".into(),
                trigger: json!({"type":"cron","schedule":"@daily"}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    let automation_id = created[0].id.clone();

    let observed = Arc::new(Mutex::new(Vec::<AutomationRunObservation>::new()));
    let observed_sink = Arc::clone(&observed);
    runtime.run_path().set_run_reporter(Some(Arc::new(move |report| {
        observed_sink.lock().unwrap().push(report.clone());
    })));

    let outcome = runtime
        .run_agent_automation_now_with(&agent.id, &automation_id, |_| {
            Ok(AutomationExecutionResult::Completed)
        })
        .expect("run");
    assert_eq!(outcome, Some(FireAutomationOutcome::Ok));

    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    let report = &observed[0];
    assert_eq!(report.agent_id, agent.id);
    assert_eq!(report.automation_id, automation_id);
    assert_eq!(report.trigger, AutomationRunTrigger::Manual);
    assert_eq!(report.outcome, FireAutomationOutcome::Ok);
    assert_eq!(report.is_group, Some(false));
    assert_eq!(report.scheduled_for_ms, None);
    assert_eq!(report.lateness_ms, None);
    assert_eq!(report.event_batch_size, None);
    assert!(report.duration_ms >= 0.0);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_composition_uses_canonical_mapper_and_unique_host_owner() {
    const HOST: &str = include_str!("../app/src/main.rs");
    const RUN_PATH: &str = include_str!("../src/extensions/transcript/automation_run_path.rs");
    const OWNER: &str = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    const COORDINATOR: &str = include_str!("../../node-agent-coordinator/src/main.rs");
    const ELECTRON: &str = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");

    assert!(RUN_PATH.contains("reporter(&AutomationRunObservation"));
    assert!(HOST.contains("set_run_reporter(Some(Arc::new(move |observed|"));
    assert!(HOST.contains("stable_automation_id("));
    assert!(HOST.contains("run_logs.report_automation_run(&report)"));
    assert!(OWNER.contains("pub fn report_automation_run(&self, report: &AutomationRunReport)"));
    assert!(OWNER.contains("self.report_projection(&automation_run_telemetry(report))"));
    assert!(!COORDINATOR.contains("sand.automation.run"));
    assert!(!ELECTRON.contains("sand.automation.run"));
}
