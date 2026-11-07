use mahayana_host_runtime::extensions::session::agent_db_schema::AGENT_DB_SCHEMA;
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::{
    TranscriptPageQuery, TranscriptWindowQuery, read_transcript_page, read_transcript_tail,
    read_transcript_window,
};
use rusqlite::params;
use serde_json::json;

fn insert(db: &rusqlite::Connection, id: &str, entry: serde_json::Value) {
    db.execute(
        "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
        params![id, entry.to_string()],
    )
    .expect("insert transcript entry");
}

#[test]
fn transcript_page_preserves_frozen_order_filtering_and_cursor_semantics() {
    let db = rusqlite::Connection::open_in_memory().expect("in-memory db");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");

    insert(
        &db,
        "old",
        json!({"id":"old","kind":"message","role":"user","content":"old","timestampMs":100}),
    );
    insert(
        &db,
        "tool",
        json!({"id":"tool","kind":"tool-call","name":"search","timestampMs":110}),
    );
    insert(
        &db,
        "branch",
        json!({"id":"branch","kind":"message","role":"user","content":"branch","branched":true,"timestampMs":120}),
    );
    insert(
        &db,
        "send",
        json!({"id":"send","kind":"send-message","message":{"type":"text","content":"send"},"timestampMs":130}),
    );
    insert(
        &db,
        "invalid",
        json!({"id":"invalid","kind":"message","role":"user","content":7,"timestampMs":140}),
    );
    insert(
        &db,
        "latest",
        json!({"id":"latest","kind":"message","fromAgent":"agent-a","content":"latest","timestampMs":150}),
    );
    insert(
        &db,
        "notice",
        json!({"id":"notice","kind":"notice","text":"notice","timestampMs":160}),
    );

    let first = read_transcript_page(
        &db,
        TranscriptPageQuery {
            before_seq: None,
            since_ms: Some(100),
            until_ms: 200,
            limit: 2,
        },
    )
    .expect("first page");
    assert_eq!(
        first
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["latest"]
    );
    assert_eq!(first.next_before_seq, Some(5));

    let second = read_transcript_page(
        &db,
        TranscriptPageQuery {
            before_seq: first.next_before_seq,
            since_ms: Some(100),
            until_ms: 200,
            limit: 2,
        },
    )
    .expect("second page");
    assert_eq!(
        second
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["old", "send"]
    );
    assert_eq!(second.next_before_seq, None);
}

#[test]
fn transcript_window_filters_tool_and_branched_rows_and_invokes_thread_projection() {
    let db = rusqlite::Connection::open_in_memory().expect("in-memory db");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
    for (id, entry) in [
        ("old", json!({"id":"old","kind":"message","role":"user","content":"old"})),
        ("tool", json!({"id":"tool","kind":"tool-call","name":"search"})),
        ("branch", json!({"id":"branch","kind":"message","role":"user","content":"branch","branched":true})),
        ("send", json!({"id":"send","kind":"send-message","message":{"type":"text","content":"send"}})),
        ("invalid", json!({"id":"invalid","kind":"message","role":"user","content":7})),
        ("latest", json!({"id":"latest","kind":"message","fromAgent":"agent-a","content":"latest"})),
        ("notice", json!({"id":"notice","kind":"notice","text":"notice"})),
    ] {
        insert(&db, id, entry);
    }

    let result = read_transcript_window(
        &db,
        TranscriptWindowQuery {
            before_seq: None,
            limit: 3,
        },
        |entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        },
    )
    .expect("window");
    assert_eq!(
        result
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["latest", "notice"]
    );
    assert_eq!(result.thread_counts, vec!["latest", "notice"]);
    assert_eq!(result.next_before_seq, Some(5));
}

#[test]
fn transcript_tail_keeps_tool_and_branched_rows_and_sanitizes_limit() {
    let db = rusqlite::Connection::open_in_memory().expect("in-memory db");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
    for (id, entry) in [
        ("old", json!({"id":"old","kind":"message","role":"user","content":"old"})),
        ("tool", json!({"id":"tool","kind":"tool-call","name":"search"})),
        ("branch", json!({"id":"branch","kind":"message","role":"user","content":"branch","branched":true})),
        ("send", json!({"id":"send","kind":"send-message","message":{"type":"text","content":"send"}})),
    ] {
        insert(&db, id, entry);
    }

    let result = read_transcript_tail(
        &db,
        TranscriptWindowQuery {
            before_seq: None,
            limit: -1,
        },
    )
    .expect("tail");
    assert_eq!(
        result
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["old", "tool", "branch", "send"]
    );

    let bounded = read_transcript_tail(
        &db,
        TranscriptWindowQuery {
            before_seq: Some(5),
            limit: 3,
        },
    )
    .expect("bounded tail");
    assert_eq!(
        bounded
            .entries
            .iter()
            .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>(),
        vec!["tool", "branch", "send"]
    );
    assert_eq!(bounded.next_before_seq, Some(2));
}
