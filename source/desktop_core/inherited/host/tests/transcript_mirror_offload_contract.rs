use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agent_isolation::{
    AgentWorkerPool, ConversationBlobWorkerBackend, OffloadingTranscriptMirror,
    OffloadingTranscriptMirrorOptions, TranscriptMirrorOffloadPool,
    TranscriptMirrorOffloadPoolOptions, TranscriptMirrorWorkerFn, TranscriptMirrorWorkerJob,
    WorkerBlobStore,
};
use mahayana_host_runtime::transcript_mirror::legacy_transcript_mirror::{
    LegacyTranscriptBlobStore, LegacyTranscriptState, count_transcript_message_lines,
};

fn job(conversation_id: &str, marker: u8) -> TranscriptMirrorWorkerJob {
    TranscriptMirrorWorkerJob {
        conversation_id: conversation_id.to_string(),
        state_blob_id: vec![marker],
        blob_db_paths: vec![PathBuf::from("conversation-blobs.db")],
        transcripts_dir: PathBuf::from("transcripts"),
    }
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not become true before timeout");
}

#[test]
fn worker_sharding_matches_frozen_js_utf16_hashing() {
    let worker: TranscriptMirrorWorkerFn = Arc::new(|_| Ok(true));
    let pool = TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 3 },
        worker,
    );

    assert_eq!(pool.worker_index_for("agent-a"), 0);
    assert_eq!(pool.worker_index_for("agent-b"), 2);
    assert_eq!(pool.worker_index_for("conversation-1"), 2);
    assert_eq!(pool.worker_index_for("conversation-2"), 1);
    assert_eq!(pool.worker_index_for("😀"), 1);
    pool.close_all();
}

#[test]
fn running_conversation_coalesces_to_latest_job_and_resolves_all_waiters() {
    let calls = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));

    let worker: TranscriptMirrorWorkerFn = {
        let calls = Arc::clone(&calls);
        let release_rx = Arc::clone(&release_rx);
        Arc::new(move |job| {
            let marker = *job.state_blob_id.first().unwrap_or(&0);
            calls.lock().expect("calls").push(marker);
            if marker == 1 {
                entered_tx.send(()).expect("entered");
                release_rx
                    .lock()
                    .expect("release receiver")
                    .recv()
                    .expect("release");
            }
            Ok(true)
        })
    };
    let pool = Arc::new(TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 1 },
        worker,
    ));

    let first_pool = Arc::clone(&pool);
    let first = thread::spawn(move || first_pool.write(job("conversation-a", 1)));
    entered_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("first job entered");

    let second_pool = Arc::clone(&pool);
    let second = thread::spawn(move || second_pool.write(job("conversation-a", 2)));
    wait_until(Duration::from_secs(3), || {
        pool.queued_waiter_count("conversation-a") == 1
    });

    let third_pool = Arc::clone(&pool);
    let third = thread::spawn(move || third_pool.write(job("conversation-a", 3)));
    wait_until(Duration::from_secs(3), || {
        pool.queued_waiter_count("conversation-a") == 2
    });

    release_tx.send(()).expect("release first");
    assert_eq!(first.join().expect("first join"), Ok(true));
    assert_eq!(second.join().expect("second join"), Ok(true));
    assert_eq!(third.join().expect("third join"), Ok(true));
    wait_until(Duration::from_secs(3), || pool.active_lane_count() == 0);

    assert_eq!(*calls.lock().expect("calls"), vec![1, 3]);
    assert_eq!(pool.active_worker_count(), 1);
    pool.close_all();
}

#[test]
fn different_shards_own_independent_long_lived_workers() {
    let worker: TranscriptMirrorWorkerFn = Arc::new(|_| Ok(true));
    let pool = TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 2 },
        worker,
    );

    assert_eq!(pool.write(job("agent-a", 1)), Ok(true));
    assert_eq!(pool.write(job("agent-b", 2)), Ok(true));
    assert_eq!(pool.active_worker_count(), 2);
    pool.close_all();
    assert_eq!(pool.active_worker_count(), 0);
}

#[test]
fn closed_pool_rejects_new_writes() {
    let worker: TranscriptMirrorWorkerFn = Arc::new(|_| Ok(true));
    let pool = TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 1 },
        worker,
    );
    pool.close_all();
    assert_eq!(
        pool.write(job("conversation-a", 1)),
        Err("mirror offload pool is closed".to_string())
    );
}


#[derive(Default)]
struct InlineMemoryBlobs {
    values: HashMap<Vec<u8>, Vec<u8>>,
}

impl LegacyTranscriptBlobStore for InlineMemoryBlobs {
    fn get_blob(&self, blob_id: &[u8]) -> Result<Option<Vec<u8>>, String> {
        Ok(self.values.get(blob_id).cloned())
    }
}

fn temp_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-transcript-offload-{label}-{}-{nanos}",
        std::process::id()
    ))
}

