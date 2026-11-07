use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::production_binding_providers::{
    create_production_state_backstop_runtime_with,
    production_cloud_agent_trace_converter,
};
use mahayana_host_runtime::storage::agent_paths::get_sand_agents_root_dir;
use rusqlite::Connection;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-production-bindings-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn production_state_backstop_binding_uses_canonical_agents_root_and_checkpointed_read() {
    let home = temp_root("backstop");
    let runtime = create_production_state_backstop_runtime_with(Some(&home), 500);
    let expected_agents_root = get_sand_agents_root_dir(Some(&home));
    assert_eq!(
        runtime.agents_root_dir,
        expected_agents_root,
        "production binding must delegate to the canonical Sand agents-root resolver"
    );

    let agent_dir = runtime.agents_root_dir.join("agent-a");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    let db_path = agent_dir.join("store.db");
    let db = Connection::open(&db_path).expect("open sqlite");
    db.pragma_update(None, "journal_mode", "WAL").expect("wal mode");
    db.execute_batch(
        "CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         INSERT INTO kv (key, value) VALUES ('answer', '42');",
    )
    .expect("seed sqlite");
    drop(db);

    let bytes = (runtime.read_db_bytes)(&db_path)
        .expect("checkpointed read")
        .expect("database bytes");
    assert!(!bytes.is_empty());

    let reopened_path = home.join("reopened.db");
    fs::write(&reopened_path, bytes).expect("write snapshot");
    let reopened = Connection::open(&reopened_path).expect("open snapshot");
    let value: String = reopened
        .query_row("SELECT value FROM kv WHERE key='answer'", [], |row| row.get(0))
        .expect("snapshot row");
    assert_eq!(value, "42");

    assert_eq!(
        (runtime.read_db_bytes)(&agent_dir.join("missing.db"))
            .expect("missing read"),
        None
    );

    let _ = fs::remove_dir_all(home);
}

#[test]
fn cloud_agent_trace_binding_consumes_generated_conversation_message_wire() {
    let converter = production_cloud_agent_trace_converter();
    // aiserver.v1.ConversationMessage { text: "hi", type: HUMAN }
    let rows = converter(&[vec![0x0a, 0x02, b'h', b'i', 0x10, 0x01]])
        .expect("canonical generated ConversationMessage trace binding");
    assert_eq!(
        rows,
        vec![serde_json::json!({
            "role": "user",
            "text": "hi",
        })]
    );
}
