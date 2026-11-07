use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use mahayana_host_runtime::agent_isolation::{
    AgentBlobWorkerBackend, AgentWorkerFuture, AgentWorkerPool, AgentWorkerPoolOptions,
    ConversationGarbageCollectionOutcome, LegacyBlobRetirementVerdict,
};

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

#[derive(Default)]
struct LifecycleBackend {
    blobs: Mutex<BTreeMap<PathBuf, BTreeMap<Vec<u8>, Vec<u8>>>>,
    worker_threads: Mutex<Vec<String>>,
}

impl LifecycleBackend {
    fn record_thread(&self) {
        self.worker_threads
            .lock()
            .expect("worker threads")
            .push(std::thread::current().name().unwrap_or("unnamed").to_string());
    }
}

impl AgentBlobWorkerBackend for LifecycleBackend {
    type Error = &'static str;

    fn get_blob<'a>(
        &'a self,
        _agent_id: &'a str,
        blob_db_path: &'a Path,
        blob_id: &'a [u8],
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<Option<Vec<u8>>, Self::Error>> {
        self.record_thread();
        let value = self
            .blobs
            .lock()
            .expect("blobs")
            .get(blob_db_path)
            .and_then(|entries| entries.get(blob_id))
            .cloned();
        Box::pin(async move { Ok(value) })
    }

    fn set_blob<'a>(
        &'a self,
        _agent_id: &'a str,
        blob_db_path: &'a Path,
        blob_id: &'a [u8],
        blob_data: &'a [u8],
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        self.record_thread();
        self.blobs
            .lock()
            .expect("blobs")
            .entry(blob_db_path.to_path_buf())
            .or_default()
            .insert(blob_id.to_vec(), blob_data.to_vec());
        Box::pin(async { Ok(()) })
    }

    fn find_latest_root_blob_id<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<Option<Vec<u8>>, Self::Error>> {
        self.record_thread();
        Box::pin(async { Ok(Some(b"root".to_vec())) })
    }

    fn clear_blobs<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        self.record_thread();
        Box::pin(async { Ok(()) })
    }

    fn clear_stale_checkpoint_roots<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<usize, Self::Error>> {
        self.record_thread();
        Box::pin(async { Ok(3) })
    }

    fn collect_conversation_garbage<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _pending_write_retention_ms: u64,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<ConversationGarbageCollectionOutcome, Self::Error>> {
        self.record_thread();
        Box::pin(async {
            Ok(ConversationGarbageCollectionOutcome::Collected {
                deleted_rows: 2,
                deleted_bytes: 128,
                live_rows: 4,
                live_bytes: 512,
                retained_pending_rows: 1,
                vacuumed: false,
            })
        })
    }

    fn verify_legacy_blob_retirement<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _legacy_blob_db_path: &'a Path,
    ) -> AgentWorkerFuture<'a, Result<LegacyBlobRetirementVerdict, Self::Error>> {
        self.record_thread();
        Box::pin(async { Ok(LegacyBlobRetirementVerdict::retirable(7, 2048)) })
    }
    fn close_store<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

}

fn options(max_workers: usize) -> AgentWorkerPoolOptions {
    AgentWorkerPoolOptions {
        busy_timeout_ms: 5_000,
        idle_timeout_ms: u64::MAX,
        max_workers,
        sweep_interval_ms: 60_000,
    }
}

