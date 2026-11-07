use mahayana_host_runtime::extensions::session::agent_db_schema::{
    AGENT_DB_SCHEMA, AGENT_DB_SCHEMA_VERSION, BRANCHED_ENTRY_FILTER_SQL, CLEAR_BLOBS_SQL,
    CLEAR_TRANSCRIPT_ENTRIES_SQL, COMPARE_AND_SET_KV_SQL,
    DELETE_KV_SQL, DELETE_TRANSCRIPT_ENTRY_SQL, DIVIDER_ANCHOR_ENTRY_FILTER_SQL,
    GET_KV_SQL, GET_TRANSCRIPT_ENTRY_SQL, HAS_LEGACY_BLOB_SQL,
    INSERT_TRANSCRIPT_ENTRY_SQL, LIST_BRANCHED_ENTRIES_SQL,
    LIST_TRANSCRIPT_ENTRIES_SQL, LIST_TRANSCRIPT_PAGE_SQL,
    LIST_TRANSCRIPT_TAIL_SQL, LIST_TRANSCRIPT_WINDOW_SQL,
    MAIN_TRANSCRIPT_MESSAGE_FILTER_SQL, NEWEST_DIVIDER_ANCHOR_TIMESTAMP_SQL,
    PREPARED_STATEMENT_SQL, SET_KV_SQL, UPDATE_TRANSCRIPT_ENTRY_SQL,
    WINDOW_ENTRY_FILTER_SQL,
};
use rusqlite::{params, Connection};

fn entry(id: &str, value: serde_json::Value) -> String {
    let mut object = value.as_object().cloned().expect("entry object");
    object.insert("id".into(), serde_json::Value::String(id.to_string()));
    serde_json::to_string(&object).expect("entry json")
}

#[test]
fn frozen_agent_db_schema_creates_strict_tables_indexes_and_all_prepared_sql() {
    let db = Connection::open_in_memory().expect("sqlite");
    db.execute_batch(AGENT_DB_SCHEMA).expect("agent schema");

    assert_eq!(AGENT_DB_SCHEMA_VERSION, "2");

    for table in [
        "kv",
        "blobs",
        "transcript_entries",
        "conversation_topics",
        "conversation_members",
        "conversation_pins",
    ] {
        let sql: String = db
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )
            .expect("table ddl");
        assert!(sql.to_ascii_uppercase().contains("STRICT"), "table {table} must be STRICT");
    }

    for index in ["idx_transcript_window", "idx_transcript_branched", "idx_transcript_topic"] {
        let present: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                params![index],
                |row| row.get(0),
            )
            .expect("index lookup");
        assert_eq!(present, 1, "index {index} must be present");
    }

    for (name, sql) in PREPARED_STATEMENT_SQL {
        db.prepare(sql)
            .unwrap_or_else(|error| panic!("prepare {name}: {error}"));
    }

    assert!(WINDOW_ENTRY_FILTER_SQL.contains("tool-call"));
    assert!(BRANCHED_ENTRY_FILTER_SQL.contains("$.branched"));
    assert!(MAIN_TRANSCRIPT_MESSAGE_FILTER_SQL.contains("send-message"));
    assert!(DIVIDER_ANCHOR_ENTRY_FILTER_SQL.contains("user-attachment"));
}

#[test]
fn frozen_kv_statements_preserve_upsert_compare_and_set_and_delete_contract() {
    let db = Connection::open_in_memory().expect("sqlite");
    db.execute_batch(AGENT_DB_SCHEMA).expect("agent schema");

    db.execute(SET_KV_SQL, params!["metadata", "v1"])
        .expect("insert kv");
    assert_eq!(
        db.query_row(GET_KV_SQL, params!["metadata"], |row| row.get::<_, String>(0))
            .expect("read kv"),
        "v1"
    );

    assert_eq!(
        db.execute(COMPARE_AND_SET_KV_SQL, params!["v2", "metadata", "wrong"])
            .expect("failed cas"),
        0
    );
    assert_eq!(
        db.execute(COMPARE_AND_SET_KV_SQL, params!["v2", "metadata", "v1"])
            .expect("cas"),
        1
    );
    assert_eq!(
        db.execute(DELETE_KV_SQL, params!["metadata"])
            .expect("delete kv"),
        1
    );

    db.execute(
        "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
        params!["aa", b"legacy".as_slice()],
    )
    .expect("legacy blob");
    assert_eq!(
        db.query_row(HAS_LEGACY_BLOB_SQL, [], |row| row.get::<_, i64>(0))
            .expect("legacy present"),
        1
    );
    db.execute(CLEAR_BLOBS_SQL, []).expect("clear blobs");
}

