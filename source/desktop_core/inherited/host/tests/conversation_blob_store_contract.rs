use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use mahayana_host_runtime::agent_isolation::{
    open_configured_conversation_blob_db, read_conversation_blob_migration_state,
    AgentWorkerPool, ConversationBlobMigrationState, ConversationBlobWorkerBackend,
    ConversationGarbageCollectionOutcome, LegacyBlobRetirementVerdict, WorkerBlobStore,
    CONVERSATION_BLOB_SCHEMA,
};
use rusqlite::params;
use sha2::{Digest, Sha256};
use uuid::Uuid;

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on_ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("test future unexpectedly pending"),
    }
}

fn test_db_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fabushi-{label}-{}.sqlite",
        Uuid::new_v4()
    ))
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

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn push_length_delimited(field_number: u64, value: &[u8], output: &mut Vec<u8>) {
    encode_varint((field_number << 3) | 2, output);
    encode_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn root_bytes(turn_id: &[u8], prompt_marker: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    push_length_delimited(1, prompt_marker, &mut output);
    push_length_delimited(8, turn_id, &mut output);
    output
}

fn digest_id(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

#[test]
fn production_worker_backend_persists_blobs_finds_roots_and_closes_sqlite_store() {
    let path = test_db_path("conversation-worker");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::new(5_000)));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-a", &path, None);
    let ctx = ();

    let turn_id = vec![0x11; 32];
    block_on_ready(store.set_blob(&ctx, &turn_id, b"turn-data")).expect("set turn");

    let root = root_bytes(&turn_id, b"root-prompt");
    let root_id = digest_id(&root);
    block_on_ready(store.set_blob(&ctx, &root_id, &root)).expect("set root");

    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &root_id)).expect("get root"),
        Some(root.clone())
    );
    assert_eq!(
        block_on_ready(pool.find_latest_root_blob_id("agent-a", &path, None))
            .expect("find latest root"),
        Some(root_id.clone())
    );
    assert_eq!(
        block_on_ready(pool.collect_conversation_garbage(
            "agent-a",
            &path,
            &to_hex(&root_id),
            60_000,
            None,
        ))
        .expect("gc result"),
        ConversationGarbageCollectionOutcome::Skipped {
            reason: "unresolved-refs".into(),
            unresolved_proto_refs: 1,
        }
    );

    assert_eq!(pool.active_worker_count(), 1);
    assert_eq!(pool.backend().active_store_count(), 1);
    block_on_ready(pool.close_store(&path));
    assert_eq!(pool.active_worker_count(), 0);
    assert_eq!(pool.backend().active_store_count(), 0);

    cleanup(&path);
}

#[test]
fn production_worker_backend_clears_sha_valid_stale_checkpoint_roots() {
    let path = test_db_path("conversation-stale-root");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-b", &path, None);
    let ctx = ();

    let turn_id = vec![0x22; 32];
    block_on_ready(store.set_blob(&ctx, &turn_id, b"turn-data")).expect("set turn");

    let retained_root = root_bytes(&turn_id, b"retained");
    let retained_id = digest_id(&retained_root);
    let stale_root = root_bytes(&turn_id, b"stale");
    let stale_id = digest_id(&stale_root);
    block_on_ready(store.set_blob(&ctx, &retained_id, &retained_root)).expect("set retained");
    block_on_ready(store.set_blob(&ctx, &stale_id, &stale_root)).expect("set stale");

    assert_eq!(
        block_on_ready(pool.clear_stale_checkpoint_roots(
            "agent-b",
            &path,
            &to_hex(&retained_id),
            None,
        ))
        .expect("clear stale roots"),
        1
    );
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &stale_id)).expect("read stale"),
        None
    );
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &retained_id)).expect("read retained"),
        Some(retained_root)
    );

    block_on_ready(pool.close_store(&path));
    cleanup(&path);
}

#[test]
fn production_worker_backend_collects_unreachable_blobs_from_generated_proto_metadata() {
    let path = test_db_path("conversation-gc");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-gc", &path, None);
    let ctx = ();

    let reachable_data = b"reachable-json".to_vec();
    let reachable_id = digest_id(&reachable_data);
    block_on_ready(store.set_blob(&ctx, &reachable_id, &reachable_data))
        .expect("set reachable leaf");

    let orphan_data = b"orphan".to_vec();
    let orphan_id = digest_id(&orphan_data);
    block_on_ready(store.set_blob(&ctx, &orphan_id, &orphan_data)).expect("set orphan");

    let mut root = Vec::new();
    push_length_delimited(1, &reachable_id, &mut root);
    let root_id = digest_id(&root);
    block_on_ready(store.set_blob(&ctx, &root_id, &root)).expect("set root");

    let outcome = block_on_ready(pool.collect_conversation_garbage(
        "agent-gc",
        &path,
        &to_hex(&root_id),
        0,
        None,
    ))
    .expect("collect garbage");
    match outcome {
        ConversationGarbageCollectionOutcome::Collected {
            deleted_rows,
            retained_pending_rows,
            ..
        } => {
            assert_eq!(deleted_rows, 1);
            assert_eq!(retained_pending_rows, 0);
        }
        other => panic!("expected collected outcome, got {other:?}"),
    }

    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &root_id)).expect("root retained"),
        Some(root)
    );
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &reachable_id)).expect("reachable retained"),
        Some(reachable_data)
    );
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &orphan_id)).expect("orphan collected"),
        None
    );

    block_on_ready(pool.close_store(&path));
    cleanup(&path);
}

