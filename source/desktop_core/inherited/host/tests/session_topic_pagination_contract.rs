use mahayana_host_runtime::extensions::session::agent_db_schema::AGENT_DB_SCHEMA;
use mahayana_host_runtime::extensions::session::agent_db_transcript_pages::{
    HumanTranscriptPageQuery, conversation_snapshot_scope, human_snapshot_scope,
    read_human_transcript_entries_for_topic, read_human_transcript_tail,
};
use rusqlite::{params, Connection};
use serde_json::json;

fn setup_db() -> Connection {
    let db = Connection::open_in_memory().expect("sqlite memory");
    db.execute_batch(AGENT_DB_SCHEMA).expect("schema");
    db
}

fn set_metadata(db: &Connection, metadata: serde_json::Value) {
    db.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![serde_json::to_string(&metadata).expect("json string")],
    )
    .expect("set metadata");
}

fn insert_message(
    db: &Connection,
    seq: i64,
    id: &str,
    timestamp_ms: f64,
    content: &str,
    topic_id: Option<&str>,
) {
    let mut entry = json!({
        "id": id,
        "kind": "message",
        "role": "user",
        "authorKind": "human",
        "authorId": "user-1",
        "content": content,
        "timestampMs": timestamp_ms,
    });
    if let Some(tid) = topic_id {
        entry["topicId"] = json!(tid);
    }
    db.execute(
        "INSERT INTO transcript_entries (seq, id, entry) VALUES (?1, ?2, ?3)",
        params![seq, id, entry.to_string()],
    )
    .expect("insert entry");
}

#[test]
fn test_group_topic_pagination_isolation_and_full_walk() {
    let db = setup_db();
    let group_metadata = json!({
        "conversationId": "group-alpha",
        "conversationKind": "group",
        "localHumanId": "user-1",
        "participantIds": ["user-1", "user-2", "user-3"]
    });
    set_metadata(&db, group_metadata.clone());

    // Main conversation messages (no topicId)
    insert_message(&db, 1, "m-root-1", 1000.0, "Root 1", None);
    insert_message(&db, 2, "m-root-2", 2000.0, "Root 2", None);
    insert_message(&db, 3, "m-root-3", 3000.0, "Root 3", None);

    // Topic alpha messages
    insert_message(&db, 4, "m-t1-1", 1100.0, "T1 1", Some("topic-alpha"));
    insert_message(&db, 5, "m-t1-2", 2100.0, "T1 2", Some("topic-alpha"));
    insert_message(&db, 6, "m-t1-3", 3100.0, "T1 3", Some("topic-alpha"));

    // Topic beta messages
    insert_message(&db, 7, "m-t2-1", 1200.0, "T2 1", Some("topic-beta"));
    insert_message(&db, 8, "m-t2-2", 2200.0, "T2 2", Some("topic-beta"));

    // 1. Verify read_human_transcript_entries_for_topic isolation
    let root_entries = read_human_transcript_entries_for_topic(&db, None).expect("root entries");
    let root_ids: Vec<&str> = root_entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(root_ids, vec!["m-root-1", "m-root-2", "m-root-3"]);

    let t1_entries =
        read_human_transcript_entries_for_topic(&db, Some("topic-alpha")).expect("t1 entries");
    let t1_ids: Vec<&str> = t1_entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(t1_ids, vec!["m-t1-1", "m-t1-2", "m-t1-3"]);

    let t2_entries =
        read_human_transcript_entries_for_topic(&db, Some("topic-beta")).expect("t2 entries");
    let t2_ids: Vec<&str> = t2_entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(t2_ids, vec!["m-t2-1", "m-t2-2"]);

    // 2. Multi-page pagination in Topic alpha
    let t1_page1 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 2,
            topic_id: Some("topic-alpha".into()),
        },
        &group_metadata,
    )
    .expect("t1 page 1")
    .0;

    let t1_page1_ids: Vec<&str> = t1_page1
        .entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(t1_page1_ids, vec!["m-t1-2", "m-t1-3"]);
    assert_eq!(t1_page1.next_before_seq, Some(5));
    let t1_cursor = t1_page1
        .next_before_cursor
        .expect("cursor for continuation");

    let t1_page2 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: t1_page1.next_before_seq,
            before_cursor: Some(t1_cursor),
            limit: 2,
            topic_id: Some("topic-alpha".into()),
        },
        &group_metadata,
    )
    .expect("t1 page 2")
    .0;

    let t1_page2_ids: Vec<&str> = t1_page2
        .entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(t1_page2_ids, vec!["m-t1-1"]);
    assert_eq!(t1_page2.next_before_seq, None);
    assert_eq!(t1_page2.next_before_cursor, None);

    // 3. Multi-page pagination in Main / Root view
    let root_page1 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 2,
            topic_id: None,
        },
        &group_metadata,
    )
    .expect("root page 1")
    .0;

    let root_page1_ids: Vec<&str> = root_page1
        .entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(root_page1_ids, vec!["m-root-2", "m-root-3"]);
    let root_cursor = root_page1
        .next_before_cursor
        .expect("cursor for root continuation");

    let root_page2 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: root_page1.next_before_seq,
            before_cursor: Some(root_cursor),
            limit: 2,
            topic_id: None,
        },
        &group_metadata,
    )
    .expect("root page 2")
    .0;

    let root_page2_ids: Vec<&str> = root_page2
        .entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    assert_eq!(root_page2_ids, vec!["m-root-1"]);
    assert_eq!(root_page2.next_before_seq, None);
    assert_eq!(root_page2.next_before_cursor, None);
}

