use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::session::agent_db_recovery::{
    DbRecoveryOptions, is_store_db_healthy, open_configured_db, salvage_store_db,
};
use mahayana_host_runtime::extensions::session::agent_db_schema::AGENT_DB_SCHEMA;
use mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations;
use rusqlite::{Connection, params};
use uuid::Uuid;

fn path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fabushi-{label}-{}.sqlite", Uuid::new_v4()))
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(format!("{}{}", path.display(), suffix));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        "{}.corrupt-",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn initialize(db: &Connection) {
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
}

#[test]
fn salvage_preserves_kv_blob_and_transcript_rows() {
    let source_path = path("agent-db-salvage-source");
    let target_path = path("agent-db-salvage-target");
    let source = Connection::open(&source_path).expect("source");
    initialize(&source);
    source.execute(
        "INSERT INTO kv(key,value) VALUES('metadata','abcd')",
        [],
    ).expect("kv");
    source.execute(
        "INSERT INTO blobs(id,data) VALUES('blob-a',?1)",
        params![b"blob-data".as_slice()],
    ).expect("blob");
    source.execute(
        "INSERT INTO transcript_entries(seq,id,entry) VALUES(7,'entry-a',?1)",
        params![r#"{"id":"entry-a","kind":"message","role":"user","content":"hi"}"#],
    ).expect("transcript");
    source.execute(
        "INSERT INTO conversation_topics(topic_id, title, icon, creator_id, created_at_ms)
         VALUES('topic-1', 'Topic 1', 'icon-url', 'creator-1', 1000)",
        [],
    ).expect("topic");
    source.execute(
        "INSERT INTO conversation_members(actor_id, status, joined_at_ms, updated_at_ms)
         VALUES('actor-1', 'member', 1000, 1000)",
        [],
    ).expect("member");
    source.execute(
        "INSERT INTO conversation_pins(child_scope, message_id, pinned_at_ms, pinned_by)
         VALUES('topic:topic-1', 'entry-a', 1000, 'actor-1')",
        [],
    ).expect("pin");
    drop(source);

    let target = Connection::open(&target_path).expect("target");
    initialize(&target);
    let counts = salvage_store_db(&source_path, &target);
    assert_eq!(counts.kv, 1);
    assert_eq!(counts.blobs, 1);
    assert_eq!(counts.transcript, 1);
    assert_eq!(counts.topics, 1);
    assert_eq!(counts.members, 1);
    assert_eq!(counts.pins, 1);
    assert_eq!(counts.total(), 6);

    let seq: i64 = target
        .query_row(
            "SELECT seq FROM transcript_entries WHERE id='entry-a'",
            [],
            |row| row.get(0),
        )
        .expect("transcript row");
    assert_eq!(seq, 7);

    let topic_title: String = target
        .query_row(
            "SELECT title FROM conversation_topics WHERE topic_id='topic-1'",
            [],
            |row| row.get(0),
        )
        .expect("topic row");
    assert_eq!(topic_title, "Topic 1");

    let member_status: String = target
        .query_row(
            "SELECT status FROM conversation_members WHERE actor_id='actor-1'",
            [],
            |row| row.get(0),
        )
        .expect("member row");
    assert_eq!(member_status, "member");

    let pinned_by: String = target
        .query_row(
            "SELECT pinned_by FROM conversation_pins WHERE child_scope='topic:topic-1' AND message_id='entry-a'",
            [],
            |row| row.get(0),
        )
        .expect("pin row");
    assert_eq!(pinned_by, "actor-1");

    drop(target);
    cleanup(&source_path);
    cleanup(&target_path);
}

#[test]
fn configured_open_recovers_corrupt_store_and_publishes_reindex() {
    let db_path = path("agent-db-corrupt");
    fs::write(&db_path, b"not-a-sqlite-database").expect("corrupt file");

    let observed = Arc::new(Mutex::new(Vec::<serde_json::Map<String, serde_json::Value>>::new()));
    let observed_for_listener = Arc::clone(&observed);
    let subscription = subscribe_transcript_mutations(move |mutation| {
        observed_for_listener
            .lock()
            .expect("mutation lock")
            .push(mutation.clone());
    });

    let options = DbRecoveryOptions {
        on_corruption_recovered: Some(Arc::new(|_| {
            panic!("callback failures are intentionally ignored");
        })),
        ..DbRecoveryOptions::default()
    };
    let db = open_configured_db(
        &db_path,
        "agent-corrupt",
        &options,
        false,
    )
    .expect("recover");
    assert!(is_store_db_healthy(&db));
    let kv_present: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='kv'",
            [],
            |row| row.get(0),
        )
        .expect("kv table");
    assert_eq!(kv_present, 1);
    drop(db);

    let mutations = observed.lock().expect("mutation lock");
    assert!(mutations.iter().any(|mutation| {
        mutation.get("kind").and_then(serde_json::Value::as_str) == Some("agent-needs-reindex")
            && mutation.get("agentId").and_then(serde_json::Value::as_str) == Some("agent-corrupt")
    }));
    drop(mutations);
    subscription.unsubscribe();

    cleanup(&db_path);
}
