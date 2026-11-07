use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::search_index_db::{
    ensure_search_index_schema, open_search_index_db, search_messages,
};
use mahayana_host_runtime::extensions::content_search::search_index_worker::{
    SearchIndexWorker, SearchIndexWorkerConfig, SearchIndexWorkerRequest,
    is_sqlite_corrupt_error,
};
use mahayana_host_runtime::extensions::content_search::search_index_writer::{
    IndexEntry, SearchIndexJob,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-search-worker-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn worker_preserves_request_id_and_executes_jobs_on_its_owned_connection() {
    let root = temp_root("request");
    fs::create_dir_all(&root).unwrap();
    let db_path = root.join("search-index.db");
    let mut worker = SearchIndexWorker::spawn(SearchIndexWorkerConfig {
        index_db_path: db_path.clone(),
        agents_root_dir: root.join("agents"),
    }).unwrap();

    let entry: IndexEntry = serde_json::from_value(serde_json::json!({
        "id": "entry-1",
        "kind": "message",
        "role": "user",
        "content": "worker needle",
        "timestampMs": 10
    })).unwrap();
    let response = worker.post(SearchIndexWorkerRequest {
        request_id: 41,
        job: SearchIndexJob::UpsertEntries {
            agent_id: "agent-a".into(),
            entries: vec![entry],
        },
    }).unwrap();
    assert_eq!(response.request_id, 41);
    assert!(response.ok);
    assert_eq!(response.message, None);
    assert!(!response.is_index_corrupt);

    worker.terminate();

    let db = open_search_index_db(&db_path).unwrap();
    ensure_search_index_schema(&db).unwrap();
    let rows = search_messages(&db, "needle", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry_id, "entry-1");

    drop(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_stop_is_idempotent_and_future_posts_fail_closed() {
    let root = temp_root("stop");
    fs::create_dir_all(&root).unwrap();
    let mut worker = SearchIndexWorker::spawn(SearchIndexWorkerConfig {
        index_db_path: root.join("search-index.db"),
        agents_root_dir: root.join("agents"),
    }).unwrap();
    worker.terminate();
    worker.terminate();

    let error = worker.post(SearchIndexWorkerRequest {
        request_id: 7,
        job: SearchIndexJob::Reconcile,
    }).unwrap_err();
    assert!(error.contains("no longer running"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn sqlite_corruption_classifier_only_marks_corruption_codes() {
    let corrupt = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseCorrupt,
            extended_code: 11,
        },
        Some("corrupt".into()),
    );
    assert!(is_sqlite_corrupt_error(&corrupt));

    let busy = rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        Some("busy".into()),
    );
    assert!(!is_sqlite_corrupt_error(&busy));
}