#[test]
fn test_concurrent_insert_boundary_stability() {
    let db = setup_db();
    let group_metadata = json!({
        "conversationId": "group-stable",
        "conversationKind": "group",
        "localHumanId": "user-1",
        "participantIds": ["user-1", "user-2"]
    });
    set_metadata(&db, group_metadata.clone());

    // Insert 4 messages in topic-target
    insert_message(&db, 10, "st-1", 100.0, "Target 1", Some("topic-target"));
    insert_message(&db, 20, "st-2", 200.0, "Target 2", Some("topic-target"));
    insert_message(&db, 30, "st-3", 300.0, "Target 3", Some("topic-target"));
    insert_message(&db, 40, "st-4", 400.0, "Target 4", Some("topic-target"));

    // Read Page 1 of topic-target
    let page1 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 2,
            topic_id: Some("topic-target".into()),
        },
        &group_metadata,
    )
    .expect("page 1")
    .0;

    let page1_ids: Vec<&str> = page1.entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(page1_ids, vec!["st-3", "st-4"]);
    let cursor = page1.next_before_cursor.expect("cursor");

    // Concurrent background writes:
    // 1. Insert into a DIFFERENT topic
    insert_message(&db, 50, "other-1", 250.0, "Other topic", Some("topic-other"));
    insert_message(&db, 60, "other-2", 450.0, "Other topic", Some("topic-other"));
    // 2. Insert into main conversation (topic_id: None)
    insert_message(&db, 70, "main-1", 350.0, "Main", None);
    // 3. Late backfill insert into topic-target (should belong to next snapshot, not current walk)
    insert_message(&db, 80, "st-late", 250.0, "Target late", Some("topic-target"));

    // Continue Page 2 with cursor:
    let page2 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: page1.next_before_seq,
            before_cursor: Some(cursor),
            limit: 2,
            topic_id: Some("topic-target".into()),
        },
        &group_metadata,
    )
    .expect("page 2 continuation");

    let page2_ids: Vec<&str> = page2
        .0
        .entries
        .iter()
        .filter_map(|e| e["id"].as_str())
        .collect();
    // Must return exactly st-1 and st-2 from the frozen sequence index; no drift from concurrent writes!
    assert_eq!(page2_ids, vec!["st-1", "st-2"]);
}

#[test]
fn test_cross_topic_cursor_rejected() {
    let db = setup_db();
    let group_metadata = json!({
        "conversationId": "group-cross",
        "conversationKind": "group",
        "localHumanId": "user-1",
        "participantIds": ["user-1", "user-2"]
    });
    set_metadata(&db, group_metadata.clone());

    insert_message(&db, 1, "t1-1", 100.0, "T1 1", Some("topic-1"));
    insert_message(&db, 2, "t1-2", 200.0, "T1 2", Some("topic-1"));
    insert_message(&db, 3, "t2-1", 100.0, "T2 1", Some("topic-2"));
    insert_message(&db, 4, "t2-2", 200.0, "T2 2", Some("topic-2"));

    let page1_t1 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 1,
            topic_id: Some("topic-1".into()),
        },
        &group_metadata,
    )
    .expect("t1 page 1")
    .0;
    let cursor_t1 = page1_t1.next_before_cursor.expect("cursor");

    // Attempt to query topic-2 with topic-1's cursor -> MUST fail
    let err_cross_topic = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: page1_t1.next_before_seq,
            before_cursor: Some(cursor_t1.clone()),
            limit: 1,
            topic_id: Some("topic-2".into()),
        },
        &group_metadata,
    );
    assert!(
        err_cross_topic.is_err(),
        "Cursor from topic-1 must be rejected when queried against topic-2"
    );

    // Attempt to query main view (topic_id: None) with topic-1's cursor -> MUST fail
    let err_cross_root = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: page1_t1.next_before_seq,
            before_cursor: Some(cursor_t1),
            limit: 1,
            topic_id: None,
        },
        &group_metadata,
    );
    assert!(
        err_cross_root.is_err(),
        "Cursor from topic-1 must be rejected when queried against main view"
    );
}