#[test]
fn worker_pool_boots_one_named_worker_per_store_and_correlates_requests() {
    let pool = AgentWorkerPool::with_options(LifecycleBackend::default(), options(4));
    let path = Path::new("/tmp/agent-a.sqlite");

    block_on_ready(pool.set_blob("agent-a", path, b"blob-a", b"value-a", None))
        .expect("set blob");
    assert_eq!(
        block_on_ready(pool.get_blob("agent-a", path, b"blob-a", None))
            .expect("get blob"),
        Some(b"value-a".to_vec())
    );

    assert_eq!(pool.active_worker_count(), 1);
    let worker = pool.describe_workers().pop().expect("worker description");
    assert_eq!(worker.blob_db_path, path);
    assert_eq!(worker.pid, std::process::id());
    assert_eq!(worker.next_request_id, 4, "init + set + get consume request ids");

    let threads = pool.backend().worker_threads.lock().expect("worker threads");
    assert_eq!(threads.len(), 2);
    assert!(threads.iter().all(|name| name.starts_with("mahayana-agent-worker-")));
}

#[test]
fn worker_pool_evicts_oldest_unretained_store_at_capacity() {
    let pool = AgentWorkerPool::with_options(LifecycleBackend::default(), options(1));
    let first = Path::new("/tmp/agent-first.sqlite");
    let second = Path::new("/tmp/agent-second.sqlite");

    block_on_ready(pool.set_blob("first", first, b"id", b"one", None)).expect("first set");
    block_on_ready(pool.set_blob("second", second, b"id", b"two", None)).expect("second set");

    assert_eq!(pool.active_worker_count(), 1);
    assert_eq!(pool.describe_workers()[0].blob_db_path, second);
}

#[test]
fn worker_pool_idle_sweep_and_close_store_retire_worker_threads() {
    let mut idle_options = options(4);
    idle_options.idle_timeout_ms = 1;
    let pool = AgentWorkerPool::with_options(LifecycleBackend::default(), idle_options);
    let first = Path::new("/tmp/agent-idle.sqlite");
    let second = Path::new("/tmp/agent-close.sqlite");

    block_on_ready(pool.set_blob("idle", first, b"id", b"one", None)).expect("idle set");
    assert_eq!(pool.sweep_idle_at(u64::MAX), 1);
    assert_eq!(pool.active_worker_count(), 0);

    block_on_ready(pool.set_blob("close", second, b"id", b"two", None)).expect("close set");
    assert_eq!(pool.active_worker_count(), 1);
    block_on_ready(pool.close_store(second));
    assert_eq!(pool.active_worker_count(), 0);
}

#[test]
fn worker_pool_routes_root_clear_gc_and_legacy_retirement_over_correlated_requests() {
    let pool = AgentWorkerPool::with_options(LifecycleBackend::default(), options(4));
    let path = Path::new("/tmp/agent-maintenance.sqlite");
    let legacy = Path::new("/tmp/agent-maintenance-legacy.sqlite");

    assert_eq!(
        block_on_ready(pool.find_latest_root_blob_id("agent", path, Some(legacy)))
            .expect("latest root"),
        Some(b"root".to_vec())
    );
    block_on_ready(pool.clear_blobs("agent", path, Some(legacy))).expect("clear blobs");
    assert_eq!(
        block_on_ready(pool.clear_stale_checkpoint_roots(
            "agent",
            path,
            "root-hex",
            Some(legacy),
        ))
        .expect("clear stale roots"),
        3
    );
    assert_eq!(
        block_on_ready(pool.collect_conversation_garbage(
            "agent",
            path,
            "root-hex",
            60_000,
            Some(legacy),
        ))
        .expect("collect garbage"),
        ConversationGarbageCollectionOutcome::Collected {
            deleted_rows: 2,
            deleted_bytes: 128,
            live_rows: 4,
            live_bytes: 512,
            retained_pending_rows: 1,
            vacuumed: false,
        }
    );
    assert_eq!(
        block_on_ready(pool.verify_legacy_blob_retirement(
            "agent",
            path,
            "root-hex",
            legacy,
        ))
        .expect("legacy retirement"),
        LegacyBlobRetirementVerdict::retirable(7, 2048)
    );

    let worker = pool.describe_workers().pop().expect("worker description");
    assert_eq!(
        worker.next_request_id, 7,
        "init + five maintenance requests consume correlated request ids",
    );
}
