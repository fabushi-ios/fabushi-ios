use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agent_isolation::{
    AgentWorkerPool, ConversationBlobWorkerBackend, WorkerBlobStore,
};
use mahayana_host_runtime::extensions::session::agent_db::{
    hidden_entry_repair_version, read_persisted_latest_root_blob_id, SandAgentDb,
    read_persisted_transcript_entries, stale_root_cleanup_version,
};
use mahayana_host_runtime::extensions::session::agent_db_schema::AGENT_DB_SCHEMA;
use mahayana_host_runtime::extensions::session::conversation_recovery::OutlineItem;
use mahayana_host_runtime::extensions::session::session_diagnostics::{
    pin_session_diagnostics_reporter, SessionDiagnostic,
};
use mahayana_host_runtime::extensions::session::session_maintenance::{
    HIDDEN_ENTRY_REPAIR_VERSION, STALE_ROOT_CLEANUP_VERSION, backfill_transcript,
    backfill_transcript_from_outline, cleanup_legacy_group_member_dirs,
    clear_stale_checkpoint_roots_once, pin_stale_root_gc,
    recover_conversation_root_if_missing, repair_hidden_transcript_entries_once,
    run_session_maintenance, sync_recovered_profile_name,
};
use rusqlite::params;
use sha2::{Digest, Sha256};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-maintenance-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_length_delimited(field: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((field << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn root_bytes(turn: &[u8], todo: &[u8], summary: &[u8], prompt: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    push_length_delimited(1, prompt, &mut output);
    push_length_delimited(3, todo, &mut output);
    push_length_delimited(6, summary, &mut output);
    push_length_delimited(8, turn, &mut output);
    output
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn create_store_db(path: &std::path::Path, agent_id: &str) {
    let db = rusqlite::Connection::open(path).expect("store db");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
    let metadata = serde_json::to_vec(&serde_json::json!({
        "agentId": agent_id,
        "latestRootBlobId": "",
        "name": "Agent",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": 1
    }))
    .expect("metadata");
    db.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', ?1)",
        params![to_hex(&metadata)],
    )
    .expect("metadata row");
}

#[test]
fn production_missing_root_recovery_requires_all_structural_refs_and_cas_persists_root() {
    let root = temp_root("recover");
    let agent_dir = root.join("agent-a");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let blob_path = agent_dir.join("conversation-blobs.db");
    create_store_db(&db_path, "agent-a");

    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(
        Arc::clone(&pool),
        "agent-a",
        &blob_path,
        Some(db_path.clone()),
    );
    let turn = vec![0x11; 32];
    let todo = vec![0x22; 32];
    let summary = vec![0x33; 32];
    futures::executor::block_on(store.set_blob(&(), &turn, b"turn")).expect("turn");
    futures::executor::block_on(store.set_blob(&(), &todo, b"todo")).expect("todo");
    futures::executor::block_on(store.set_blob(&(), &summary, b"summary")).expect("summary");
    let root_blob = root_bytes(&turn, &todo, &summary, b"prompt");
    let root_id = Sha256::digest(&root_blob).to_vec();
    futures::executor::block_on(store.set_blob(&(), &root_id, &root_blob)).expect("root");

    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    assert!(recover_conversation_root_if_missing(
        Arc::clone(&pool),
        "agent-a",
        &db_path,
        &blob_path,
        500,
        &owner,
    )
    .expect("recovery"));
    assert_eq!(
        read_persisted_latest_root_blob_id(&db_path, 500).expect("persisted root"),
        root_id
    );
    assert!(!recover_conversation_root_if_missing(
        Arc::clone(&pool),
        "agent-a",
        &db_path,
        &blob_path,
        500,
        &owner,
    )
    .expect("second recovery"));

    futures::executor::block_on(pool.close_all());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_root_cleanup_is_feature_gated_and_records_one_time_version() {
    let root = temp_root("stale");
    let agent_dir = root.join("agent-b");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let blob_path = agent_dir.join("conversation-blobs.db");
    create_store_db(&db_path, "agent-b");

    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(
        Arc::clone(&pool),
        "agent-b",
        &blob_path,
        Some(db_path.clone()),
    );
    let turn = vec![0x44; 32];
    futures::executor::block_on(store.set_blob(&(), &turn, b"turn")).expect("turn");
    let retained = root_bytes(&turn, &[], &[], b"retained");
    let retained_id = Sha256::digest(&retained).to_vec();
    let stale = root_bytes(&turn, &[], &[], b"stale");
    let stale_id = Sha256::digest(&stale).to_vec();
    futures::executor::block_on(store.set_blob(&(), &retained_id, &retained)).expect("retained");
    futures::executor::block_on(store.set_blob(&(), &stale_id, &stale)).expect("stale");

    use mahayana_host_runtime::extensions::session::agent_db::compare_and_set_persisted_latest_root_blob_id;
    assert!(compare_and_set_persisted_latest_root_blob_id(
        &db_path, 500, &[], &retained_id,
    ).expect("seed root"));

    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    pin_stale_root_gc(false);
    assert!(!clear_stale_checkpoint_roots_once(
        Arc::clone(&pool), "agent-b", &db_path, &blob_path, 500, &owner,
    ).expect("disabled cleanup"));

    pin_stale_root_gc(true);
    assert!(clear_stale_checkpoint_roots_once(
        Arc::clone(&pool), "agent-b", &db_path, &blob_path, 500, &owner,
    ).expect("enabled cleanup"));
    assert_eq!(
        stale_root_cleanup_version(&db_path, 500).expect("cleanup version"),
        STALE_ROOT_CLEANUP_VERSION
    );
    assert_eq!(
        futures::executor::block_on(store.get_blob(&(), &stale_id)).expect("stale read"),
        None
    );
    assert_eq!(
        futures::executor::block_on(store.get_blob(&(), &retained_id)).expect("retained read"),
        Some(retained)
    );
    pin_stale_root_gc(false);

    futures::executor::block_on(pool.close_all());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn transcript_backfill_only_appends_a_matching_recovered_tail() {
    let root = temp_root("backfill");
    let agent_dir = root.join("agent-c");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    create_store_db(&db_path, "agent-c");
    let db = rusqlite::Connection::open(&db_path).expect("db");
    db.execute(
        "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
        params![
            "one",
            serde_json::json!({"id":"one","kind":"message","role":"user","content":"one"}).to_string()
        ],
    )
    .expect("persisted transcript");
    drop(db);

    let rebuilt = vec![
        serde_json::json!({"id":"recovered-one","kind":"message","role":"user","content":"one"}),
        serde_json::json!({"id":"recovered-two","kind":"message","role":"user","content":"two"}),
    ];
    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    assert_eq!(backfill_transcript(&db_path, 500, &rebuilt, &owner).expect("backfill"), 1);
    assert_eq!(backfill_transcript(&db_path, 500, &rebuilt, &owner).expect("idempotent"), 0);

    let _ = fs::remove_dir_all(root);
}


#[test]
fn outline_backfill_and_hidden_repair_match_frozen_maintenance_contract() {
    let root = temp_root("outline-maintenance");
    let agent_dir = root.join("agent-d");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    create_store_db(&db_path, "agent-d");
    let db = rusqlite::Connection::open(&db_path).expect("db");
    db.execute(
        "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
        params![
            "recovered-outline-user-hidden",
            serde_json::json!({
                "id":"recovered-outline-user-hidden",
                "kind":"message",
                "role":"user",
                "content":"hidden"
            }).to_string()
        ],
    )
    .expect("hidden recovered row");
    drop(db);

    let turns = vec![vec![
        OutlineItem::User {
            id: "outline-user-hidden".into(),
            hidden: true,
            text: "hidden".into(),
            timestamp_ms: None,
        },
        OutlineItem::SendMessage {
            id: "outline-send".into(),
            message: serde_json::json!({"type":"text","content":"reply"}),
            timestamp_ms: None,
        },
        OutlineItem::ToolCall {
            id: "outline-tool".into(),
            name: "Task".into(),
            status: "done".into(),
            summary: Some("summary".into()),
            timestamp_ms: None,
        },
    ]];

    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    assert_eq!(
        backfill_transcript_from_outline(&db_path, 500, &turns, &owner).expect("outline backfill"),
        0,
        "a mismatched persisted prefix must not append a tail"
    );
    let outline = turns.iter().flatten().cloned().collect::<Vec<_>>();
    assert_eq!(
        repair_hidden_transcript_entries_once(&db_path, 500, &outline, &owner)
            .expect("hidden repair"),
        1
    );
    assert_eq!(
        hidden_entry_repair_version(&db_path, 500).expect("repair version"),
        HIDDEN_ENTRY_REPAIR_VERSION
    );
    assert!(
        read_persisted_transcript_entries(&db_path, 500)
            .expect("transcript after repair")
            .is_empty()
    );
    assert_eq!(
        repair_hidden_transcript_entries_once(&db_path, 500, &[], &owner)
            .expect("idempotent hidden repair"),
        0
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn outline_backfill_serializes_visible_user_send_and_tool_items() {
    let root = temp_root("outline-backfill");
    let agent_dir = root.join("agent-e");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    create_store_db(&db_path, "agent-e");

    let turns = vec![vec![
        OutlineItem::User {
            id: "outline-user-0".into(),
            hidden: false,
            text: "hello".into(),
            timestamp_ms: None,
        },
        OutlineItem::SendMessage {
            id: "outline-send".into(),
            message: serde_json::json!({"type":"text","content":"reply"}),
            timestamp_ms: None,
        },
        OutlineItem::ToolCall {
            id: "outline-tool".into(),
            name: "Task".into(),
            status: "done".into(),
            summary: Some("summary".into()),
            timestamp_ms: None,
        },
    ]];

    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    assert_eq!(
        backfill_transcript_from_outline(&db_path, 500, &turns, &owner).expect("backfill"),
        3
    );
    let entries = read_persisted_transcript_entries(&db_path, 500).expect("entries");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["id"], "recovered-outline-user-0");
    assert_eq!(entries[1]["kind"], "send-message");
    assert_eq!(entries[2]["name"], "Task");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn recovered_profile_name_sync_matches_frozen_recovery_contract() {
    let root = temp_root("profile-name-sync");
    let agent_dir = root.join("agent-profile");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    create_store_db(&db_path, "agent-profile");

    let owner = SandAgentDb::open(&db_path, 500).expect("owner");
    assert!(sync_recovered_profile_name(&db_path, 500, "  Profile Wins  ", &owner)
        .expect("sync profile name"));
    assert!(!sync_recovered_profile_name(&db_path, 500, "Profile Wins", &owner)
        .expect("idempotent sync"));
    assert!(!sync_recovered_profile_name(&db_path, 500, "   ", &owner)
        .expect("blank profile ignored"));

    use mahayana_host_runtime::extensions::session::agent_db::read_persisted_agent_name;
    assert_eq!(
        read_persisted_agent_name(&db_path, 500)
            .expect("read synced name")
            .as_deref(),
        Some("Profile Wins")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn run_session_maintenance_reports_failure_and_continues_in_order() {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let ran = Arc::new(AtomicUsize::new(0));
    let reports = Arc::new(Mutex::new(Vec::<(usize, String)>::new()));
    let ran_second = Arc::clone(&ran);
    let reports_for_callback = Arc::clone(&reports);

    run_session_maintenance(
        vec![
            Box::new(|| Err("first failed".to_string())),
            Box::new(move || {
                ran_second.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        ],
        move |error, index| {
            reports_for_callback
                .lock()
                .expect("reports")
                .push((index, error.to_string()));
        },
    );

    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert_eq!(
        *reports.lock().expect("reports"),
        vec![(0, "first failed".to_string())]
    );
}


#[test]
fn legacy_member_cleanup_reports_frozen_maintenance_diagnostic_shape() {
    use std::sync::Mutex;

    let root = temp_root("member-cleanup-diagnostic");
    let agent_dir = root.join("agent-diag");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    fs::write(agent_dir.join("members"), b"not-a-directory").expect("members file");

    let reports = Arc::new(Mutex::new(Vec::<SessionDiagnostic>::new()));
    let reports_for_callback = Arc::clone(&reports);
    pin_session_diagnostics_reporter(Some(Arc::new(move |report| {
        reports_for_callback
            .lock()
            .expect("diagnostic reports")
            .push(report.clone());
    })));

    let failures = cleanup_legacy_group_member_dirs(&root, "members");
    pin_session_diagnostics_reporter(None);

    assert_eq!(failures.len(), 1);
    let reports = reports.lock().expect("reports");
    let report = reports
        .iter()
        .find(|report| report.kind == "member_cleanup_failed")
        .expect("member cleanup diagnostic");
    assert_eq!(report.family, "maintenance");
    assert_eq!(
        report.metadata.get("agentId").and_then(serde_json::Value::as_str),
        Some("agent-diag")
    );
    assert_eq!(
        report.metadata.get("errorClass").and_then(serde_json::Value::as_str),
        Some("Error")
    );

    let _ = fs::remove_dir_all(root);
}