#[test]
fn wrapper_prefers_incremental_and_falls_back_to_full_for_inline_store() {
    let root = temp_root("inline");
    let pool = Arc::new(TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 1 },
        Arc::new(|_| panic!("inline fallback must not call offload worker")),
    ));
    let mirror = OffloadingTranscriptMirror::for_transcripts_dir(
        Arc::clone(&pool),
        OffloadingTranscriptMirrorOptions {
            blob_db_paths: vec![],
            transcripts_dir: root.clone(),
        },
        0,
    );

    let first = vec![1];
    let second = vec![2];
    let mut blobs = InlineMemoryBlobs::default();
    blobs.values.insert(
        first.clone(),
        br#"{"role":"user","content":"one"}"#.to_vec(),
    );
    blobs.values.insert(
        second.clone(),
        br#"{"role":"assistant","content":"two"}"#.to_vec(),
    );

    mirror
        .write_inline(
            "agent-inline",
            &LegacyTranscriptState {
                root_prompt_messages_json: vec![first.clone()],
                ..LegacyTranscriptState::default()
            },
            &blobs,
        )
        .expect("initial full fallback");
    assert_eq!(mirror.previous_root_prompt_count(), 1);

    mirror
        .write_inline(
            "agent-inline",
            &LegacyTranscriptState {
                root_prompt_messages_json: vec![first, second],
                ..LegacyTranscriptState::default()
            },
            &blobs,
        )
        .expect("incremental append");
    assert_eq!(mirror.previous_root_prompt_count(), 2);

    let path = root.join("agent-inline").join("agent-inline.jsonl");
    let content = fs::read_to_string(path).expect("jsonl");
    assert_eq!(count_transcript_message_lines(&content), 2);

    pool.close_all();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_store_offloads_full_fallback_and_advances_root_prompt_cursor() {
    let root = temp_root("worker");
    let calls = Arc::new(Mutex::new(Vec::<TranscriptMirrorWorkerJob>::new()));
    let captured = Arc::clone(&calls);
    let pool = Arc::new(TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 1 },
        Arc::new(move |job| {
            captured.lock().expect("calls").push(job.clone());
            Ok(true)
        }),
    ));
    let worker_pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let blob_db_path = root.join("conversation-blobs.db");
    let store = WorkerBlobStore::new(
        Arc::clone(&worker_pool),
        "agent-worker",
        &blob_db_path,
        None,
    );
    let mirror = OffloadingTranscriptMirror::for_transcripts_dir(
        Arc::clone(&pool),
        OffloadingTranscriptMirrorOptions {
            blob_db_paths: vec![blob_db_path.clone()],
            transcripts_dir: root.join("transcripts"),
        },
        0,
    );
    let state = LegacyTranscriptState {
        root_prompt_messages_json: vec![vec![9]],
        ..LegacyTranscriptState::default()
    };

    mirror
        .write_worker("agent-worker", &state, &store, Some(&[7, 8, 9]))
        .expect("worker offload");
    assert_eq!(mirror.previous_root_prompt_count(), 1);
    let calls = calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].conversation_id, "agent-worker");
    assert_eq!(calls[0].state_blob_id, vec![7, 8, 9]);
    assert_eq!(calls[0].blob_db_paths, vec![blob_db_path]);

    pool.close_all();
    futures::executor::block_on(worker_pool.close_all());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_blob_store_legacy_read_is_safe_inside_existing_local_executor() {
    let root = temp_root("nested-local-executor");
    let worker_pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(
        Arc::clone(&worker_pool),
        "agent-nested-local-executor",
        root.join("conversation-blobs.db"),
        None,
    );

    let result = futures::executor::block_on(async {
        LegacyTranscriptBlobStore::get_blob(&store, b"missing")
    });
    assert_eq!(result, Ok(None));

    futures::executor::block_on(worker_pool.close_all());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn worker_store_without_state_blob_does_not_claim_successful_progress() {
    let root = temp_root("missing-state");
    let pool = Arc::new(TranscriptMirrorOffloadPool::with_worker(
        TranscriptMirrorOffloadPoolOptions { max_workers: 1 },
        Arc::new(|_| panic!("missing state id must not call worker")),
    ));
    let worker_pool = Arc::new(AgentWorkerPool::new(ConversationBlobWorkerBackend::default()));
    let store = WorkerBlobStore::new(
        Arc::clone(&worker_pool),
        "agent-missing",
        root.join("conversation-blobs.db"),
        None,
    );
    let mirror = OffloadingTranscriptMirror::for_transcripts_dir(
        Arc::clone(&pool),
        OffloadingTranscriptMirrorOptions {
            blob_db_paths: vec![root.join("conversation-blobs.db")],
            transcripts_dir: root.join("transcripts"),
        },
        0,
    );
    mirror
        .write_worker(
            "agent-missing",
            &LegacyTranscriptState {
                root_prompt_messages_json: vec![vec![1]],
                ..LegacyTranscriptState::default()
            },
            &store,
            None,
        )
        .expect("missing state is observational");
    assert_eq!(mirror.previous_root_prompt_count(), 0);
    assert_eq!(pool.active_worker_count(), 0);

    pool.close_all();
    futures::executor::block_on(worker_pool.close_all());
    let _ = fs::remove_dir_all(root);
}
