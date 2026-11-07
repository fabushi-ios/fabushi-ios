use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::session_runtime::SessionRuntime;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-transcript-session-runtime-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn focus_and_switch_follow_frozen_active_session_viewed_semantics() {
    let root = temp_root("switch");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let first = store.create_session(None, "user", None).expect("first");
    let second = store.create_session(None, "user", None).expect("second");
    store
        .write_active_agent_id(&first.id)
        .expect("seed active pointer");
    sessions
        .append_agent_transcript_entries(
            &second.id,
            &[serde_json::json!({"id":"second-1","kind":"message","role":"user","content":"hello"})],
        )
        .expect("second transcript");

    sessions
        .mark_agent_activity(&first.id, 100.0)
        .expect("first activity");
    let runtime = SessionRuntime::new();
    assert_eq!(runtime.window_focused_at_ms(), None);
    assert!(runtime
        .set_window_focused(&sessions, true, 200.0)
        .expect("focus"));
    assert_eq!(runtime.window_focused_at_ms(), Some(200.0));

    let focused_first = sessions
        .open_agent_db_owner(&first.id)
        .expect("first owner")
        .get_unread_state()
        .expect("first unread");
    assert_eq!(focused_first.last_viewed_at, 200.0);

    sessions
        .mark_agent_activity(&first.id, 300.0)
        .expect("new first activity");
    sessions
        .set_agent_unread(&second.id, true, 250.0)
        .expect("manual second unread");

    let entries = runtime
        .switch_agent(&sessions, &second.id, 400.0)
        .expect("switch");
    assert!(runtime.is_live_session(&second.id));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], "second-1");
    assert_eq!(runtime.get_entries(), entries);
    runtime.append_entry(serde_json::json!({"id":"local","kind":"notice","text":"local"}));
    assert_eq!(runtime.get_entries().len(), 2);
    assert!(runtime.remove_entry("local"));
    assert_eq!(runtime.get_entries().len(), 1);
    assert_eq!(
        store.read_active_agent_id().as_deref(),
        Some(second.id.as_str())
    );

    let left_behind = sessions
        .open_agent_db_owner(&first.id)
        .expect("first owner")
        .get_unread_state()
        .expect("first unread");
    assert_eq!(left_behind.last_viewed_at, 400.0);

    let activated = sessions
        .open_agent_db_owner(&second.id)
        .expect("second owner")
        .get_unread_state()
        .expect("second unread");
    assert_eq!(activated.last_viewed_at, 400.0);
    assert!(!activated.is_manually_unread);

    assert!(!runtime
        .set_window_focused(&sessions, false, 500.0)
        .expect("blur"));
    assert_eq!(runtime.window_focused_at_ms(), None);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn opening_the_already_active_agent_does_not_rewrite_viewed_state() {
    let root = temp_root("same-agent");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let agent = store.create_session(None, "user", None).expect("agent");
    store
        .write_active_agent_id(&agent.id)
        .expect("active pointer");
    sessions
        .mark_agent_activity(&agent.id, 100.0)
        .expect("activity");
    sessions
        .append_agent_transcript_entries(
            &agent.id,
            &[serde_json::json!({"id":"same-1","kind":"message","role":"user","content":"seed"})],
        )
        .expect("same transcript");

    let runtime = SessionRuntime::new();
    let first_snapshot = runtime
        .switch_agent(&sessions, &agent.id, 900.0)
        .expect("same active");
    assert_eq!(first_snapshot.len(), 1);
    assert_eq!(runtime.get_entries(), first_snapshot);
    runtime.append_entry(serde_json::json!({"id":"memory-only","kind":"notice","text":"cached"}));
    let second_snapshot = runtime
        .switch_agent(&sessions, &agent.id, 901.0)
        .expect("same active cached");
    assert_eq!(second_snapshot.len(), 2);
    assert_eq!(second_snapshot[1]["id"], "memory-only");
    let unread = sessions
        .open_agent_db_owner(&agent.id)
        .expect("owner")
        .get_unread_state()
        .expect("unread");
    assert_eq!(unread.last_viewed_at, 0.0);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_open_session_once_reuses_real_db_and_agent_store_owners_and_fences_deleted_agents() {
    let root = temp_root("open-once");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let agent = store.create_session(None, "user", None).expect("agent");

    // Reset creation-time owners so this contract observes SessionRuntime's
    // shipping open ownership rather than materialization side effects.
    sessions.close_agent_store_owner(&agent.id, true);
    sessions.close_agent_db_owner(&agent.id, true);
    assert_eq!(sessions.active_agent_store_owner_count(), 0);
    assert_eq!(sessions.active_agent_db_owner_count(), 0);

    let runtime = SessionRuntime::new();
    runtime
        .resolve_background_session(&sessions, &agent.id)
        .expect("first open");
    assert!(runtime.is_live_session(&agent.id));
    assert_eq!(runtime.live_session_count(), 1);
    assert_eq!(sessions.active_agent_store_owner_count(), 1);
    assert_eq!(sessions.active_agent_db_owner_count(), 1);

    runtime
        .resolve_background_session(&sessions, &agent.id)
        .expect("deduped second open");
    assert_eq!(runtime.live_session_count(), 1);
    assert_eq!(sessions.active_agent_store_owner_count(), 1);
    assert_eq!(sessions.active_agent_db_owner_count(), 1);

    runtime.mark_agent_deleted(&agent.id);
    assert!(!runtime.is_live_session(&agent.id));
    assert!(
        runtime
            .resolve_background_session(&sessions, &agent.id)
            .expect_err("deleted agent must be fenced")
            .contains("no longer exists")
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_switch_retires_only_an_inactive_idle_live_session() {
    let root = temp_root("retire-idle");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let first = store.create_session(None, "user", None).expect("first");
    let second = store.create_session(None, "user", None).expect("second");
    sessions.close_agent_store_owner(&first.id, true);
    sessions.close_agent_db_owner(&first.id, true);
    sessions.close_agent_store_owner(&second.id, true);
    sessions.close_agent_db_owner(&second.id, true);

    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    runtime
        .session_runtime()
        .resolve_background_session(&sessions, &first.id)
        .expect("open first live session");
    store.write_active_agent_id(&first.id).expect("active first");
    assert!(runtime.session_runtime().is_live_session(&first.id));

    runtime
        .switch_agent(&sessions, &second.id, 500.0)
        .expect("switch to second");
    assert!(!runtime.session_runtime().is_live_session(&first.id));
    assert!(runtime.session_runtime().is_live_session(&second.id));
    assert_eq!(
        store.read_active_agent_id().as_deref(),
        Some(second.id.as_str())
    );

    // The retired first owner is gone while the active second owner stays live.
    assert_eq!(sessions.active_agent_store_owner_count(), 1);
    assert_eq!(sessions.active_agent_db_owner_count(), 1);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn provider_terminal_retires_session_that_was_running_when_user_switched_away() {
    let root = temp_root("retire-after-provider");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let active = store.create_session(None, "user", None).expect("active");
    let background = store.create_session(None, "user", None).expect("background");

    for id in [&active.id, &background.id] {
        sessions.close_agent_store_owner(id, true);
        sessions.close_agent_db_owner(id, true);
    }
    store.write_active_agent_id(&active.id).expect("active pointer");

    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    runtime
        .session_runtime()
        .resolve_background_session(&sessions, &background.id)
        .expect("open background live session");
    assert!(runtime.session_runtime().is_live_session(&background.id));
    assert_eq!(sessions.active_agent_store_owner_count(), 1);
    assert_eq!(sessions.active_agent_db_owner_count(), 1);

    runtime.begin_provider_run(&background.id);
    assert!(!runtime
        .retire_idle_live_session(&sessions, &background.id)
        .expect("running session retained"));
    assert!(runtime.session_runtime().is_live_session(&background.id));

    runtime.end_provider_run(&background.id);
    assert!(runtime
        .retire_idle_live_session(&sessions, &background.id)
        .expect("terminal retirement"));
    assert!(!runtime.session_runtime().is_live_session(&background.id));
    assert_eq!(sessions.active_agent_store_owner_count(), 0);
    assert_eq!(sessions.active_agent_db_owner_count(), 0);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn provider_terminal_retirement_is_safe_inside_existing_local_executor() {
    let root = temp_root("retire-inside-local-executor");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let active = store.create_session(None, "user", None).expect("active");
    let background = store.create_session(None, "user", None).expect("background");

    for id in [&active.id, &background.id] {
        sessions.close_agent_store_owner(id, true);
        sessions.close_agent_db_owner(id, true);
    }
    store.write_active_agent_id(&active.id).expect("active pointer");

    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    runtime
        .session_runtime()
        .resolve_background_session(&sessions, &background.id)
        .expect("open background live session");
    runtime.begin_provider_run(&background.id);
    runtime.end_provider_run(&background.id);

    let retired = futures::executor::block_on(async {
        runtime.retire_idle_live_session(&sessions, &background.id)
    });
    assert_eq!(retired, Ok(true));
    assert!(!runtime.session_runtime().is_live_session(&background.id));

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn desktop_contact_refreshes_focus_freshness_only_while_focused() {
    let root = temp_root("desktop-contact");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let agent = store.create_session(None, "user", None).expect("agent");
    store.write_active_agent_id(&agent.id).expect("active");

    let runtime = SessionRuntime::new();
    runtime.note_desktop_contact(100.0);
    assert_eq!(runtime.window_focused_at_ms(), None);

    runtime
        .set_window_focused(&sessions, true, 200.0)
        .expect("focus");
    runtime.note_desktop_contact(350.0);
    assert_eq!(runtime.window_focused_at_ms(), Some(350.0));

    runtime
        .set_window_focused(&sessions, false, 400.0)
        .expect("blur");
    runtime.note_desktop_contact(500.0);
    assert_eq!(runtime.window_focused_at_ms(), None);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn deferred_window_activation_is_supersedable_and_catches_up_after_shipped_entry() {
    let runtime = SessionRuntime::new();
    let first = runtime.schedule_deferred_activation("agent-a", Some("entry-1"));
    let second = runtime.schedule_deferred_activation("agent-b", Some("entry-2"));

    assert_eq!(runtime.pending_activation_agent_id().as_deref(), Some("agent-b"));
    assert!(runtime.claim_deferred_activation(first, "agent-a").is_none());

    let claim = runtime
        .claim_deferred_activation(second, "agent-b")
        .expect("latest activation claim");
    assert_eq!(claim.shipped_through_id.as_deref(), Some("entry-2"));
    assert_eq!(runtime.pending_activation_agent_id(), None);

    let entries = vec![
        serde_json::json!({"id":"entry-1","kind":"message"}),
        serde_json::json!({"id":"entry-2","kind":"message"}),
        serde_json::json!({"id":"entry-3","kind":"message"}),
        serde_json::json!({"id":"entry-4","kind":"message"}),
    ];
    let catch_up = runtime.windowed_catch_up(Some("entry-2"), &entries);
    assert_eq!(
        catch_up
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-3", "entry-4"]
    );
    assert!(runtime.windowed_catch_up(Some("missing"), &entries).is_empty());
    assert_eq!(runtime.windowed_catch_up(None, &entries), entries);
}

#[test]
fn explicit_agent_switch_cancels_pending_window_activation() {
    let root = temp_root("activation-cancel");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(root.join("agents"), 500));
    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let first = store.create_session(None, "user", None).expect("first");
    let second = store.create_session(None, "user", None).expect("second");
    store.write_active_agent_id(&first.id).expect("active");

    let runtime = SessionRuntime::new();
    let generation = runtime.schedule_deferred_activation(&second.id, None);
    runtime
        .switch_agent(&sessions, &first.id, 600.0)
        .expect("explicit switch");
    assert!(
        runtime
            .claim_deferred_activation(generation, &second.id)
            .is_none()
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
