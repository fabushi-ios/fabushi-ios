use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::search_index_db::{
    AttachmentKind, ensure_search_index_schema, open_search_index_db, read_reconcile_done,
    search_media, search_messages,
};
use mahayana_host_runtime::extensions::content_search::search_index_writer::{
    INCREMENTAL_VACUUM_PAGES, INDEXED_BODY_MAX_CHARS, IndexEntry, SandSearchIndexWriter,
    SearchIndexJob, STORE_FILENAME,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-search-writer-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn entry(value: serde_json::Value) -> IndexEntry {
    serde_json::from_value(value).expect("entry")
}

fn create_store(root: &std::path::Path, agent_id: &str, entries: &[serde_json::Value]) {
    let dir = root.join(agent_id);
    fs::create_dir_all(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join(STORE_FILENAME)).unwrap();
    db.execute_batch(
        "CREATE TABLE transcript_entries(
            seq INTEGER PRIMARY KEY,
            entry TEXT NOT NULL
        );",
    )
    .unwrap();
    for (index, value) in entries.iter().enumerate() {
        db.execute(
            "INSERT INTO transcript_entries(seq, entry) VALUES (?1, ?2)",
            rusqlite::params![(index + 1) as i64, value.to_string()],
        )
        .unwrap();
    }
}

