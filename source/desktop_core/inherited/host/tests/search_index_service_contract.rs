use std::collections::VecDeque;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::content_search::search_index_db::{
    SEARCH_INDEX_SCHEMA_VERSION, ensure_search_index_schema, open_search_index_db,
    read_search_index_schema_version,
};
use mahayana_host_runtime::extensions::content_search::search_index_service::{
    MAX_FAILED_JOB_RECONCILES, MAX_INDEX_REBUILDS, MAX_WORKER_RESPAWNS,
    SandSearchIndexService, SearchIndexHealth, SearchIndexJobFailure, SearchIndexJobPort,
    SearchIndexJobPortFactory, SearchIndexJobResult, TranscriptMutation,
};
use mahayana_host_runtime::extensions::content_search::search_index_worker::SearchIndexWorkerConfig;
use mahayana_host_runtime::extensions::content_search::search_index_writer::{
    IndexEntry, SearchIndexJob,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-search-service-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn entry(value: serde_json::Value) -> IndexEntry {
    serde_json::from_value(value).unwrap()
}

#[test]
fn start_reconcile_mutation_search_and_dispose_use_separate_worker_owner() {
    let root = temp_root("live");
    fs::create_dir_all(root.join("agents")).unwrap();
    let index_path = root.join("search-index.db");
    let service = SandSearchIndexService::new(&index_path, root.join("agents"));

    service.start();
    assert!(service.when_idle());
    assert!(service.is_search_ready());
    assert_eq!(
        read_search_index_schema_version(
            &open_search_index_db(&index_path).unwrap()
        ).unwrap(),
        SEARCH_INDEX_SCHEMA_VERSION
    );

    service.apply_mutation(TranscriptMutation::EntriesUpserted {
        agent_id: "agent-a".into(),
        entries: vec![entry(serde_json::json!({
            "id": "one",
            "kind": "message",
            "role": "user",
            "content": "service needle",
            "timestampMs": 5
        }))],
    });
    assert!(service.when_idle());
    let rows = service.search_messages("needle", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry_id, "one");

    service.apply_mutation(TranscriptMutation::EntryDeleted {
        agent_id: "agent-a".into(),
        entry_id: "one".into(),
    });
    assert!(service.when_idle());
    assert!(service.search_messages("needle", 10).unwrap().is_empty());

    service.dispose();
    assert!(service.snapshot().is_disposed);
    let _ = fs::remove_dir_all(root);
}

struct ScriptedPort {
    results: Arc<Mutex<VecDeque<SearchIndexJobResult>>>,
}

impl SearchIndexJobPort for ScriptedPort {
    fn post(&mut self, _job: &SearchIndexJob) -> SearchIndexJobResult {
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(SearchIndexJobResult::Ok)
    }

    fn terminate(&mut self) {}
}

#[test]
fn worker_unavailable_respawns_then_reconciles_without_rebuilding_index() {
    let root = temp_root("respawn");
    fs::create_dir_all(&root).unwrap();
    let results = Arc::new(Mutex::new(VecDeque::from([
        SearchIndexJobResult::Failure(SearchIndexJobFailure {
            message: "worker exited".into(),
            is_index_corrupt: false,
            is_worker_unavailable: true,
        }),
        SearchIndexJobResult::Ok,
    ])));
    let factory_results = Arc::clone(&results);
    let factory: SearchIndexJobPortFactory = Arc::new(move |_config: &SearchIndexWorkerConfig| {
        Ok(Box::new(ScriptedPort {
            results: Arc::clone(&factory_results),
        }))
    });
    let health = Arc::new(Mutex::new(Vec::<SearchIndexHealth>::new()));
    let health_sink = Arc::clone(&health);
    let service = SandSearchIndexService::new_with_factory(
        root.join("search-index.db"),
        root.join("agents"),
        factory,
        Arc::new(move |event| health_sink.lock().unwrap().push(event)),
    );

    service.start();
    assert!(service.when_idle());
    assert!(service.when_idle());
    assert_eq!(service.snapshot().worker_respawn_count, 1);
    assert_eq!(service.snapshot().rebuild_count, 0);
    assert!(service.is_search_ready());
    assert!(health.lock().unwrap().iter().any(|event| event.kind == "worker_respawn"));

    service.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn corrupt_worker_result_recreates_index_and_runs_reconcile() {
    let root = temp_root("rebuild");
    fs::create_dir_all(&root).unwrap();
    let results = Arc::new(Mutex::new(VecDeque::from([
        SearchIndexJobResult::Failure(SearchIndexJobFailure {
            message: "database disk image is malformed".into(),
            is_index_corrupt: true,
            is_worker_unavailable: false,
        }),
        SearchIndexJobResult::Ok,
    ])));
    let factory_results = Arc::clone(&results);
    let factory: SearchIndexJobPortFactory = Arc::new(move |_config| {
        Ok(Box::new(ScriptedPort {
            results: Arc::clone(&factory_results),
        }))
    });
    let health = Arc::new(Mutex::new(Vec::<SearchIndexHealth>::new()));
    let health_sink = Arc::clone(&health);
    let service = SandSearchIndexService::new_with_factory(
        root.join("search-index.db"),
        root.join("agents"),
        factory,
        Arc::new(move |event| health_sink.lock().unwrap().push(event)),
    );

    service.start();
    assert!(service.when_idle());
    assert!(service.when_idle());
    assert_eq!(service.snapshot().rebuild_count, 1);
    assert!(!service.snapshot().is_unavailable);
    assert!(service.is_search_ready());
    assert!(health.lock().unwrap().iter().any(|event| event.kind == "corrupt_rebuild"));

    service.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn nonfresh_schema_mismatch_is_recreated_and_limits_match_frozen_service() {
    let root = temp_root("migration");
    fs::create_dir_all(&root).unwrap();
    let index_path = root.join("search-index.db");
    {
        let db = open_search_index_db(&index_path).unwrap();
        ensure_search_index_schema(&db).unwrap();
        db.execute(
            "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
             VALUES ('old', 'entry', 'assistant', 1, 'stale')",
            [],
        ).unwrap();
        db.pragma_update(None, "user_version", 99).unwrap();
    }

    let service = SandSearchIndexService::new(&index_path, root.join("agents"));
    service.start();
    assert!(service.when_idle());
    assert_eq!(
        read_search_index_schema_version(
            &open_search_index_db(&index_path).unwrap()
        ).unwrap(),
        SEARCH_INDEX_SCHEMA_VERSION
    );
    assert!(service.search_messages("stale", 10).unwrap().is_empty());

    assert_eq!(MAX_INDEX_REBUILDS, 3);
    assert_eq!(MAX_WORKER_RESPAWNS, 3);
    assert_eq!(MAX_FAILED_JOB_RECONCILES, 3);

    service.dispose();
    let _ = fs::remove_dir_all(root);
}