#[test]
fn test_human_direct_1to1_zero_degradation() {
    let db = setup_db();
    let direct_metadata = json!({
        "conversationId": "human-direct-test-123",
        "conversationKind": "human",
        "participantIds": ["user-1", "user-2"]
    });
    set_metadata(&db, direct_metadata.clone());

    // Legacy messages without topicId
    insert_message(&db, 1, "d-1", 100.0, "Direct 1", None);
    insert_message(&db, 2, "d-2", 200.0, "Direct 2", None);
    insert_message(&db, 3, "d-3", 300.0, "Direct 3", None);

    // Ensure scope matches legacy human_snapshot_scope
    let scope_generic = conversation_snapshot_scope(&direct_metadata, None).expect("generic scope");
    let scope_legacy = human_snapshot_scope(&direct_metadata).expect("legacy scope");
    assert_eq!(scope_generic, scope_legacy);

    // Page 1
    let page1 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 2,
            topic_id: None,
        },
        &direct_metadata,
    )
    .expect("page 1")
    .0;

    let p1_ids: Vec<&str> = page1.entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(p1_ids, vec!["d-2", "d-3"]);
    let cursor = page1.next_before_cursor.expect("cursor");

    // Check registry key in kv is the exact legacy key
    let reg_raw: Option<String> = db
        .query_row(
            "SELECT value FROM kv WHERE key = 'humanTranscriptSnapshots:v1'",
            [],
            |row| row.get(0),
        )
        .ok();
    assert!(
        reg_raw.is_some(),
        "Registry must be stored in 'humanTranscriptSnapshots:v1' for legacy 1:1"
    );

    // Page 2
    let page2 = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: page1.next_before_seq,
            before_cursor: Some(cursor),
            limit: 2,
            topic_id: None,
        },
        &direct_metadata,
    )
    .expect("page 2")
    .0;

    let p2_ids: Vec<&str> = page2.entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(p2_ids, vec!["d-1"]);
    assert_eq!(page2.next_before_seq, None);
    assert_eq!(page2.next_before_cursor, None);
}

#[test]
fn test_invalid_topic_id_and_scope_validation() {
    let db = setup_db();
    let group_metadata = json!({
        "conversationId": "group-test",
        "conversationKind": "group",
        "localHumanId": "user-1",
        "participantIds": ["user-1", "user-2"]
    });
    set_metadata(&db, group_metadata.clone());

    // Empty topicId string
    let err_empty = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 10,
            topic_id: Some("".into()),
        },
        &group_metadata,
    );
    assert!(err_empty.is_err());
    assert!(
        err_empty
            .unwrap_err()
            .to_string()
            .contains("Human transcript cursor topicId must not be empty")
    );

    // Whitespace topicId string
    let err_whitespace = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 10,
            topic_id: Some("   ".into()),
        },
        &group_metadata,
    );
    assert!(err_whitespace.is_err());
    assert!(
        err_whitespace
            .unwrap_err()
            .to_string()
            .contains("Human transcript cursor topicId must not be empty")
    );

    // Invalid conversationKind
    let invalid_metadata = json!({
        "conversationId": "unknown-123",
        "conversationKind": "unsupported_kind"
    });
    let err_scope = conversation_snapshot_scope(&invalid_metadata, None);
    assert!(err_scope.is_err());
    assert!(
        err_scope
            .unwrap_err()
            .to_string()
            .contains("Human transcript cursor has invalid conversation scope")
    );
}

#[test]
fn test_channel_pagination() {
    let db = setup_db();
    let channel_metadata = json!({
        "conversationId": "channel-announcements",
        "conversationKind": "channel"
    });
    set_metadata(&db, channel_metadata.clone());

    insert_message(&db, 1, "c-1", 100.0, "Announcement 1", None);
    insert_message(&db, 2, "c-2", 200.0, "Announcement 2", None);

    let page = read_human_transcript_tail(
        &db,
        &HumanTranscriptPageQuery {
            before_seq: None,
            before_cursor: None,
            limit: 10,
            topic_id: None,
        },
        &channel_metadata,
    )
    .expect("channel page")
    .0;

    let ids: Vec<&str> = page.entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(ids, vec!["c-1", "c-2"]);
}
