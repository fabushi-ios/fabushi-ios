use super::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Weak, mpsc};
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};

use crate::storage::store_db::live_db_handle_count;

const LOCAL: &str = "human-local";
const PEER: &str = "human-peer";
const WAIT: Duration = Duration::from_secs(20);

fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("fabushi-human-creation-{label}-{}", Uuid::new_v4()))
}

fn test_workers(root: &Path) -> ProductionSessionWorkers {
    ProductionSessionWorkers::with_agents_root_identity_and_dependencies(
        root.to_path_buf(),
        25,
        Some(LOCAL.to_string()),
        Arc::new(|| None),
        Arc::new(MemoryService::new(root.to_path_buf())),
    )
}

fn receive<T>(receiver: &mpsc::Receiver<T>, label: &str) -> T {
    receiver.recv_timeout(WAIT).unwrap_or_else(|error| panic!("{label}: {error}"))
}

type CreationResult = Result<Value, String>;

fn spawn_creation(
    workers: &Arc<ProductionSessionWorkers>,
    peer: &str,
    title: &str,
) -> (mpsc::Receiver<CreationResult>, thread::JoinHandle<()>) {
    let workers = Arc::clone(workers);
    let peer = peer.to_string();
    let title = title.to_string();
    let (finished, result) = mpsc::channel();
    let task = thread::spawn(move || {
        let result = workers.create_human_conversation(&peer, &title);
        finished.send(result).expect("creation result receiver remains owned by the test");
    });
    (result, task)
}

fn finish_creation(
    result: mpsc::Receiver<CreationResult>,
    task: thread::JoinHandle<()>,
) -> CreationResult {
    let result = receive(&result, "creation did not finish");
    task.join().expect("creation thread did not panic");
    result
}

fn conversation_id(conversation: &Value) -> &str {
    conversation["id"].as_str().expect("canonical conversation id")
}

#[derive(Debug, PartialEq, Eq)]
struct StoredRows {
    kv: Vec<(String, String)>,
    transcript: Vec<(i64, String, String)>,
    blobs: Vec<(String, Vec<u8>)>,
}

