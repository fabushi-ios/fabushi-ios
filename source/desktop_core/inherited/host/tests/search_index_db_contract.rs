use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::search_index_db::{
    AttachmentKind, FTS_QUERY_MAX_TERMS, META_RECONCILE_DONE, SEARCH_INDEX_FILENAME,
    SEARCH_INDEX_SCHEMA_VERSION, build_fts_match_query, ensure_search_index_schema,
    open_search_index_db, read_reconcile_done, read_search_index_schema_version,
    search_media, search_messages, stamp_search_index_schema_version, write_reconcile_done,
};

fn temp_db(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-content-search-{label}-{}-{suffix}.db",
        std::process::id()
    ))
}

#[test]
fn schema_version_reconcile_and_pragmas_match_frozen_db_contract() {
    let path = temp_db("schema");
    let db = open_search_index_db(&path).expect("open");
    ensure_search_index_schema(&db).expect("schema");

    assert_eq!(SEARCH_INDEX_FILENAME, "search-index.db");
    assert_eq!(SEARCH_INDEX_SCHEMA_VERSION, 1);
    assert_eq!(META_RECONCILE_DONE, "reconcile_done");
    assert_eq!(read_search_index_schema_version(&db).unwrap(), 0);
    stamp_search_index_schema_version(&db).unwrap();
    assert_eq!(read_search_index_schema_version(&db).unwrap(), 1);
    assert!(!read_reconcile_done(&db).unwrap());
    write_reconcile_done(&db).unwrap();
    assert!(read_reconcile_done(&db).unwrap());

    let journal: String = db.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
    let synchronous: i64 = db.query_row("PRAGMA synchronous", [], |row| row.get(0)).unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 1);

    drop(db);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn fts_query_nfkc_quotes_terms_and_caps_at_eight() {
    assert_eq!(build_fts_match_query("   "), None);
    assert_eq!(
        build_fts_match_query("Ａlpha beta").as_deref(),
        Some("\"Alpha\"* \"beta\"*")
    );
    assert_eq!(
        build_fts_match_query("a \"b\"").as_deref(),
        Some("\"a\"* \"\"\"b\"\"\"*")
    );
    let query = (0..FTS_QUERY_MAX_TERMS + 3)
        .map(|n| format!("t{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        build_fts_match_query(&query)
            .unwrap()
            .split_whitespace()
            .count(),
        FTS_QUERY_MAX_TERMS
    );
}

#[test]
fn message_search_enforces_per_agent_cap_recency_and_timestamp_fallback() {
    let path = temp_db("messages");
    let db = open_search_index_db(&path).unwrap();
    ensure_search_index_schema(&db).unwrap();

    for index in 0..7i64 {
        db.execute(
            "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
             VALUES ('a', ?1, 'user', ?2, ?3)",
            rusqlite::params![format!("a-{index}"), 100 + index, format!("needle   a {index}")],
        ).unwrap();
    }
    db.execute(
        "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
         VALUES ('b', 'b-latest', 'assistant', 250, 'needle latest')",
        [],
    ).unwrap();
    db.execute(
        "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
         VALUES ('b', 'b-zero', 'assistant', 0, 'needle fallback')",
        [],
    ).unwrap();

    let rows = search_messages(&db, "needle", 20).unwrap();
    assert_eq!(rows.iter().filter(|row| row.agent_id == "a").count(), 5);
    assert_eq!(rows[0].agent_id, "b");
    assert_eq!(rows[0].timestamp_ms, 250);
    let zero = rows.iter().find(|row| row.entry_id == "b-zero").unwrap();
    assert_eq!(zero.timestamp_ms, 250);
    assert!(!zero.snippet.contains("  "));
    assert!(search_messages(&db, "", 10).unwrap().is_empty());
    assert!(search_messages(&db, "needle", 0).unwrap().is_empty());

    drop(db);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn media_search_supports_recent_all_fts_filter_and_unknown_kind_fallback() {
    let path = temp_db("media");
    let db = open_search_index_db(&path).unwrap();
    ensure_search_index_schema(&db).unwrap();

    db.execute(
        "INSERT INTO media(agent_id, entry_id, file_name, ext, mime, kind, timestamp_ms, width, height)
         VALUES ('a', 'one', 'alpha report.pdf', 'pdf', 'application/pdf', 'pdf', 10, NULL, NULL)",
        [],
    ).unwrap();
    db.execute(
        "INSERT INTO media(agent_id, entry_id, file_name, ext, mime, kind, timestamp_ms, width, height)
         VALUES ('b', 'two', 'holiday photo.png', 'png', 'image/png', 'mystery', 20, 640, 480)",
        [],
    ).unwrap();

    let recent = search_media(&db, "", 10).unwrap();
    assert_eq!(recent[0].entry_id, "two");
    assert_eq!(recent[0].kind, AttachmentKind::File);
    assert_eq!(recent[0].width, Some(640));

    let filtered = search_media(&db, "report", 10).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].entry_id, "one");
    assert_eq!(filtered[0].kind, AttachmentKind::Pdf);
    assert!(search_media(&db, "photo", 0).unwrap().is_empty());

    drop(db);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(path.with_extension("db-wal"));
    let _ = fs::remove_file(path.with_extension("db-shm"));
}