#[test]
fn production_worker_backend_preserves_fail_closed_gc_on_unresolved_proto_refs() {
    let path = test_db_path("conversation-gc-unresolved");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-gc-unresolved", &path, None);
    let ctx = ();

    let turn_id = vec![0x42; 32];
    block_on_ready(store.set_blob(&ctx, &turn_id, b"not-a-conversation-turn"))
        .expect("set malformed proto referent");
    let orphan_id = digest_id(b"must-survive-skipped-gc");
    block_on_ready(store.set_blob(&ctx, &orphan_id, b"must-survive-skipped-gc"))
        .expect("set orphan");

    let mut root = Vec::new();
    push_length_delimited(8, &turn_id, &mut root);
    let root_id = digest_id(&root);
    block_on_ready(store.set_blob(&ctx, &root_id, &root)).expect("set root");

    assert_eq!(
        block_on_ready(pool.collect_conversation_garbage(
            "agent-gc-unresolved",
            &path,
            &to_hex(&root_id),
            0,
            None,
        ))
        .expect("gc result"),
        ConversationGarbageCollectionOutcome::Skipped {
            reason: "unresolved-refs".into(),
            unresolved_proto_refs: 1,
        }
    );
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &orphan_id)).expect("skipped gc keeps orphan"),
        Some(b"must-survive-skipped-gc".to_vec())
    );

    block_on_ready(pool.close_store(&path));
    cleanup(&path);
}

#[test]
fn production_worker_backend_retains_recent_pending_writes_until_retention_floor() {
    let path = test_db_path("conversation-gc-pending-write");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-gc-pending", &path, None);
    let ctx = ();

    let root = Vec::new();
    let root_id = digest_id(&root);
    block_on_ready(store.set_blob(&ctx, &root_id, &root)).expect("set root");

    let pending_data = b"recent-pending-write".to_vec();
    let pending_id = digest_id(&pending_data);
    block_on_ready(store.set_blob(&ctx, &pending_id, &pending_data)).expect("set pending");

    let first = block_on_ready(pool.collect_conversation_garbage(
        "agent-gc-pending",
        &path,
        &to_hex(&root_id),
        60_000,
        None,
    ))
    .expect("collect with retention");
    assert!(matches!(
        first,
        ConversationGarbageCollectionOutcome::Collected {
            deleted_rows: 0,
            retained_pending_rows: 1,
            ..
        }
    ));
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &pending_id)).expect("pending retained"),
        Some(pending_data.clone())
    );

    let second = block_on_ready(pool.collect_conversation_garbage(
        "agent-gc-pending",
        &path,
        &to_hex(&root_id),
        0,
        None,
    ))
    .expect("collect after retention");
    assert!(matches!(
        second,
        ConversationGarbageCollectionOutcome::Collected {
            deleted_rows: 1,
            retained_pending_rows: 0,
            ..
        }
    ));
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &pending_id)).expect("pending collected"),
        None
    );

    block_on_ready(pool.close_store(&path));
    cleanup(&path);
}

#[test]
fn production_worker_backend_does_not_retain_intentionally_unwalked_summary_archive_edges() {
    let path = test_db_path("conversation-gc-summary-archive");
    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(Arc::clone(&pool), "agent-gc-summary", &path, None);
    let ctx = ();

    let archive_data = b"collectable-summary-archive".to_vec();
    let archive_id = digest_id(&archive_data);
    block_on_ready(store.set_blob(&ctx, &archive_id, &archive_data)).expect("set archive");

    let mut root = Vec::new();
    push_length_delimited(11, &archive_id, &mut root);
    let root_id = digest_id(&root);
    block_on_ready(store.set_blob(&ctx, &root_id, &root)).expect("set root");

    let outcome = block_on_ready(pool.collect_conversation_garbage(
        "agent-gc-summary",
        &path,
        &to_hex(&root_id),
        0,
        None,
    ))
    .expect("collect summary archive");
    assert!(matches!(
        outcome,
        ConversationGarbageCollectionOutcome::Collected {
            deleted_rows: 1,
            ..
        }
    ));
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &archive_id)).expect("archive collected"),
        None
    );

    block_on_ready(pool.close_store(&path));
    cleanup(&path);
}

#[test]
fn production_worker_backend_adopts_legacy_blobs_and_proves_retirement_gate() {
    let path = test_db_path("conversation-adoption");
    let legacy = test_db_path("conversation-legacy");

    {
        let db = rusqlite::Connection::open(&legacy).expect("open legacy");
        db.execute_batch(CONVERSATION_BLOB_SCHEMA)
            .expect("legacy schema");
        db.execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
            params!["aa", b"legacy".as_slice()],
        )
        .expect("legacy row");
    }

    let pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(
        Arc::clone(&pool),
        "agent-c",
        &path,
        Some(legacy.clone()),
    );
    let ctx = ();

    assert_eq!(
        block_on_ready(store.get_blob(&ctx, &[0xaa])).expect("adopted read"),
        Some(b"legacy".to_vec())
    );
    assert_eq!(
        block_on_ready(pool.verify_legacy_blob_retirement(
            "agent-c",
            &path,
            "aa",
            &legacy,
        ))
        .expect("legacy retirement"),
        LegacyBlobRetirementVerdict::retirable(1, 6)
    );

    block_on_ready(pool.close_store(&path));
    let db = open_configured_conversation_blob_db(&path, 5_000).expect("open adopted db");
    assert_eq!(
        read_conversation_blob_migration_state(&db).expect("migration state"),
        ConversationBlobMigrationState::AdoptionComplete
    );
    drop(db);

    cleanup(&path);
    cleanup(&legacy);
}