#[test]
fn entry_projection_preserves_frozen_message_and_media_rules() {
    let root = temp_root("projection");
    fs::create_dir_all(&root).unwrap();
    let db_path = root.join("index.db");
    let db = open_search_index_db(&db_path).unwrap();
    ensure_search_index_schema(&db).unwrap();
    let mut writer = SandSearchIndexWriter::new(db, &root);

    let body = format!("  {}  ", "x".repeat(INDEXED_BODY_MAX_CHARS + 25));
    let entries = vec![
        entry(serde_json::json!({
            "id": "user",
            "kind": "message",
            "role": "user",
            "content": body,
            "timestampMs": 10.6
        })),
        entry(serde_json::json!({
            "id": "peer-hidden",
            "kind": "message",
            "role": "user",
            "content": "must not index",
            "toAgent": {"kind": "user"}
        })),
        entry(serde_json::json!({
            "id": "notice",
            "kind": "notice",
            "text": " notice body "
        })),
        entry(serde_json::json!({
            "id": "attachment",
            "kind": "user-attachment",
            "file_path": "file:///tmp/My%20Photo.PNG",
            "timestampMs": 5.4,
            "width": 640.2,
            "height": 480.7
        })),
        entry(serde_json::json!({
            "id": "archive",
            "kind": "send-message",
            "message": {
                "type": "attachment",
                "url": "https://example.test/files/build.tar.gz",
                "file_name": " build.tar.gz "
            },
            "timestampMs": 7
        })),
    ];

    writer.upsert_entries("agent-a", &entries).unwrap();
    let messages = search_messages(writer.connection(), "notice", 10).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].entry_id, "notice");
    assert_eq!(messages[0].role, "assistant");
    assert!(search_messages(writer.connection(), "must", 10).unwrap().is_empty());

    let user = search_messages(writer.connection(), "xxxxx", 10).unwrap();
    assert_eq!(user[0].timestamp_ms, 11);
    let stored_len: i64 = writer.connection().query_row(
        "SELECT length(body) FROM messages WHERE entry_id='user'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(stored_len, INDEXED_BODY_MAX_CHARS as i64);

    let media = search_media(writer.connection(), "", 10).unwrap();
    let photo = media.iter().find(|item| item.entry_id == "attachment").unwrap();
    assert_eq!(photo.file_name, "My Photo.PNG");
    assert_eq!(photo.ext, ".png");
    assert_eq!(photo.mime.as_deref(), Some("image/png"));
    assert_eq!(photo.kind, AttachmentKind::Image);
    assert_eq!(photo.timestamp_ms, 5);
    assert_eq!(photo.width, Some(640));
    assert_eq!(photo.height, Some(481));
    let archive = media.iter().find(|item| item.entry_id == "archive").unwrap();
    assert_eq!(archive.kind, AttachmentKind::Archive);
    assert_eq!(archive.mime, None);

    assert_eq!(INCREMENTAL_VACUUM_PAGES, 512);
    drop(writer);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reindex_and_reconcile_follow_store_fingerprint_and_remove_missing_agents() {
    let root = temp_root("reconcile");
    fs::create_dir_all(&root).unwrap();
    create_store(
        &root,
        "agent-a",
        &[
            serde_json::json!({
                "id": "one",
                "kind": "message",
                "role": "user",
                "content": "alpha needle",
                "timestampMs": 20
            }),
            serde_json::json!({
                "id": "two",
                "kind": "send-message",
                "message": {"type": "text", "content": "beta needle"},
                "timestampMs": 30
            }),
        ],
    );

    let db = open_search_index_db(root.join("index.db")).unwrap();
    ensure_search_index_schema(&db).unwrap();
    let mut writer = SandSearchIndexWriter::new(db, &root);
    writer.run_job(&SearchIndexJob::Reconcile).unwrap();
    assert!(read_reconcile_done(writer.connection()).unwrap());

    let messages = search_messages(writer.connection(), "needle", 10).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].entry_id, "two");
    let fingerprint: String = writer.connection().query_row(
        "SELECT fingerprint FROM agents WHERE agent_id='agent-a'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(fingerprint, "2:2");

    writer.connection().execute(
        "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
         VALUES ('gone', 'ghost', 'assistant', 1, 'ghost')",
        [],
    ).unwrap();
    writer.reconcile().unwrap();
    let gone: i64 = writer.connection().query_row(
        "SELECT COUNT(*) FROM messages WHERE agent_id='gone'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(gone, 0);

    fs::remove_dir_all(root.join("agent-a")).unwrap();
    writer.reconcile().unwrap();
    let count: i64 = writer.connection().query_row(
        "SELECT COUNT(*) FROM messages WHERE agent_id='agent-a'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 0);

    drop(writer);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_clear_and_job_dispatch_share_one_writer_state_machine() {
    let root = temp_root("jobs");
    fs::create_dir_all(&root).unwrap();
    let db = open_search_index_db(root.join("index.db")).unwrap();
    ensure_search_index_schema(&db).unwrap();
    let mut writer = SandSearchIndexWriter::new(db, &root);

    let item = entry(serde_json::json!({
        "id": "one",
        "kind": "message",
        "role": "assistant",
        "content": "searchable"
    }));
    writer.run_job(&SearchIndexJob::UpsertEntries {
        agent_id: "agent-a".into(),
        entries: vec![item],
    }).unwrap();
    assert_eq!(search_messages(writer.connection(), "searchable", 10).unwrap().len(), 1);

    writer.run_job(&SearchIndexJob::DeleteEntry {
        agent_id: "agent-a".into(),
        entry_id: "one".into(),
    }).unwrap();
    assert!(search_messages(writer.connection(), "searchable", 10).unwrap().is_empty());

    let media = entry(serde_json::json!({
        "id": "media",
        "kind": "user-attachment",
        "file_path": "/tmp/data.jsonc"
    }));
    writer.run_job(&SearchIndexJob::UpsertEntries {
        agent_id: "agent-a".into(),
        entries: vec![media],
    }).unwrap();
    assert_eq!(search_media(writer.connection(), "", 10).unwrap()[0].kind, AttachmentKind::Json);

    writer.run_job(&SearchIndexJob::ClearAgent {
        agent_id: "agent-a".into(),
    }).unwrap();
    assert!(search_media(writer.connection(), "", 10).unwrap().is_empty());

    drop(writer);
    let _ = fs::remove_dir_all(root);
}
