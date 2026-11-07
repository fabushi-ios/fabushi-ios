use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use mahayana_host_runtime::agent_isolation::{
    AgentBlobWorkerBackend, AgentWorkerFuture, AgentWorkerPool, WorkerBlobStore,
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Get {
        agent_id: String,
        blob_db_path: PathBuf,
        blob_id: Vec<u8>,
        legacy_blob_db_path: Option<PathBuf>,
    },
    Set {
        agent_id: String,
        blob_db_path: PathBuf,
        blob_id: Vec<u8>,
        blob_data: Vec<u8>,
        legacy_blob_db_path: Option<PathBuf>,
    },
}

#[derive(Default)]
struct FakeBackend {
    calls: Mutex<Vec<Call>>,
    blobs: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl AgentBlobWorkerBackend for FakeBackend {
    type Error = &'static str;

    fn get_blob<'a>(
        &'a self,
        agent_id: &'a str,
        blob_db_path: &'a Path,
        blob_id: &'a [u8],
        legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<Option<Vec<u8>>, Self::Error>> {
        self.calls.lock().expect("calls").push(Call::Get {
            agent_id: agent_id.to_string(),
            blob_db_path: blob_db_path.to_path_buf(),
            blob_id: blob_id.to_vec(),
            legacy_blob_db_path: legacy_blob_db_path.map(Path::to_path_buf),
        });
        let value = self.blobs.lock().expect("blobs").get(blob_id).cloned();
        Box::pin(async move { Ok(value) })
    }

    fn set_blob<'a>(
        &'a self,
        agent_id: &'a str,
        blob_db_path: &'a Path,
        blob_id: &'a [u8],
        blob_data: &'a [u8],
        legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        self.calls.lock().expect("calls").push(Call::Set {
            agent_id: agent_id.to_string(),
            blob_db_path: blob_db_path.to_path_buf(),
            blob_id: blob_id.to_vec(),
            blob_data: blob_data.to_vec(),
            legacy_blob_db_path: legacy_blob_db_path.map(Path::to_path_buf),
        });
        self.blobs
            .lock()
            .expect("blobs")
            .insert(blob_id.to_vec(), blob_data.to_vec());
        Box::pin(async { Ok(()) })
    }

    fn find_latest_root_blob_id<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<Option<Vec<u8>>, Self::Error>> {
        Box::pin(async { Ok(None) })
    }

    fn clear_blobs<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    fn clear_stale_checkpoint_roots<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<usize, Self::Error>> {
        Box::pin(async { Ok(0) })
    }

    fn collect_conversation_garbage<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _pending_write_retention_ms: u64,
        _legacy_blob_db_path: Option<&'a Path>,
    ) -> AgentWorkerFuture<'a, Result<mahayana_host_runtime::agent_isolation::ConversationGarbageCollectionOutcome, Self::Error>> {
        Box::pin(async {
            Ok(mahayana_host_runtime::agent_isolation::ConversationGarbageCollectionOutcome::Skipped {
                reason: "not-used".into(),
                unresolved_proto_refs: 0,
            })
        })
    }

    fn verify_legacy_blob_retirement<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
        _retained_root_id_hex: &'a str,
        _legacy_blob_db_path: &'a Path,
    ) -> AgentWorkerFuture<'a, Result<mahayana_host_runtime::agent_isolation::LegacyBlobRetirementVerdict, Self::Error>> {
        Box::pin(async {
            Ok(mahayana_host_runtime::agent_isolation::LegacyBlobRetirementVerdict::defer("not-used"))
        })
    }
    fn close_store<'a>(
        &'a self,
        _agent_id: &'a str,
        _blob_db_path: &'a Path,
    ) -> AgentWorkerFuture<'a, Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

}

#[test]
fn worker_blob_store_routes_get_set_local_only_and_flush_through_worker_pool() {
    let backend = FakeBackend::default();
    let pool = Arc::new(AgentWorkerPool::new(backend));
    let store = WorkerBlobStore::new(
        Arc::clone(&pool),
        "agent-7",
        "/tmp/agent-7.sqlite",
        Some(PathBuf::from("/tmp/legacy.sqlite")),
    );
    let ctx = "ignored-context";

    block_on_ready(store.set_blob(&ctx, b"id-a", b"data-a")).expect("set blob");
    assert_eq!(
        block_on_ready(store.get_blob(&ctx, b"id-a")).expect("get blob"),
        Some(b"data-a".to_vec())
    );
    block_on_ready(store.set_blob_locally_only(&ctx, b"id-b", b"data-b"))
        .expect("local set");
    block_on_ready(store.flush(&ctx));

    let calls = pool.backend().calls.lock().expect("calls").clone();
    assert_eq!(
        calls,
        vec![
            Call::Set {
                agent_id: "agent-7".to_string(),
                blob_db_path: PathBuf::from("/tmp/agent-7.sqlite"),
                blob_id: b"id-a".to_vec(),
                blob_data: b"data-a".to_vec(),
                legacy_blob_db_path: Some(PathBuf::from("/tmp/legacy.sqlite")),
            },
            Call::Get {
                agent_id: "agent-7".to_string(),
                blob_db_path: PathBuf::from("/tmp/agent-7.sqlite"),
                blob_id: b"id-a".to_vec(),
                legacy_blob_db_path: Some(PathBuf::from("/tmp/legacy.sqlite")),
            },
            Call::Set {
                agent_id: "agent-7".to_string(),
                blob_db_path: PathBuf::from("/tmp/agent-7.sqlite"),
                blob_id: b"id-b".to_vec(),
                blob_data: b"data-b".to_vec(),
                legacy_blob_db_path: Some(PathBuf::from("/tmp/legacy.sqlite")),
            },
        ]
    );
}
