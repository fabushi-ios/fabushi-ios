use std::cell::RefCell;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::session_runtime::SessionRuntime;
use mahayana_host_runtime::host_initial_transcript_load::{
    INITIAL_TRANSCRIPT_BUSY_ATTEMPTS, InitialTranscriptDegradedReason,
    ensure_initial_transcript_loaded, load_initial_transcript_resiliently_with_delay,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-initial-transcript-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn retries_sqlite_busy_with_frozen_exponential_delays_then_succeeds() {
    let attempts = RefCell::new(0usize);
    let delays = RefCell::new(Vec::new());
    let outcome = load_initial_transcript_resiliently_with_delay(
        || {
            let mut attempt = attempts.borrow_mut();
            *attempt += 1;
            if *attempt < INITIAL_TRANSCRIPT_BUSY_ATTEMPTS {
                Err("database is locked".to_string())
            } else {
                Ok(7)
            }
        },
        |delay| delays.borrow_mut().push(delay),
    )
    .expect("load outcome");

    assert_eq!(outcome.entry_count, 7);
    assert_eq!(outcome.degraded, None);
    assert_eq!(*attempts.borrow(), INITIAL_TRANSCRIPT_BUSY_ATTEMPTS);
    assert_eq!(&*delays.borrow(), &[100, 200, 400, 800]);
}

#[test]
fn exhausted_sqlite_busy_and_agent_cap_keep_host_alive() {
    let delays = RefCell::new(Vec::new());
    let busy = load_initial_transcript_resiliently_with_delay(
        || Err("database table is locked".to_string()),
        |delay| delays.borrow_mut().push(delay),
    )
    .expect("busy degrades");
    assert_eq!(busy.entry_count, 0);
    assert!(matches!(
        busy.degraded,
        Some(InitialTranscriptDegradedReason::SqliteBusy(_))
    ));
    assert_eq!(&*delays.borrow(), &[100, 200, 400, 800]);

    let cap = load_initial_transcript_resiliently_with_delay(
        || Err("Agent limit of 50 reached".to_string()),
        |_| panic!("agent cap must not retry"),
    )
    .expect("agent cap degrades");
    assert_eq!(cap.entry_count, 0);
    assert_eq!(
        cap.degraded,
        Some(InitialTranscriptDegradedReason::AgentLimit)
    );
}

#[test]
fn non_degraded_startup_failures_remain_fatal() {
    let result = load_initial_transcript_resiliently_with_delay(
        || Err("profile json is corrupt".to_string()),
        |_| panic!("fatal errors must not retry"),
    );
    assert_eq!(result, Err("profile json is corrupt".to_string()));
}

#[test]
fn production_loader_materializes_fallback_and_publishes_active_transcript_owner() {
    let root = temp_root("production");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let runtime = SessionRuntime::new();

    assert_eq!(
        ensure_initial_transcript_loaded(&sessions, &runtime).expect("initial load"),
        0
    );
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let active = store.read_active_agent_id().expect("active agent pointer");
    assert!(store.agent_exists(&active));
    assert_eq!(runtime.get_entries(), Vec::<serde_json::Value>::new());
    assert_eq!(store.list_agent_record_ids().expect("record ids"), vec![active]);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