#[test]
fn frozen_transcript_filters_preserve_window_branch_page_and_divider_behavior() {
    let db = Connection::open_in_memory().expect("sqlite");
    db.execute_batch(AGENT_DB_SCHEMA).expect("agent schema");

    let rows = [
        (
            "u1",
            entry(
                "u1",
                serde_json::json!({"kind":"message","role":"user","content":"hello","timestampMs":100}),
            ),
        ),
        (
            "tool",
            entry(
                "tool",
                serde_json::json!({"kind":"tool-call","name":"x","status":"done","timestampMs":120}),
            ),
        ),
        (
            "branch",
            entry(
                "branch",
                serde_json::json!({"kind":"message","role":"assistant","content":"branch","branched":true,"timestampMs":130}),
            ),
        ),
        (
            "send",
            entry(
                "send",
                serde_json::json!({"kind":"send-message","message":{"type":"text","content":"hi"},"timestampMs":140}),
            ),
        ),
        (
            "attachment",
            entry(
                "attachment",
                serde_json::json!({"kind":"user-attachment","file_path":"/tmp/a","timestampMs":150}),
            ),
        ),
        (
            "agent",
            entry(
                "agent",
                serde_json::json!({"kind":"message","role":"assistant","content":"handoff","fromAgent":"agent-b","timestampMs":160}),
            ),
        ),
    ];
    for (id, json) in rows {
        db.execute(INSERT_TRANSCRIPT_ENTRY_SQL, params![id, json])
            .expect("insert transcript");
    }

    let list = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Vec<String> {
        let mut statement = db.prepare(sql).expect("prepare list");
        let rows = statement
            .query_map(params, |row| row.get::<_, String>(1))
            .expect("query list");
        rows.map(|row| row.expect("entry")).collect()
    };

    let window = list(
        LIST_TRANSCRIPT_WINDOW_SQL,
        &[
            &Option::<i64>::None,
            &Option::<i64>::None,
            &50_i64,
        ],
    );
    assert_eq!(window.len(), 4);
    assert!(window.iter().all(|raw| !raw.contains(r#""id":"tool""#)));
    assert!(window.iter().all(|raw| !raw.contains(r#""id":"branch""#)));

    let mut branched = db.prepare(LIST_BRANCHED_ENTRIES_SQL).expect("branched");
    let branches = branched
        .query_map([], |row| row.get::<_, String>(0))
        .expect("branch query")
        .map(|row| row.expect("branch row"))
        .collect::<Vec<_>>();
    assert_eq!(branches.len(), 1);
    assert!(branches[0].contains(r#""id":"branch""#));

    let newest: i64 = db
        .query_row(NEWEST_DIVIDER_ANCHOR_TIMESTAMP_SQL, [], |row| row.get(0))
        .expect("newest divider timestamp");
    assert_eq!(newest, 160);

    let page = list(
        LIST_TRANSCRIPT_PAGE_SQL,
        &[
            &Option::<i64>::None,
            &Option::<i64>::None,
            &Option::<i64>::None,
            &Option::<i64>::None,
            &200_i64,
            &50_i64,
        ],
    );
    assert_eq!(page.len(), 4);
    assert!(page.iter().any(|raw| raw.contains(r#""id":"u1""#)));
    assert!(page.iter().any(|raw| raw.contains(r#""id":"send""#)));
    assert!(page.iter().any(|raw| raw.contains(r#""id":"attachment""#)));
    assert!(page.iter().any(|raw| raw.contains(r#""id":"agent""#)));

    let tail = list(
        LIST_TRANSCRIPT_TAIL_SQL,
        &[
            &Option::<i64>::None,
            &Option::<i64>::None,
            &50_i64,
        ],
    );
    assert_eq!(tail.len(), 6);

    let listed = db
        .prepare(LIST_TRANSCRIPT_ENTRIES_SQL)
        .expect("list entries")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("list query")
        .count();
    assert_eq!(listed, 6);

    let raw: String = db
        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params!["send"], |row| row.get(0))
        .expect("get transcript");
    assert!(raw.contains(r#""id":"send""#));

    db.execute(
        UPDATE_TRANSCRIPT_ENTRY_SQL,
        params![
            entry(
                "send",
                serde_json::json!({"kind":"send-message","message":{"type":"text","content":"updated"},"timestampMs":140})
            ),
            "send"
        ],
    )
    .expect("update transcript");
    assert_eq!(
        db.execute(DELETE_TRANSCRIPT_ENTRY_SQL, params!["tool"])
            .expect("delete transcript"),
        1
    );
    db.execute(CLEAR_TRANSCRIPT_ENTRIES_SQL, [])
        .expect("clear transcripts");
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM transcript_entries", [], |row| row.get(0))
        .expect("count transcripts");
    assert_eq!(count, 0);
}