fn stored_rows(path: &Path) -> StoredRows {
    // Independent read-only handles cannot recreate a path deleted by a creator.
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("the canonical database must remain readable");
    let kv = db.prepare("SELECT key, value FROM kv ORDER BY key")
        .expect("kv snapshot query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("kv snapshot rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("complete kv snapshot");
    let transcript = db.prepare("SELECT seq, id, entry FROM transcript_entries ORDER BY seq")
        .expect("transcript snapshot query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("transcript snapshot rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("complete transcript snapshot");
    let blobs = db.prepare("SELECT id, data FROM blobs ORDER BY id")
        .expect("blob snapshot query")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("blob snapshot rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("complete blob snapshot");
    StoredRows { kv, transcript, blobs }
}

fn persisted_entries() -> Vec<Value> {
    vec![
        json!({
            "id": "human-remote:17", "kind": "message", "role": "user",
            "authorKind": "human", "authorId": PEER, "remoteMessageId": "17",
            "content": "received before repeated creation", "timestampMs": 17.0,
            "delivery": "received",
        }),
        json!({
            "id": "human-message:creation-pending", "kind": "message", "role": "user",
            "authorKind": "human", "authorId": LOCAL, "clientNonce": "creation-pending",
            "content": "durable pending send", "composedAtMs": 18.0,
            "timestampMs": 18.0, "delivery": "pending",
        }),
    ]
}

fn seed_entries(owner: &SandAgentDb) {
    assert_eq!(owner.append_transcript_entries(&persisted_entries()).unwrap(), 2);
    assert!(owner.write_kv("creation-sibling-state", "must-survive").unwrap());
}

fn seed_durable_state(owner: &SandAgentDb) {
    seed_entries(owner);
    assert!(owner.set_metadata("humanHistoryCoverage", json!({"afterId": 17, "complete": false})).unwrap());
    assert!(owner.set_metadata("lastActivityAt", json!(18.0)).unwrap());
}

fn assert_durable_state(owner: &SandAgentDb) {
    assert_eq!(owner.get_transcript_entries().unwrap(), persisted_entries());
    assert_eq!(owner.get_metadata("humanHistoryCoverage").unwrap(), Some(json!({"afterId": 17, "complete": false})));
    assert_eq!(owner.read_kv("creation-sibling-state").unwrap().as_deref(), Some("must-survive"));
}

struct PausedCreation {
    id: String,
    path: PathBuf,
    candidate: Weak<SandAgentDb>,
}

fn pause_once(
    workers: &mut ProductionSessionWorkers,
    stage_to_pause: HumanCreationStage,
) -> (mpsc::Receiver<PausedCreation>, mpsc::Sender<()>) {
    let (paused, observation) = mpsc::channel();
    let (resume, released) = mpsc::channel();
    let released = Mutex::new(released);
    let used = AtomicBool::new(false);
    workers.human_creation_hook = Some(Arc::new(move |stage, id, owner| {
        if stage == stage_to_pause && !used.swap(true, Ordering::SeqCst) {
            let owner = owner.expect("the requested checkpoint follows SQLite schema opening");
            paused.send(PausedCreation {
                id: id.to_string(),
                path: owner.db_path().to_path_buf(),
                candidate: Arc::downgrade(owner),
            }).expect("send owned creation checkpoint");
            receive(&released.lock().unwrap(), "release paused creation");
        }
    }));
    (observation, resume)
}

fn cached_owner(workers: &ProductionSessionWorkers, id: &str) -> Option<Arc<SandAgentDb>> {
    workers.db_owners.lock().unwrap()
        .get(&ProductionSessionWorkers::native_conversation_owner_key(id)).cloned()
}

#[test]
fn same_pair_waits_before_open_while_other_pair_progresses_and_state_survives_restart() {
    let root = test_root("same-pair");
    let mut workers = test_workers(&root);
    let (locked, first_locked) = mpsc::channel();
    let (resume, released) = mpsc::channel();
    let released = Mutex::new(released);
    let (contending, second_before_lock) = mpsc::channel();
    let (seeded, durable_state) = mpsc::channel();
    let first_pair = Mutex::new(None::<String>);
    let seeded_once = AtomicBool::new(false);
    workers.human_creation_hook = Some(Arc::new(move |stage, id, owner| {
        match stage {
            HumanCreationStage::BeforeLock => {
                if first_pair.lock().unwrap().as_deref() == Some(id) {
                    contending.send(id.to_string()).unwrap();
                }
            }
            HumanCreationStage::Locked => {
                let pause = {
                    let mut first_pair = first_pair.lock().unwrap();
                    if first_pair.is_none() {
                        *first_pair = Some(id.to_string());
                        true
                    } else {
                        false
                    }
                };
                if pause {
                    locked.send(id.to_string()).unwrap();
                    receive(&released.lock().unwrap(), "release first pair before opening");
                }
            }
            HumanCreationStage::BeforePublish => {
                if first_pair.lock().unwrap().as_deref() == Some(id)
                    && !seeded_once.swap(true, Ordering::SeqCst)
                {
                    let owner = owner.unwrap();
                    seed_durable_state(owner);
                    seeded.send((stored_rows(owner.db_path()), Arc::downgrade(owner))).unwrap();
                }
            }
            HumanCreationStage::BeforeInitialize => {}
        }
    }));
    let workers = Arc::new(workers);
    let (first_result, first_task) = spawn_creation(&workers, PEER, "First saved title");
    let id = receive(&first_locked, "first creator acquired the mutation lane");
    let path = get_native_conversation_db_path(&root, &id).unwrap();
    assert!(!path.exists(), "the first creator is paused before any database opening");

    let serial = workers.human_message_lock(&id).unwrap();
    assert!(matches!(serial.try_lock(), Err(std::sync::TryLockError::WouldBlock)));
    let (second_result, second_task) = spawn_creation(&workers, PEER, "Losing title");
    assert_eq!(receive(&second_before_lock, "second creator reached the occupied lane"), id);
    assert!(matches!(second_result.try_recv(), Err(mpsc::TryRecvError::Empty)));

    // This completion is a causal cross-pair progress oracle while the first
    // pair remains held; no sleep or repeated probabilistic collision is used.
    let (other_result, other_task) = spawn_creation(&workers, "human-other", "Other pair");
    let other = finish_creation(other_result, other_task).unwrap();
    assert_ne!(conversation_id(&other), id);
    assert!(!path.exists());
    assert!(matches!(second_result.try_recv(), Err(mpsc::TryRecvError::Empty)));

    resume.send(()).unwrap();
    let first = finish_creation(first_result, first_task).unwrap();
    let second = finish_creation(second_result, second_task).unwrap();
    let (saved, first_owner) = receive(&durable_state, "first creator persisted state before publication");
    assert_eq!(first, second);
    assert_eq!(first["title"], "First saved title");
    let owner = workers.open_human_conversation_db_owner(&id).unwrap();
    assert!(Arc::ptr_eq(&owner, &first_owner.upgrade().unwrap()));
    assert!(!owner.is_closed());
    assert_eq!(live_db_handle_count(&path), 1);
    assert_durable_state(&owner);
    assert_eq!(stored_rows(&path), saved);
    assert_eq!(workers.create_human_conversation(PEER, "Repeated title").unwrap(), first);
    assert!(Arc::ptr_eq(&owner, &cached_owner(&workers, &id).unwrap()));
    assert_eq!(stored_rows(&path), saved);

    workers.shutdown();
    assert_eq!(live_db_handle_count(&path), 0);
    drop(owner);
    drop(workers);
    let restarted = test_workers(&root);
    assert_eq!(restarted.create_human_conversation(PEER, "Restart title").unwrap(), first);
    assert_durable_state(&restarted.open_human_conversation_db_owner(&id).unwrap());
    assert_eq!(stored_rows(&path), saved);
    restarted.shutdown();
    drop(restarted);
    fs::remove_dir_all(root).unwrap();
}

#[derive(Clone, Copy)]
enum InitializerFailure {
    Busy,
    Refused,
    Error,
}

fn assert_initializer_failure_preserves_empty_store(failure: InitializerFailure) {
    let root = test_root("initializer");
    let mut workers = test_workers(&root);
    let (paused, resume) = pause_once(&mut workers, HumanCreationStage::BeforeInitialize);
    let workers = Arc::new(workers);
    let (result, task) = spawn_creation(&workers, PEER, "Uncommitted title");
    let PausedCreation { id, path, candidate } = receive(&paused, "schema open precedes fault injection");
    let owner = candidate.upgrade().unwrap();
    assert_eq!(owner.read_kv("metadata").unwrap(), None);
    drop(owner);

    let sibling = path.parent().unwrap().join("unrelated-resource.bin");
    fs::write(&sibling, b"unrelated canonical-directory bytes").unwrap();
    let preserved = stored_rows(&path);
    let injector = Connection::open(&path).unwrap();
    assert_eq!(injector.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('kv', 'transcript_entries')",
        [], |row| row.get::<_, i64>(0),
    ).unwrap(), 2, "faults must be injected after successful production schema opening");
    match failure {
        InitializerFailure::Busy => injector.execute_batch("BEGIN IMMEDIATE").unwrap(),
        InitializerFailure::Refused => injector.execute_batch(
            "CREATE TRIGGER refuse_human_metadata BEFORE INSERT ON kv
             WHEN NEW.key = 'metadata' BEGIN SELECT RAISE(IGNORE); END;",
        ).unwrap(),
        InitializerFailure::Error => injector.execute_batch(
            "CREATE TRIGGER fail_human_metadata BEFORE INSERT ON kv
             WHEN NEW.key = 'metadata'
             BEGIN SELECT RAISE(ABORT, 'human creation initializer fixture failure'); END;",
        ).unwrap(),
    }
    resume.send(()).unwrap();
    let error = finish_creation(result, task).expect_err("the initializer cannot publish empty metadata");
    match failure {
        InitializerFailure::Busy | InitializerFailure::Refused => {
            assert_eq!(error, "failed to initialize native Human conversation metadata");
        }
        InitializerFailure::Error => assert!(error.contains("human creation initializer fixture failure")),
    }
    assert!(path.is_file());
    assert_eq!(fs::read(&sibling).unwrap(), b"unrelated canonical-directory bytes");
    assert_eq!(stored_rows(&path), preserved);
    assert!(cached_owner(&workers, &id).is_none());
    assert!(candidate.upgrade().is_none(), "the unpublished failing handle is released");
    assert_eq!(live_db_handle_count(&path), 0);

    match failure {
        InitializerFailure::Busy => injector.execute_batch("ROLLBACK").unwrap(),
        InitializerFailure::Refused => injector.execute_batch("DROP TRIGGER refuse_human_metadata").unwrap(),
        InitializerFailure::Error => injector.execute_batch("DROP TRIGGER fail_human_metadata").unwrap(),
    }
    drop(injector);
    // Retry through this exact Session instance even though store.db now exists.
    let retry = workers.create_human_conversation(PEER, "Recovered title").unwrap();
    assert_eq!(conversation_id(&retry), id);
    assert_eq!(retry["title"], "Recovered title");
    let owner = workers.open_human_conversation_db_owner(&id).unwrap();
    assert!(!owner.is_closed());
    assert!(owner.get_transcript_entries().unwrap().is_empty());
    let mut recovered = stored_rows(&path);
    assert!(recovered.kv.iter().any(|(key, _)| key == "metadata"));
    recovered.kv.retain(|(key, _)| key != "metadata");
    assert_eq!(recovered, preserved);
    assert_eq!(fs::read(&sibling).unwrap(), b"unrelated canonical-directory bytes");
    assert_eq!(live_db_handle_count(&path), 1);
    workers.shutdown();
    drop(owner);
    drop(workers);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn busy_initializer_after_schema_open_preserves_directory_and_allows_same_worker_retry() {
    assert_initializer_failure_preserves_empty_store(InitializerFailure::Busy);
}

#[test]
fn refused_initializer_after_schema_open_preserves_directory_and_allows_same_worker_retry() {
    assert_initializer_failure_preserves_empty_store(InitializerFailure::Refused);
}

#[test]
fn storage_error_at_initializer_preserves_directory_and_allows_same_worker_retry() {
    assert_initializer_failure_preserves_empty_store(InitializerFailure::Error);
}

fn write_metadata_json(db: &Connection, metadata: &Value) {
    db.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', lower(hex(?1)))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![metadata.to_string()],
    ).unwrap();
}

#[test]
fn initializer_refusal_reuses_valid_metadata_and_an_already_published_reader_owner() {
    let root = test_root("existing-at-initializer");
    let mut workers = test_workers(&root);
    let (paused, resume) = pause_once(&mut workers, HumanCreationStage::BeforeInitialize);
    let workers = Arc::new(workers);
    let (result, task) = spawn_creation(&workers, PEER, "Losing proposed title");
    let PausedCreation { id, path, candidate } = receive(&paused, "creator saw absent metadata");
    let metadata = json!({
        "conversationKind": "human", "conversationId": id,
        "title": "  Existing title is verbatim  ", "participantIds": [PEER, LOCAL],
        "createdAt": 123.0, "lastActivityAt": 18.0,
        "humanHistoryCoverage": {"afterId": 17, "complete": false},
        "otherMetadata": {"retain": true},
    });
    let injector = Connection::open(&path).unwrap();
    write_metadata_json(&injector, &metadata);
    drop(injector);
    // The actual read-side opener publishes while create is still inside its
    // originally-empty initialization branch. Its initializer must now refuse.
    let winner = workers.open_human_conversation_db_owner(&id).unwrap();
    seed_entries(&winner);
    let saved = stored_rows(&path);
    assert_eq!(live_db_handle_count(&path), 2);
    resume.send(()).unwrap();
    let conversation = finish_creation(result, task).unwrap();
    assert_eq!(conversation, json!({
        "id": id, "kind": "human", "title": metadata["title"],
        "participantIds": [PEER, LOCAL],
    }));
    assert!(Arc::ptr_eq(&winner, &cached_owner(&workers, &id).unwrap()));
    assert!(!winner.is_closed());
    assert!(candidate.upgrade().is_none());
    assert_eq!(live_db_handle_count(&path), 1);
    assert_eq!(winner.read_metadata().unwrap(), metadata);
    assert_durable_state(&winner);
    assert_eq!(stored_rows(&path), saved);
    assert_eq!(workers.create_human_conversation(PEER, "Another title").unwrap(), conversation);
    assert!(Arc::ptr_eq(&winner, &workers.open_human_conversation_db_owner(&id).unwrap()));
    assert_eq!(stored_rows(&path), saved);
    workers.shutdown();
    drop(winner);
    drop(workers);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reader_published_after_initialization_keeps_its_arc_and_durable_state() {
    let root = test_root("reader-publication");
    let mut workers = test_workers(&root);
    let (paused, resume) = pause_once(&mut workers, HumanCreationStage::BeforePublish);
    let workers = Arc::new(workers);
    let (result, task) = spawn_creation(&workers, PEER, "Initialized title");
    let PausedCreation { id, path, candidate } = receive(&paused, "valid metadata precedes owner publication");
    assert!(cached_owner(&workers, &id).is_none());
    let winner = workers.open_human_conversation_db_owner(&id).unwrap();
    assert!(!Arc::ptr_eq(&candidate.upgrade().unwrap(), &winner));
    seed_durable_state(&winner);
    assert!(winner.set_metadata("title", json!("Reader saved title")).unwrap());
    let saved = stored_rows(&path);
    assert_eq!(live_db_handle_count(&path), 2);
    resume.send(()).unwrap();
    let conversation = finish_creation(result, task).unwrap();
    assert_eq!(conversation["title"], "Reader saved title");
    assert!(Arc::ptr_eq(&winner, &cached_owner(&workers, &id).unwrap()));
    assert!(!winner.is_closed());
    assert!(candidate.upgrade().is_none());
    assert_eq!(live_db_handle_count(&path), 1);
    assert_durable_state(&winner);
    assert_eq!(stored_rows(&path), saved);
    assert_eq!(workers.create_human_conversation(PEER, "Repeated proposal").unwrap(), conversation);
    assert!(Arc::ptr_eq(&winner, &cached_owner(&workers, &id).unwrap()));
    workers.shutdown();
    drop(winner);
    drop(workers);
    let restarted = test_workers(&root);
    assert_eq!(restarted.create_human_conversation(PEER, "Restart proposal").unwrap(), conversation);
    assert_durable_state(&restarted.open_human_conversation_db_owner(&id).unwrap());
    assert_eq!(stored_rows(&path), saved);
    restarted.shutdown();
    drop(restarted);
    fs::remove_dir_all(root).unwrap();
}

fn encoded_json(value: &Value) -> String {
    value.to_string().as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn malformed_or_foreign_metadata_is_preserved_and_rejected_with_or_without_cached_owner() {
    let root = test_root("invalid-metadata");
    let mut workers = test_workers(&root);
    let conversation = workers.create_human_conversation(PEER, "Existing title").unwrap();
    let id = conversation_id(&conversation).to_string();
    let owner = workers.open_human_conversation_db_owner(&id).unwrap();
    let path = owner.db_path().to_path_buf();
    seed_durable_state(&owner);
    let valid = owner.read_metadata().unwrap();
    let sibling = path.parent().unwrap().join("unrelated-resource.bin");
    fs::write(&sibling, b"preserve incompatible metadata and siblings").unwrap();

    // Preserve the established missing-title fallback without filling it in.
    let mut no_title = valid.clone();
    no_title.as_object_mut().unwrap().remove("title");
    assert!(owner.write_kv("metadata", &encoded_json(&no_title)).unwrap());
    let before_fallback = stored_rows(&path);
    let fallback = workers.create_human_conversation(PEER, "Must not be stored").unwrap();
    assert_eq!(fallback["title"], PEER);
    assert_eq!(stored_rows(&path), before_fallback);
    assert!(Arc::ptr_eq(&owner, &cached_owner(&workers, &id).unwrap()));
    drop(owner);

    let mut cases = vec![
        ("invalid hex", "not-hex".to_string()),
        ("non-ASCII hex", "🙂".to_string()),
        ("invalid JSON", "7b".to_string()),
        ("null metadata", encoded_json(&Value::Null)),
        ("array metadata", encoded_json(&json!([]))),
        ("empty object", encoded_json(&json!({}))),
    ];
    for (label, field, value) in [
        ("foreign kind", "conversationKind", json!("agent")),
        ("foreign canonical id", "conversationId", json!("human-direct-foreign")),
        ("missing local", "participantIds", json!(["foreign-local", PEER])),
        ("missing peer", "participantIds", json!([LOCAL, "foreign-peer"])),
        ("extra participant", "participantIds", json!([LOCAL, PEER, "third"])),
        ("non-string participant", "participantIds", json!([LOCAL, 2])),
        ("duplicate participant", "participantIds", json!([LOCAL, LOCAL])),
        ("non-array participants", "participantIds", json!({"local": LOCAL, "peer": PEER})),
        ("non-string title", "title", json!(42)),
    ] {
        let mut metadata = valid.clone();
        metadata[field] = value;
        cases.push((label, encoded_json(&metadata)));
    }
    let mut missing_id = valid.clone();
    missing_id.as_object_mut().unwrap().remove("conversationId");
    cases.push(("missing canonical id", encoded_json(&missing_id)));

    for (label, raw) in cases {
        let injector = Connection::open(&path).unwrap();
        write_metadata_json(&injector, &valid);
        let cached = workers.open_human_conversation_db_owner(&id).unwrap();
        injector.execute("UPDATE kv SET value = ?1 WHERE key = 'metadata'", params![raw]).unwrap();
        drop(injector);
        let saved = stored_rows(&path);
        workers.create_human_conversation(PEER, "Must not overwrite").expect_err(label);
        assert!(Arc::ptr_eq(&cached, &cached_owner(&workers, &id).unwrap()), "{label}");
        assert!(!cached.is_closed(), "{label}");
        assert_eq!(stored_rows(&path), saved, "{label}");
        assert_eq!(fs::read(&sibling).unwrap(), b"preserve incompatible metadata and siblings");
        workers.shutdown();
        drop(cached);
        drop(workers);
        workers = test_workers(&root);
        workers.create_human_conversation(PEER, "Restart must not overwrite").expect_err(label);
        assert!(cached_owner(&workers, &id).is_none(), "{label}");
        assert_eq!(live_db_handle_count(&path), 0, "{label}");
        assert_eq!(stored_rows(&path), saved, "{label}");
        assert_eq!(fs::read(&sibling).unwrap(), b"preserve incompatible metadata and siblings");
    }
    workers.shutdown();
    drop(workers);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn distinct_worker_creators_keep_first_committed_metadata_and_independent_owners() {
    let root = test_root("distinct-workers");
    let mut first_workers = test_workers(&root);
    let (first_paused, first_resume) = pause_once(&mut first_workers, HumanCreationStage::BeforeInitialize);
    let first_workers = Arc::new(first_workers);
    let (first_result, first_task) = spawn_creation(&first_workers, PEER, "First committed title");
    let first = receive(&first_paused, "first worker opened its candidate");

    let mut second_workers = test_workers(&root);
    let (second_paused, second_resume) = pause_once(&mut second_workers, HumanCreationStage::BeforeInitialize);
    let second_workers = Arc::new(second_workers);
    let (second_result, second_task) = spawn_creation(&second_workers, PEER, "Losing worker title");
    let second = receive(&second_paused, "second independent worker also observed absent metadata");
    assert_eq!(first.id, second.id);
    assert_eq!(first.path, second.path);
    assert!(!Arc::ptr_eq(&first.candidate.upgrade().unwrap(), &second.candidate.upgrade().unwrap()));
    assert_eq!(live_db_handle_count(&first.path), 2);

    first_resume.send(()).unwrap();
    let winner = finish_creation(first_result, first_task).unwrap();
    let first_owner = first_workers.open_human_conversation_db_owner(&first.id).unwrap();
    seed_durable_state(&first_owner);
    assert!(first_owner.set_metadata("creationWinner", json!({"preserve": [1, 2, 3]})).unwrap());
    let saved = stored_rows(&first.path);
    second_resume.send(()).unwrap();
    let reused = finish_creation(second_result, second_task).unwrap();
    assert_eq!(winner, reused);
    assert_eq!(reused["title"], "First committed title");
    let second_owner = second_workers.open_human_conversation_db_owner(&second.id).unwrap();
    assert!(!Arc::ptr_eq(&first_owner, &second_owner));
    assert_eq!(second_owner.read_metadata().unwrap(), first_owner.read_metadata().unwrap());
    assert_durable_state(&second_owner);
    assert_eq!(stored_rows(&first.path), saved);

    first_workers.shutdown();
    drop(first_owner);
    drop(first_workers);
    assert!(!second_owner.is_closed(), "closing one worker cannot close another worker's owner");
    assert_eq!(live_db_handle_count(&second.path), 1);
    assert_eq!(second_workers.create_human_conversation(PEER, "Repeated title").unwrap(), winner);
    assert_eq!(stored_rows(&second.path), saved);
    second_workers.shutdown();
    drop(second_owner);
    drop(second_workers);
    let restarted = test_workers(&root);
    assert_eq!(restarted.create_human_conversation(PEER, "Restart title").unwrap(), winner);
    assert_eq!(stored_rows(&second.path), saved);
    restarted.shutdown();
    drop(restarted);
    fs::remove_dir_all(root).unwrap();
}

fn insert_identityless_content(path: &Path, kind: &str) {
    let db = Connection::open(path).unwrap();
    match kind {
        "kv" => {
            db.execute("INSERT INTO kv (key, value) VALUES ('foreign-durable-state', 'retain exactly')", []).unwrap();
        }
        "transcript" => {
            db.execute(
                "INSERT INTO transcript_entries (id, entry) VALUES ('identityless-message', ?1)",
                params![json!({"id":"identityless-message", "kind":"message", "role":"user", "content":"retain without adoption"}).to_string()],
            ).unwrap();
        }
        "blob" => {
            db.execute("INSERT INTO blobs (id, data) VALUES ('identityless-blob', ?1)", params![b"retained blob bytes".as_slice()]).unwrap();
        }
        _ => panic!("unknown retained content case"),
    }
}

#[test]
fn first_initializer_refuses_identityless_kv_transcript_or_blob_without_adoption() {
    for kind in ["kv", "transcript", "blob"] {
        let root = test_root(kind);
        let mut workers = test_workers(&root);
        let (paused, resume) = pause_once(&mut workers, HumanCreationStage::BeforeInitialize);
        let workers = Arc::new(workers);
        let (result, task) = spawn_creation(&workers, PEER, "Must not adopt foreign contents");
        let PausedCreation { id, path, candidate } = receive(&paused, "empty metadata read before retained data appears");
        insert_identityless_content(&path, kind);
        let sibling = path.parent().unwrap().join("retained-resource.bin");
        fs::write(&sibling, b"retained outside the identityless database").unwrap();
        let saved = stored_rows(&path);
        resume.send(()).unwrap();
        let error = finish_creation(result, task).expect_err("populated schema is not an empty interrupted creation");
        assert_eq!(error, "failed to initialize native Human conversation metadata", "{kind}");
        assert!(cached_owner(&workers, &id).is_none());
        assert!(candidate.upgrade().is_none());
        assert_eq!(live_db_handle_count(&path), 0);
        assert_eq!(stored_rows(&path), saved);
        workers.create_human_conversation(PEER, "Retry must not adopt").expect_err(kind);
        assert_eq!(stored_rows(&path), saved);
        workers.shutdown();
        drop(workers);
        let restarted = test_workers(&root);
        restarted.create_human_conversation(PEER, "Reopen must not adopt").expect_err(kind);
        assert!(cached_owner(&restarted, &id).is_none());
        assert_eq!(stored_rows(&path), saved);
        assert_eq!(fs::read(&sibling).unwrap(), b"retained outside the identityless database");
        restarted.shutdown();
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn cached_owner_with_missing_identity_never_relabels_retained_data() {
    for kind in ["kv", "transcript", "blob"] {
        let root = test_root("cached-missing-identity");
        let workers = test_workers(&root);
        let conversation = workers.create_human_conversation(PEER, "Original identity").unwrap();
        let id = conversation_id(&conversation).to_string();
        let owner = workers.open_human_conversation_db_owner(&id).unwrap();
        let path = owner.db_path().to_path_buf();
        insert_identityless_content(&path, kind);
        assert!(owner.delete_kv("metadata").unwrap());
        let saved = stored_rows(&path);
        workers.create_human_conversation(PEER, "Must not reconstruct identity over data").expect_err(kind);
        assert!(Arc::ptr_eq(&owner, &cached_owner(&workers, &id).unwrap()));
        assert!(!owner.is_closed());
        assert_eq!(stored_rows(&path), saved);
        workers.shutdown();
        drop(owner);
        drop(workers);
        let restarted = test_workers(&root);
        restarted.create_human_conversation(PEER, "Restart must not adopt").expect_err(kind);
        assert!(cached_owner(&restarted, &id).is_none());
        assert_eq!(stored_rows(&path), saved);
        restarted.shutdown();
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn existing_empty_directory_and_schema_only_store_resume_without_replacing_files() {
    for retained_store in ["empty-directory", "zero-length-file", "schema-only"] {
        let root = test_root(&format!("empty-restart-{retained_store}"));
        let mut workers = test_workers(&root);
        let (locked, observed) = mpsc::channel();
        let (resume, released) = mpsc::channel();
        let released = Mutex::new(released);
        workers.human_creation_hook = Some(Arc::new(move |stage, id, _owner| {
            if stage == HumanCreationStage::Locked {
                locked.send(id.to_string()).unwrap();
                receive(&released.lock().unwrap(), "allow empty directory discovery");
            }
        }));
        let workers = Arc::new(workers);
        let (result, task) = spawn_creation(&workers, PEER, "Recovered empty store");
        let id = receive(&observed, "canonical identity before path opening");
        let path = get_native_conversation_db_path(&root, &id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let sibling = path.parent().unwrap().join("retained-empty-directory.bin");
        fs::write(&sibling, b"existing directory sentinel").unwrap();
        if retained_store == "schema-only" {
            let previous = SandAgentDb::open_conversation_store(&path, 25).unwrap();
            assert_eq!(previous.read_kv("metadata").unwrap(), None);
            drop(previous);
            assert_eq!(live_db_handle_count(&path), 0);
        } else if retained_store == "zero-length-file" {
            fs::write(&path, b"").unwrap();
            assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        }
        resume.send(()).unwrap();
        let first = finish_creation(result, task).unwrap();
        let saved = stored_rows(&path);
        assert_eq!(fs::read(&sibling).unwrap(), b"existing directory sentinel");
        workers.shutdown();
        drop(workers);
        let restarted = test_workers(&root);
        assert_eq!(restarted.create_human_conversation(PEER, "Must keep saved title").unwrap(), first);
        assert_eq!(stored_rows(&path), saved);
        assert_eq!(fs::read(&sibling).unwrap(), b"existing directory sentinel");
        restarted.shutdown();
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

fn directory_file_bytes(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = fs::read_dir(path).unwrap().map(|entry| {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file(), "creation must not add a recovery directory");
        (entry.file_name().to_string_lossy().into_owned(), fs::read(entry.path()).unwrap())
    }).collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn corrupt_or_notadb_creation_preserves_original_database_sidecars_and_siblings() {
    for (corruption, retained_header_length, orphan_sidecar) in [
        ("notadb", None, None),
        ("short-header", Some(6), None),
        ("truncated-header", None, None),
        ("notadb-full-header", None, None),
        ("schema", None, None),
        ("data-page-delete-mode", None, None),
        ("short-header-1", Some(1), None),
        ("short-header-15", Some(15), None),
        ("truncated-header-16", Some(16), None),
        ("truncated-header-99", Some(99), None),
        ("missing-main-sidecars", None, None),
        ("empty-main-sidecars", None, None),
        ("missing-main-sidecars", None, Some(("-wal", false))),
        ("missing-main-sidecars", None, Some(("-shm", false))),
        ("missing-main-sidecars", None, Some(("-journal", false))),
        ("missing-main-sidecars", None, Some(("-wal", true))),
        ("missing-main-sidecars", None, Some(("-shm", true))),
        ("missing-main-sidecars", None, Some(("-journal", true))),
        ("empty-main-sidecars", None, Some(("-wal", false))),
        ("empty-main-sidecars", None, Some(("-shm", false))),
        ("empty-main-sidecars", None, Some(("-journal", false))),
        ("empty-main-sidecars", None, Some(("-wal", true))),
        ("empty-main-sidecars", None, Some(("-shm", true))),
        ("empty-main-sidecars", None, Some(("-journal", true))),
    ] {
        let root = test_root("corrupt-store-refusal");
        let mut workers = test_workers(&root);
        let (locked, observed) = mpsc::channel();
        let (resume, released) = mpsc::channel();
        let released = Mutex::new(released);
        let used = AtomicBool::new(false);
        workers.human_creation_hook = Some(Arc::new(move |stage, id, _| {
            if stage == HumanCreationStage::Locked && !used.swap(true, Ordering::SeqCst) {
                locked.send(id.to_string()).unwrap();
                receive(&released.lock().unwrap(), "allow creation to encounter the retained corrupt store");
            }
        }));
        let workers = Arc::new(workers);
        let (result, task) = spawn_creation(&workers, PEER, "Must not reset corrupted identity");
        let id = receive(&observed, "creation stopped before opening any SQLite handle");
        let path = get_native_conversation_db_path(&root, &id).unwrap();
        let directory = path.parent().unwrap();
        fs::create_dir_all(directory).unwrap();
        if matches!(corruption, "schema" | "data-page-delete-mode") {
            let previous = SandAgentDb::open_conversation_store(&path, 25).unwrap();
            seed_entries(&previous);
            previous.close(true);
            drop(previous);
            let damage_offset = if corruption == "data-page-delete-mode" {
                // A readable schema is not enough: journal_mode=WAL would
                // change this main-file header before discovering a bad data page.
                let db = Connection::open(&path).unwrap();
                let mode: String = db.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0)).unwrap();
                assert_eq!(mode, "delete");
                let page_size: i64 = db.query_row("PRAGMA page_size", [], |row| row.get(0)).unwrap();
                let root_page: i64 = db.query_row(
                    "SELECT rootpage FROM sqlite_schema WHERE name='transcript_entries'",
                    [], |row| row.get(0),
                ).unwrap();
                assert!(page_size > 0);
                assert!(root_page > 1);
                drop(db);
                usize::try_from(root_page - 1).unwrap()
                    .checked_mul(usize::try_from(page_size).unwrap()).unwrap()
            } else { 100 };
            let mut bytes = fs::read(&path).unwrap();
            assert!(bytes.len() > damage_offset);
            assert_eq!(&bytes[..16], b"SQLite format 3\0");
            bytes[damage_offset] = 0; // Invalid B-tree type, with a valid SQLite header.
            fs::write(&path, bytes).unwrap();
        } else {
            let retained_bytes = if let Some(length) = retained_header_length {
                let mut bytes = b"SQLite format 3\0".to_vec();
                bytes.resize(length, 0);
                bytes
            } else if corruption == "truncated-header" {
                // Valid magic must not admit an incomplete main-file header.
                b"SQLite format 3\0retained truncated database header".to_vec()
            } else if corruption == "notadb-full-header" {
                // Full length alone must not admit a non-SQLite main file.
                vec![b'X'; 100]
            } else {
                b"retained NOTADB bytes; never quarantine or replace".to_vec()
            };
            if corruption == "empty-main-sidecars" {
                fs::write(&path, b"").unwrap();
            } else if corruption != "missing-main-sidecars" {
                fs::write(&path, retained_bytes).unwrap();
            }
            for suffix in ["-wal", "-shm", "-journal"] {
                if orphan_sidecar.is_some_and(|(selected, _)| selected != suffix) { continue; }
                let bytes = if orphan_sidecar.is_some_and(|(_, empty)| empty) {
                    Vec::new()
                } else {
                    format!("retained original {suffix} bytes").into_bytes()
                };
                fs::write(PathBuf::from(format!("{}{suffix}", path.display())), bytes).unwrap();
            }
        }
        fs::write(directory.join("unrelated-resource.bin"), b"retained resource outside corrupt SQLite").unwrap();
        let saved = directory_file_bytes(directory);
        resume.send(()).unwrap();
        let error = finish_creation(result, task).expect_err("creation cannot recover a corrupt store into a new identity");
        assert!(!error.is_empty());
        assert!(cached_owner(&workers, &id).is_none());
        assert_eq!(live_db_handle_count(&path), 0);
        assert_eq!(directory_file_bytes(directory), saved, "{corruption}: original files must survive refusal");
        if corruption == "missing-main-sidecars" { assert!(!path.exists()); }
        workers.create_human_conversation(PEER, "Retry must preserve corrupt data").expect_err("same-worker refusal");
        assert_eq!(directory_file_bytes(directory), saved, "{corruption}: original files must survive refusal");
        if corruption == "missing-main-sidecars" { assert!(!path.exists()); }
        workers.shutdown();
        drop(workers);
        let restarted = test_workers(&root);
        restarted.create_human_conversation(PEER, "Reopen must preserve corrupt data").expect_err("reopened refusal");
        assert!(cached_owner(&restarted, &id).is_none());
        assert_eq!(live_db_handle_count(&path), 0);
        assert_eq!(directory_file_bytes(directory), saved, "{corruption}: original files must survive refusal");
        if corruption == "missing-main-sidecars" { assert!(!path.exists()); }
        restarted.shutdown();
        drop(restarted);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn retained_wal_creation_keeps_committed_identity_and_transcript_after_reopen() {
    let root = test_root("retained-wal");
    let workers = test_workers(&root);
    let original = workers.create_human_conversation(PEER, "Checkpointed original title").unwrap();
    let id = conversation_id(&original).to_string();
    let owner = workers.open_human_conversation_db_owner(&id).unwrap();
    let path = owner.db_path().to_path_buf();
    let original_raw_metadata = owner.read_kv("metadata").unwrap().unwrap();
    let mut retained_metadata = owner.read_metadata().unwrap();
    retained_metadata["title"] = json!("Committed only in the retained WAL");
    owner.close(true);
    workers.shutdown();
    drop(owner);
    drop(workers);
    assert_eq!(live_db_handle_count(&path), 0);

    let writer = Connection::open(&path).unwrap();
    let mode: String = writer.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "wal");
    writer.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
    assert!(writer.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true,
    ).unwrap());
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    write_metadata_json(&writer, &retained_metadata);
    for entry in persisted_entries() {
        writer.execute(
            "INSERT INTO transcript_entries (id, entry) VALUES (?1, ?2)",
            params![entry["id"].as_str().unwrap(), entry.to_string()],
        ).unwrap();
    }
    writer.execute_batch("COMMIT").unwrap();
    let saved = stored_rows(&path);
    drop(writer);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    assert!(fs::metadata(&wal).unwrap().len() > 32, "committed frames must remain after the last writer closes");

    // Prove that the main file alone is not the current identity/transcript.
    // SQLite owns this probe's descriptor just as it does in production.
    let mut uri = url::Url::from_file_path(&path).unwrap();
    uri.query_pairs_mut().append_pair("mode", "ro").append_pair("immutable", "1");
    let main_only = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    ).unwrap();
    let main_metadata: String = main_only.query_row(
        "SELECT value FROM kv WHERE key='metadata'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(main_metadata, original_raw_metadata);
    assert_ne!(main_metadata, encoded_json(&retained_metadata));
    let main_entries: i64 = main_only.query_row(
        "SELECT COUNT(*) FROM transcript_entries", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(main_entries, 0);
    drop(main_only);
    assert!(fs::metadata(&wal).unwrap().len() > 32);

    let reopened = test_workers(&root);
    let conversation = reopened.create_human_conversation(PEER, "Must not replace WAL identity").unwrap();
    assert_eq!(conversation["id"], id);
    assert_eq!(conversation["title"], retained_metadata["title"]);
    let owner = reopened.open_human_conversation_db_owner(&id).unwrap();
    assert_eq!(owner.read_metadata().unwrap(), retained_metadata);
    assert_eq!(owner.get_transcript_entries().unwrap(), persisted_entries());
    assert_eq!(stored_rows(&path), saved);
    reopened.shutdown();
    drop(owner);
    drop(reopened);

    let restarted = test_workers(&root);
    assert_eq!(restarted.create_human_conversation(PEER, "Must retain reopened identity").unwrap(), conversation);
    assert_eq!(stored_rows(&path), saved);
    restarted.shutdown();
    drop(restarted);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn human_format_probe_preserves_posix_locks_against_an_external_writer() {
    const CHILD_DB: &str = "FBCP_HUMAN_FORMAT_LOCK_CHILD_DB";
    const CHILD_EXPECTATION: &str = "FBCP_HUMAN_FORMAT_LOCK_CHILD_EXPECTATION";
    const CASE: &str = "extensions::session::production::human_creation_tests::human_format_probe_preserves_posix_locks_against_an_external_writer";

    if let Some(path) = std::env::var_os(CHILD_DB) {
        let expectation = std::env::var(CHILD_EXPECTATION).unwrap();
        let db = Connection::open_with_flags(
            PathBuf::from(path),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
        ).unwrap();
        db.busy_timeout(Duration::ZERO).unwrap();
        let result = db.execute("UPDATE probe_lock_guard SET value=value+1 WHERE id=1", []);
        match expectation.as_str() {
            "blocked" => {
                let error = result.expect_err("an independent process must not write through the held SQLite lock");
                assert!(crate::storage::sqlite_busy::is_sqlite_busy_error(&error), "{error}");
                let value: i64 = db.query_row(
                    "SELECT value FROM probe_lock_guard WHERE id=1", [], |row| row.get(0),
                ).unwrap();
                assert_eq!(value, 0);
            }
            "writable" => {
                assert_eq!(result.unwrap(), 1);
                let value: i64 = db.query_row(
                    "SELECT value FROM probe_lock_guard WHERE id=1", [], |row| row.get(0),
                ).unwrap();
                assert_eq!(value, 1);
            }
            _ => panic!("unexpected child lock expectation"),
        }
        println!("human-format-lock-child:{expectation}:verified");
        return;
    }

    let root = test_root("external-lock");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("store.db");
    let db = Connection::open(&path).unwrap();
    let mode: String = db.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0)).unwrap();
    assert_eq!(mode, "delete");
    db.execute_batch(
        "CREATE TABLE probe_lock_guard (id INTEGER PRIMARY KEY, value INTEGER NOT NULL);
         INSERT INTO probe_lock_guard (id, value) VALUES (1, 0);
         BEGIN IMMEDIATE;
         UPDATE probe_lock_guard SET value=99 WHERE id=1;",
    ).unwrap();
    let run_child = |expectation: &str| {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
            .env(CHILD_DB, &path)
            .env(CHILD_EXPECTATION, expectation)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "child {expectation} failed:\n{stdout}\n{stderr}");
        assert!(stdout.contains("1 passed; 0 failed; 0 ignored;"), "{stdout}");
        assert!(stdout.contains(&format!("human-format-lock-child:{expectation}:verified")), "{stdout}");
    };

    // The baseline and positive control distinguish a real held OS lock from
    // an unavailable database or a child that never executed the exact case.
    run_child("blocked");
    super::super::agent_db_recovery::probe_human_conversation_store(&path, 25).unwrap();
    run_child("blocked");
    let held_value: i64 = db.query_row(
        "SELECT value FROM probe_lock_guard WHERE id=1", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(held_value, 99);
    db.execute_batch("ROLLBACK").unwrap();
    run_child("writable");
    let final_value: i64 = db.query_row(
        "SELECT value FROM probe_lock_guard WHERE id=1", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(final_value, 1);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

