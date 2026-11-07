use std::path::{Path, PathBuf};

use mahayana_host_runtime::extensions::session::agent_db::{
    read_persisted_agent_name, read_persisted_latest_root_blob_id, set_persisted_agent_name,
};
use rusqlite::params;
use uuid::Uuid;

fn path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fabushi-{label}-{}.sqlite", Uuid::new_v4()))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn persisted_latest_root_projection_matches_agent_metadata_serde_shape() {
    let db_path = path("agent-db-metadata-root");
    let db = rusqlite::Connection::open(&db_path).expect("open session db");
    db.execute_batch(
        "CREATE TABLE kv (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) STRICT;",
    )
    .expect("create kv");
    let metadata = serde_json::to_vec(&serde_json::json!({
        "agentId": "agent-a",
        "latestRootBlobId": "aabbcc",
        "name": "Agent A",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": 1
    }))
    .expect("metadata json");
    db.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', ?1)",
        params![to_hex(&metadata)],
    )
    .expect("insert metadata");
    drop(db);

    assert_eq!(
        read_persisted_latest_root_blob_id(&db_path, 5_000).expect("project root"),
        vec![0xaa, 0xbb, 0xcc]
    );

    cleanup(&db_path);
}

#[test]
fn persisted_latest_root_projection_is_empty_for_unmaterialized_metadata() {
    let db_path = path("agent-db-empty-metadata");
    let db = rusqlite::Connection::open(&db_path).expect("open session db");
    drop(db);

    assert_eq!(
        read_persisted_latest_root_blob_id(&db_path, 5_000).expect("empty projection"),
        Vec::<u8>::new()
    );

    cleanup(&db_path);
}

#[test]
fn persisted_agent_name_mutation_trims_updates_and_is_idempotent() {
    let db_path = path("agent-db-metadata-name");
    let db = rusqlite::Connection::open(&db_path).expect("open session db");
    db.execute_batch(
        "CREATE TABLE kv (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        ) STRICT;",
    )
    .expect("create kv");
    let metadata = serde_json::to_vec(&serde_json::json!({
        "agentId": "agent-name",
        "latestRootBlobId": "",
        "name": "Old Name",
        "mode": "default",
        "isRunEverything": false,
        "createdAt": 1
    }))
    .expect("metadata json");
    db.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', ?1)",
        params![to_hex(&metadata)],
    )
    .expect("insert metadata");
    drop(db);

    assert!(set_persisted_agent_name(&db_path, 5_000, "  Profile Name  ")
        .expect("set name"));
    assert_eq!(
        read_persisted_agent_name(&db_path, 5_000).expect("read name").as_deref(),
        Some("Profile Name")
    );
    assert!(!set_persisted_agent_name(&db_path, 5_000, "Profile Name")
        .expect("idempotent name"));
    assert!(!set_persisted_agent_name(&db_path, 5_000, "   ")
        .expect("reject blank name"));

    cleanup(&db_path);
}
