use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::session::agent_db_serde::AwaitingUserResponse;
use mahayana_host_runtime::extensions::session::conversation_size_limits::ConversationSizePolicy;
use sha2::{Digest, Sha256};
use mahayana_host_runtime::extensions::session::session_paths::{
    CONVERSATION_BLOBS_FILENAME, STORE_FILENAME,
};

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_len(field: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-production-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn production_session_workers_bind_worker_blob_store_to_frozen_session_paths() {
    let root = temp_root("paths");
    let runtime = ProductionSessionWorkers::with_agents_root(&root, 500);

    let store = runtime
        .create_agent_blob_store("agent-a")
        .expect("production blob store");
    assert_eq!(
        store.blob_db_path,
        root.join("agent-a").join(CONVERSATION_BLOBS_FILENAME)
    );
    assert_eq!(
        store.legacy_blob_db_path,
        Some(root.join("agent-a").join(STORE_FILENAME))
    );
    assert_eq!(runtime.active_worker_count(), 0);
    assert!(runtime.create_agent_blob_store("../escape").is_err());

    runtime.shutdown();
}

#[test]
fn production_session_workers_use_real_sqlite_backend_and_close_on_host_shutdown() {
    let root = temp_root("sqlite");
    let agent_dir = root.join("agent-live");
    fs::create_dir_all(&agent_dir).expect("agent directory");
    let session_db_path = agent_dir.join(STORE_FILENAME);
    let connection = rusqlite::Connection::open(&session_db_path).expect("session sqlite");
    connection
        .execute_batch(
            "CREATE TABLE kv (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) STRICT;
            CREATE TABLE transcript_entries (
                seq INTEGER PRIMARY KEY,
                id TEXT NOT NULL UNIQUE,
                entry TEXT NOT NULL
            ) STRICT;",
        )
        .expect("session kv");
    let persisted_root = vec![0xaa, 0xbb, 0xcc];
    let metadata = serde_json::to_vec(&serde_json::json!({
        "agentId": "agent-live",
        "latestRootBlobId": to_hex(&persisted_root),
        "name": "Agent Live",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": 1
    }))
    .expect("metadata json");
    connection
        .execute(
            "INSERT INTO kv (key, value) VALUES ('metadata', ?1)",
            rusqlite::params![to_hex(&metadata)],
        )
        .expect("metadata row");
    for (key, value) in [
        (
            "sandProfile",
            r#"{"description":"Shipping profile","avatarPath":"/tmp/avatar.png"}"#,
        ),
        (
            "unreadState",
            r#"{"lastActivityAt":123,"lastViewedAt":"bad","isManuallyUnread":true,"unreadCount":2.9}"#,
        ),
        (
            "automationSpendGuardState",
            r#"{"nudgedAtMs":42,"snoozedUntilMs":-1,"optedOut":true,"cardEntryIds":["card","",2],"pausedAutomationIds":["auto"]}"#,
        ),
        (
            "awaitingUserResponse",
            r#"{"tabId":"tab-1","reason":7,"since":44}"#,
        ),
        (
            "requestIds",
            r#"[{"id":" request-1 ","at":9,"prompt":"hello","source":"turn"},{"id":"   ","at":3}]"#,
        ),
        (
            "episodePending",
            r#"[{"ts":5,"user":"question","agent":"answer"},{"ts":7,"user":"","agent":""}]"#,
        ),
        (
            "memoryPromptSnapshot",
            r#"{"render":"memory","compactionEpoch":3}"#,
        ),
    ] {
        connection
            .execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .expect("session state row");
    }
    for (id, entry) in [
        (
            "entry-1",
            serde_json::json!({"id":"entry-1","kind":"message","role":"user","content":"first"}),
        ),
        (
            "entry-2",
            serde_json::json!({"id":"entry-2","kind":"notice","text":"second"}),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
                rusqlite::params![id, entry.to_string()],
            )
            .expect("transcript row");
    }
    drop(connection);

    let runtime = ProductionSessionWorkers::with_agents_root(&root, 500);
    let store = runtime
        .create_agent_blob_store("agent-live")
        .expect("worker blob store");
    let blob_id = b"production-worker-blob";
    futures::executor::block_on(store.set_blob(&(), blob_id, b"hello"))
        .expect("write through worker pool");
    let read = futures::executor::block_on(store.get_blob(&(), blob_id))
        .expect("read through worker pool");
    assert_eq!(read.as_deref(), Some(b"hello".as_slice()));
    assert_eq!(runtime.active_worker_count(), 1);

    let prepared = runtime
        .prepare_existing_agent("agent-live")
        .expect("prepare existing agent")
        .expect("existing agent session");
    assert_eq!(prepared.session_db_path, session_db_path);
    assert_eq!(
        prepared.blob_db_path,
        agent_dir.join(CONVERSATION_BLOBS_FILENAME)
    );
    assert_eq!(prepared.persisted_root_blob_id, vec![0xaa, 0xbb, 0xcc]);
    assert_eq!(prepared.session_state.profile.description, "Shipping profile");
    assert_eq!(
        prepared.session_state.profile.avatar_path.as_deref(),
        Some("/tmp/avatar.png")
    );
    assert_eq!(prepared.session_state.unread_state.last_activity_at, 123.0);
    assert_eq!(prepared.session_state.unread_state.last_viewed_at, 0.0);
    assert!(prepared.session_state.unread_state.is_manually_unread);
    assert_eq!(prepared.session_state.unread_state.unread_count, 2.0);
    assert_eq!(prepared.session_state.spend_guard_state.nudged_at_ms, Some(42.0));
    assert_eq!(prepared.session_state.spend_guard_state.snoozed_until_ms, None);
    assert!(prepared.session_state.spend_guard_state.opted_out);
    assert_eq!(
        prepared.session_state.spend_guard_state.card_entry_ids,
        vec!["card".to_string()]
    );
    assert_eq!(
        prepared.session_state.awaiting_user_response.as_ref().map(|state| state.tab_id.as_str()),
        Some("tab-1")
    );
    assert_eq!(prepared.session_state.request_ids.len(), 1);
    assert_eq!(prepared.session_state.request_ids[0].id, "request-1");
    assert_eq!(prepared.session_state.request_ids[0].source.as_deref(), Some("turn"));
    assert_eq!(prepared.session_state.pending_episode_turns.len(), 1);
    assert_eq!(prepared.session_state.pending_episode_turns[0].agent, "answer");
    assert_eq!(
        prepared.session_state.memory_prompt_snapshot.as_ref().map(|snapshot| snapshot.render.as_str()),
        Some("memory")
    );
    assert_eq!(
        prepared
            .transcript_tail
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["entry-1", "entry-2"]
    );
    assert_eq!(prepared.transcript_tail.next_before_seq, None);
    assert_eq!(
        prepared.profile_file.as_ref().map(|profile| profile.name.as_str()),
        Some("Agent Live")
    );
    assert_eq!(
        prepared
            .profile_file
            .as_ref()
            .map(|profile| profile.description.as_str()),
        Some("Shipping profile")
    );
    let profile_text = fs::read_to_string(agent_dir.join("profile.json"))
        .expect("materialized profile file");
    assert!(profile_text.ends_with('\n'));
    assert_eq!(
        fs::read_to_string(agent_dir.join("settings.json")).expect("materialized settings file"),
        "{}\n"
    );
    assert_eq!(runtime.active_worker_count(), 1);

    runtime.shutdown();
    assert_eq!(runtime.active_worker_count(), 0);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_session_open_owns_frozen_soft_gc_schedule_wiring() {
    let root = temp_root("soft-gc");
    let runtime = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("materialized agent");
    let store = runtime
        .create_agent_blob_store(&record.id)
        .expect("blob store");

    let reachable = vec![0x41; 256];
    let reachable_id = Sha256::digest(&reachable).to_vec();
    let orphan = vec![0x52; 512];
    let orphan_id = Sha256::digest(&orphan).to_vec();
    futures::executor::block_on(store.set_blob(&(), &reachable_id, &reachable))
        .expect("reachable blob");

    // Model an orphan left by an older Host generation rather than a fresh
    // in-flight write. Frozen Grok keeps fresh blob writes protected for
    // GC_PENDING_WRITE_RETENTION_MS, so writing this through WorkerBlobStore
    // would correctly retain it and make this soft-GC wiring test invalid.
    let orphan_db = rusqlite::Connection::open(&store.blob_db_path)
        .expect("open conversation blob db for historical orphan");
    orphan_db
        .execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
            rusqlite::params![to_hex(&orphan_id), &orphan],
        )
        .expect("historical orphan blob");
    drop(orphan_db);

    let mut root_blob = Vec::new();
    push_len(1, &reachable_id, &mut root_blob);
    let root_id = Sha256::digest(&root_blob).to_vec();
    futures::executor::block_on(store.set_blob(&(), &root_id, &root_blob))
        .expect("root blob");
    let db = runtime.open_agent_db_owner(&record.id).expect("db owner");
    assert!(db
        .compare_and_set_latest_root_blob_id(&[], &root_id)
        .expect("persist root"));

    let prepared = runtime
        .prepare_existing_agent(&record.id)
        .expect("prepare")
        .expect("prepared");
    assert_eq!(prepared.persisted_root_blob_id, root_id);

    assert!(runtime.schedule_conversation_size_maintenance(
        &prepared,
        ConversationSizePolicy {
            enabled: true,
            soft_limit_bytes: 1,
            hard_limit_bytes: 64 * 1024 * 1024,
        },
    ));

    let mut orphan_collected = false;
    for _ in 0..200 {
        if futures::executor::block_on(store.get_blob(&(), &orphan_id))
            .expect("orphan read")
            .is_none()
        {
            orphan_collected = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(orphan_collected, "scheduled production soft GC did not collect orphan");
    assert_eq!(
        futures::executor::block_on(store.get_blob(&(), &root_id)).expect("root read"),
        Some(root_blob)
    );

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_session_exposes_durable_awaiting_user_state_for_runner_send_guard() {
    let root = temp_root("awaiting-send-guard");
    let runtime = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = runtime
        .materialize_new_session(None, "user", None)
        .expect("materialize session");
    assert!(
        runtime
            .get_agent_awaiting_user_response(&record.id)
            .expect("read initial awaiting")
            .is_none()
    );

    let awaiting = AwaitingUserResponse {
        tab_id: "tab-1".into(),
        reason: "widget".into(),
        since: 123.0,
    };
    assert!(
        runtime
            .set_agent_awaiting_user_response(&record.id, Some(&awaiting))
            .expect("set awaiting")
    );
    assert_eq!(
        runtime
            .get_agent_awaiting_user_response(&record.id)
            .expect("read awaiting"),
        Some(awaiting)
    );

    assert!(
        runtime
            .set_agent_awaiting_user_response(&record.id, None)
            .expect("clear awaiting")
    );
    assert!(
        runtime
            .get_agent_awaiting_user_response(&record.id)
            .expect("read cleared awaiting")
            .is_none()
    );

    runtime.shutdown();
    let _ = fs::remove_dir_all(root);
}
