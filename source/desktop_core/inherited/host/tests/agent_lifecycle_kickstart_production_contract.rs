use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::{
    AgentDeletionRuntimeDeps, AgentKickstartHook, CreatedAgentKickstartRuntimePort,
    KickstartRunError, KickstartTurnOutcome, REPLY_NUDGE_PROMPT,
    SAND_DISK_SAVER_KICKSTART_PROMPT, SAND_DISK_SAVER_REAUDIT_PROMPT,
    SAND_ONBOARDING_KICKSTART_PROMPT,
    dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes,
    request_disk_saver_audit, run_created_agent_kickstart,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::classify_send_dispatch;
use mahayana_host_runtime::extensions::transcript::run_scheduler::RunLane;
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-kickstart-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[derive(Default)]
struct FakeKickstartRuntime {
    ready: bool,
    can_execute: bool,
    disallowed: bool,
    in_flight: bool,
    outcomes: Mutex<VecDeque<Result<KickstartTurnOutcome, KickstartRunError>>>,
    calls: Mutex<Vec<(String, String, String)>>,
    resume: Mutex<Vec<(String, String)>>,
    failures: Mutex<Vec<String>>,
    audit_failures: Mutex<Vec<String>>,
    trays: Mutex<Vec<String>>,
    roster_updates: Mutex<Vec<String>>,
}

impl CreatedAgentKickstartRuntimePort for FakeKickstartRuntime {
    fn is_run_ready(&self) -> bool {
        self.ready
    }

    fn can_execute(&self) -> bool {
        self.can_execute
    }

    fn is_disallowed_session(&self, _agent_id: &str) -> bool {
        self.disallowed
    }

    fn is_run_in_flight(&self, _agent_id: &str) -> bool {
        self.in_flight
    }

    fn run_hidden(
        &self,
        agent_id: &str,
        prompt: &str,
        source: &str,
    ) -> Result<KickstartTurnOutcome, KickstartRunError> {
        self.calls.lock().expect("calls").push((
            agent_id.to_string(),
            prompt.to_string(),
            source.to_string(),
        ));
        self.outcomes
            .lock()
            .expect("outcomes")
            .pop_front()
            .expect("scripted outcome")
    }

    fn mark_resume_pending(&self, agent_id: &str, source: &str) -> Result<(), String> {
        self.resume
            .lock()
            .expect("resume")
            .push((agent_id.to_string(), source.to_string()));
        Ok(())
    }

    fn report_failure(&self, agent_id: &str, error: &KickstartRunError) {
        self.failures
            .lock()
            .expect("failures")
            .push(format!("{agent_id}:{}", error.message));
    }

    fn report_disk_saver_audit_failure(&self, agent_id: &str, error: &KickstartRunError) {
        self.audit_failures
            .lock()
            .expect("audit failures")
            .push(format!("{agent_id}:{}", error.message));
    }

    fn push_introduction_failure(&self, agent_id: &str, error: &KickstartRunError) {
        self.trays
            .lock()
            .expect("trays")
            .push(format!("{agent_id}:{}", error.message));
    }

    fn emit_agent_update(&self, agent_id: &str) -> Result<(), String> {
        self.roster_updates
            .lock()
            .expect("roster")
            .push(agent_id.to_string());
        Ok(())
    }
}

fn successful(sent_message_count: usize, reacted: bool) -> KickstartTurnOutcome {
    KickstartTurnOutcome {
        aborted: false,
        quiesced_for_upgrade: false,
        sent_message_count,
        reacted,
    }
}

#[test]
fn create_agent_only_schedules_kickstart_for_foreground_requested_creation() {
    let root = temp_root("create");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let scheduled = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&scheduled);
    let hook: AgentKickstartHook = Arc::new(move |agent_id| {
        sink.lock().expect("scheduled").push(agent_id.to_string());
    });

    let created = dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes(
        &production,
        &AgentDeletionRuntimeDeps::default(),
        None,
        Some(Arc::clone(&hook)),
        "createAgent",
        &json!({
            "name":"Kickstart",
            "description":"",
            "isKickstartRequested":true
        }),
    )
    .expect("handled")
    .expect("created");
    let foreground_id = created["agent"]["id"].as_str().expect("id").to_string();
    assert_eq!(
        *scheduled.lock().expect("scheduled"),
        vec![foreground_id.clone()]
    );
    assert!(
        production
            .get_agent_introduction_pending(&foreground_id)
            .expect("pending")
    );

    dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes(
        &production,
        &AgentDeletionRuntimeDeps::default(),
        None,
        Some(hook),
        "createBackgroundAgent",
        &json!({
            "name":"Background",
            "description":"",
            "isKickstartRequested":true
        }),
    )
    .expect("handled")
    .expect("created background");
    assert_eq!(scheduled.lock().expect("scheduled").len(), 1);

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn disk_saver_kickstart_runs_hidden_on_user_lane_and_settles_introduction() {
    let root = temp_root("disk-saver");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", Some("disk-saver"))
        .expect("agent");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    runtime
        .outcomes
        .lock()
        .expect("outcomes")
        .push_back(Ok(successful(1, false)));

    assert!(run_created_agent_kickstart(&production, &runtime, &record.id).expect("kickstart"));
    assert!(!production
        .get_agent_introduction_pending(&record.id)
        .expect("pending settled"));
    let calls = runtime.calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, SAND_DISK_SAVER_KICKSTART_PROMPT);
    assert_eq!(calls[0].2, "kickstart");
    assert_eq!(
        *runtime.roster_updates.lock().expect("roster"),
        vec![record.id.clone()]
    );

    let (lane, source) =
        classify_send_dispatch(&json!({"requestSource":"kickstart"})).expect("dispatch");
    assert_eq!(lane, RunLane::User);
    assert_eq!(source, "kickstart");

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn onboarding_kickstart_retries_hidden_delivery_and_accepts_reaction_on_retry() {
    let root = temp_root("nudge");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    {
        let mut outcomes = runtime.outcomes.lock().expect("outcomes");
        outcomes.push_back(Ok(successful(0, true)));
        outcomes.push_back(Ok(successful(0, true)));
    }

    assert!(run_created_agent_kickstart(&production, &runtime, &record.id).expect("kickstart"));
    assert!(!production
        .get_agent_introduction_pending(&record.id)
        .expect("pending settled"));
    let calls = runtime.calls.lock().expect("calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, SAND_ONBOARDING_KICKSTART_PROMPT);
    assert_eq!(calls[1].1, REPLY_NUDGE_PROMPT);

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn quiesced_kickstart_marks_turn_resume_and_clears_introduction_pending() {
    let root = temp_root("quiesce");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    runtime.outcomes.lock().expect("outcomes").push_back(Ok(
        KickstartTurnOutcome {
            aborted: true,
            quiesced_for_upgrade: true,
            sent_message_count: 0,
            reacted: false,
        },
    ));

    assert!(run_created_agent_kickstart(&production, &runtime, &record.id).expect("kickstart"));
    assert!(!production
        .get_agent_introduction_pending(&record.id)
        .expect("pending settled"));
    assert_eq!(
        *runtime.resume.lock().expect("resume"),
        vec![(record.id.clone(), "turn".to_string())]
    );

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_kickstart_reports_error_and_tray_without_clearing_introduction() {
    let root = temp_root("failure");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    runtime
        .outcomes
        .lock()
        .expect("outcomes")
        .push_back(Err(KickstartRunError {
            request_id: Some("kickstart-request".into()),
            message: "provider failed".into(),
        }));

    assert!(run_created_agent_kickstart(&production, &runtime, &record.id).expect("kickstart"));
    assert!(production
        .get_agent_introduction_pending(&record.id)
        .expect("pending preserved"));
    assert_eq!(runtime.failures.lock().expect("failures").len(), 1);
    assert_eq!(runtime.trays.lock().expect("trays").len(), 1);

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn user_message_cancels_pending_introduction_without_starting_runner() {
    let root = temp_root("user-message");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store.create_session(None, "user", None).expect("agent");
    production
        .append_agent_transcript_entries(
            &record.id,
            &[json!({"id":"user-message-1","kind":"message","role":"user","content":"hello"})],
        )
        .expect("transcript");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };

    assert!(!run_created_agent_kickstart(&production, &runtime, &record.id).expect("skip"));
    assert!(!production
        .get_agent_introduction_pending(&record.id)
        .expect("pending cleared"));
    assert!(runtime.calls.lock().expect("calls").is_empty());

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_host_wires_real_kickstart_adapter_and_upgrade_quiesce_override() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main");
    let manager = fs::read_to_string(
        manifest_dir.join("src/extensions/transcript/transcript_manager.rs"),
    )
    .expect("transcript manager");
    assert!(main.contains("ProductionCreatedAgentKickstartRuntime"));
    assert!(main.contains("run_local_kickstart_turn"));
    assert!(main.contains(".dispatch_agent_lifecycle_gateway_call("));
    assert!(main.contains("Some(self.created_agent_kickstart_hook())"));
    assert!(manager.contains("dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes"));
    assert!(manager.contains("kickstart_created_agent"));
    assert!(main.contains("active_turn_source(agent_id).as_deref()"));
    assert!(main.contains("source: Some(\"turn\".into())"));
    assert!(main.contains("method == \"kickstartAgent\" || method == \"requestDiskSaverAudit\""));
    assert!(main.contains("request_disk_saver_audit("));
    assert!(main.contains("source: \"disk_saver_reaudit\".into()"));
}


#[test]
fn disk_saver_reaudit_runs_on_background_event_lane_and_nudges_empty_delivery() {
    let root = temp_root("reaudit");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let record = store
        .create_session(None, "user", Some("disk-saver"))
        .expect("agent");
    production
        .set_agent_introduction_pending(&record.id, false)
        .expect("settle introduction");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    {
        let mut outcomes = runtime.outcomes.lock().expect("outcomes");
        outcomes.push_back(Ok(successful(0, false)));
        outcomes.push_back(Ok(successful(1, false)));
    }

    assert!(request_disk_saver_audit(&production, &runtime, &record.id).expect("reaudit"));
    let calls = runtime.calls.lock().expect("calls");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, SAND_DISK_SAVER_REAUDIT_PROMPT);
    assert_eq!(calls[0].2, "event");
    assert_eq!(calls[1].1, REPLY_NUDGE_PROMPT);
    assert_eq!(calls[1].2, "event");
    assert!(runtime.trays.lock().expect("trays").is_empty());
    assert_eq!(
        *runtime.roster_updates.lock().expect("roster"),
        vec![record.id.clone()]
    );
    let (lane, source) =
        classify_send_dispatch(&json!({"requestSource":"event"})).expect("event dispatch");
    assert_eq!(lane, RunLane::Background);
    assert_eq!(source, "event");

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn disk_saver_reaudit_rejects_other_purposes_and_reports_failure_without_tray() {
    let root = temp_root("reaudit-failure");
    let production = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let store = SandAgentSessionStore::new(Arc::clone(&production));
    let normal = store.create_session(None, "user", None).expect("normal");
    production
        .set_agent_introduction_pending(&normal.id, false)
        .expect("settle normal");
    let runtime = FakeKickstartRuntime {
        ready: true,
        can_execute: true,
        ..FakeKickstartRuntime::default()
    };
    assert!(!request_disk_saver_audit(&production, &runtime, &normal.id).expect("skip normal"));

    let disk = store
        .create_session(None, "user", Some("disk-saver"))
        .expect("disk saver");
    production
        .set_agent_introduction_pending(&disk.id, false)
        .expect("settle disk saver");
    runtime
        .outcomes
        .lock()
        .expect("outcomes")
        .push_back(Err(KickstartRunError {
            request_id: Some("reaudit-request".into()),
            message: "reaudit failed".into(),
        }));
    assert!(request_disk_saver_audit(&production, &runtime, &disk.id).expect("handled failure"));
    assert_eq!(runtime.audit_failures.lock().expect("audit failures").len(), 1);
    assert!(runtime.trays.lock().expect("trays").is_empty());

    production.shutdown();
    let _ = fs::remove_dir_all(root);
}
