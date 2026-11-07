use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::TranscriptWindowQuery;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::transcript_manager::TranscriptManager;

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-row79-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("root");
    root
}

#[test]
fn transcript_manager_is_the_shared_owner_for_switch_tail_and_windowed_open() {
    let root = temp_root("manager-owner");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&workers));
    let first = store.create_session(None, "user", None).expect("first");
    let second = store.create_session(None, "user", None).expect("second");
    let manager = TranscriptManager::new(root.join("transcript"), Arc::clone(&workers));

    manager.switch_agent(&first.id, 1_900_000_000_000.0).expect("switch first");
    assert_eq!(manager.active_agent_id().as_deref(), Some(first.id.as_str()));

    let query = TranscriptWindowQuery { before_seq: None, limit: 200 };
    manager
        .open_agent_tail(&second.id, query, 1_900_000_000_001.0)
        .expect("cold tail");
    assert_eq!(
        manager.active_agent_id().as_deref(),
        Some(first.id.as_str()),
        "frozen cold bounded open marks viewed but does not synchronously replace the active session"
    );

    manager
        .transcript_runtime()
        .session_runtime()
        .resolve_background_session(&workers, &second.id)
        .expect("make second live");
    manager
        .open_agent_windowed(&second.id, query, 1_900_000_000_002.0)
        .expect("live windowed");
    assert_eq!(
        manager.active_agent_id().as_deref(),
        Some(second.id.as_str()),
        "frozen already-live bounded open activates through the same manager"
    );

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_gateway_samples_was_active_before_operation_and_reports_after_success() {
    let app = include_str!("../app/src/main.rs");
    let sample = app.find("let previous_active_agent_id = self.transcript_manager.active_agent_id();").expect("pre-open active sample");
    let operation = app.find("let (response, entry_count) = match method").expect("open operation");
    let report = app.find("self.telemetry_logs.report_agent_open(&AgentOpenReport").expect("Host report");
    assert!(sample < operation);
    assert!(operation < report);
    assert!(app.contains(r#"matches!(method, "openAgent" | "openAgentWindowed" | "openAgentTail")"#));
    assert!(app.contains(".transcript_manager\n                        .open_agent_tail(agent_id, query, now_ms)"));
    assert!(app.contains(".transcript_manager\n                        .open_agent_windowed(agent_id, query, now_ms)"));

    let manager = include_str!("../src/extensions/transcript/transcript_manager.rs");
    assert!(manager.contains("pub fn active_agent_id(&self) -> Option<String>"));
    assert!(manager.contains("pub fn open_agent_tail("));
    assert!(manager.contains("pub fn open_agent_windowed("));
    assert!(manager.contains("if self.prepare_live_bounded_open(agent_id, now_ms)?"));
    assert!(manager.contains("self.session_workers.mark_agent_viewed(agent_id, now_ms, false)?"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_agent_open(&self, report: &AgentOpenReport)"));
    assert!(owner.contains("self.report_projection(&agent_open_telemetry(report))"));

    let mapper = include_str!("../src/extensions/telemetry/agent_open_telemetry.rs");
    assert_eq!(mapper.matches("sand.agent.open").count(), 1);

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportAgentOpen"));
    assert!(!coordinator.contains("report_agent_open"));
    assert!(!coordinator.contains("sand.agent.open"));

    let electron = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportAgentOpen"));
    assert!(!electron.contains("sand.agent.open"));
}
