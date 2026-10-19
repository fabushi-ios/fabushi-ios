use crate::agent_store_worker::AgentStoreWorker;
use crate::conversation_blob_db::ConversationBlobRecoveryError;
use crate::conversation_blob_gc::BlobReferenceDecoder;
use crate::conversation_blob_store::GarbageCollectionOutcome;
use crate::legacy_blob_retirement::LegacyBlobRetirementVerdict;
use crate::worker_blob_store::AgentWorkerBlobPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 5 * 60_000;
pub const DEFAULT_MAX_WORKERS: usize = 64;
pub const DEFAULT_SWEEP_INTERVAL_MS: u64 = 30_000;

type WorkerLane = Arc<Mutex<AgentStoreWorker>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorkerDescription {
    pub agent_id: String,
    pub blob_db_path: PathBuf,
    pub idle_for_ms: u64,
    pub active_operations: usize,
    pub next_request_id: u64,
}

#[derive(Debug)]
struct PoolState {
    workers: HashMap<PathBuf, WorkerLane>,
    active_ops: HashMap<PathBuf, usize>,
    next_request_ids: HashMap<PathBuf, u64>,
}

#[derive(Debug, Clone)]
pub struct AgentWorkerPool {
    busy_timeout_ms: u64,
    idle_timeout: Duration,
    max_workers: usize,
    sweep_interval: Duration,
    state: Arc<Mutex<PoolState>>,
    sweep_started: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
}

impl Default for AgentWorkerPool {
    fn default() -> Self {
        Self::new(5_000, DEFAULT_IDLE_TIMEOUT_MS, DEFAULT_MAX_WORKERS)
    }
}

impl AgentWorkerPool {
    pub fn new(busy_timeout_ms: u64, idle_timeout_ms: u64, max_workers: usize) -> Self {
        Self::with_sweep_interval(
            busy_timeout_ms,
            idle_timeout_ms,
            max_workers,
            DEFAULT_SWEEP_INTERVAL_MS,
        )
    }

    pub fn with_sweep_interval(
        busy_timeout_ms: u64,
        idle_timeout_ms: u64,
        max_workers: usize,
        sweep_interval_ms: u64,
    ) -> Self {
        Self {
            busy_timeout_ms,
            idle_timeout: Duration::from_millis(idle_timeout_ms),
            max_workers: max_workers.max(1),
            sweep_interval: Duration::from_millis(sweep_interval_ms.max(1)),
            state: Arc::new(Mutex::new(PoolState {
                workers: HashMap::new(),
                active_ops: HashMap::new(),
                next_request_ids: HashMap::new(),
            })),
            sweep_started: Arc::new(AtomicBool::new(false)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn poisoned(scope: &'static str) -> ConversationBlobRecoveryError {
        ConversationBlobRecoveryError::new(
            "SAND_AGENT_WORKER_POOL_POISONED",
            format!("{scope} mutex was poisoned"),
        )
    }

    fn worker_identity_matches(
        worker: &AgentStoreWorker,
        agent_id: &str,
        legacy_blob_db_path: Option<&Path>,
    ) -> bool {
        worker.agent_id() == agent_id
            && worker.legacy_blob_db_path() == legacy_blob_db_path
    }

    fn sweep_idle_locked(
        &self,
        state: &mut PoolState,
    ) -> Result<(), ConversationBlobRecoveryError> {
        let candidates = state
            .workers
            .iter()
            .filter_map(|(path, lane)| {
                if state.active_ops.get(path).copied().unwrap_or(0) != 0 {
                    return None;
                }
                let worker = lane.lock().ok()?;
                (worker.idle_for() >= self.idle_timeout).then_some(path.clone())
            })
            .collect::<Vec<_>>();

        for path in candidates {
            if let Some(lane) = state.workers.remove(&path) {
                state.active_ops.remove(&path);
                state.next_request_ids.remove(&path);
                if let Ok(mut worker) = lane.lock() {
                    let _ = worker.close();
                }
            }
        }
        Ok(())
    }

    fn start_sweep(&self) {
        if self.idle_timeout == Duration::from_millis(u64::MAX) {
            return;
        }
        if self.sweep_started.swap(true, Ordering::AcqRel) {
            return;
        }

        let state = Arc::downgrade(&self.state);
        let closed = Arc::downgrade(&self.closed);
        let idle_timeout = self.idle_timeout;
        let sweep_interval = self.sweep_interval;
        let _ = thread::Builder::new()
            .name("fabushi-ios-agent-worker-sweep".into())
            .spawn(move || loop {
                thread::sleep(sweep_interval);
                let Some(state) = state.upgrade() else {
                    break;
                };
                let Some(closed) = closed.upgrade() else {
                    break;
                };
                if closed.load(Ordering::Acquire) {
                    break;
                }

                let victims = {
                    let mut state = match state.lock() {
                        Ok(state) => state,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    let candidates = state
                        .workers
                        .iter()
                        .filter_map(|(path, lane)| {
                            if state.active_ops.get(path).copied().unwrap_or(0) != 0 {
                                return None;
                            }
                            let worker = lane.lock().ok()?;
                            (worker.idle_for() >= idle_timeout).then_some(path.clone())
                        })
                        .collect::<Vec<_>>();
                    candidates
                        .into_iter()
                        .filter_map(|path| {
                            let lane = state.workers.remove(&path);
                            state.active_ops.remove(&path);
                            state.next_request_ids.remove(&path);
                            lane
                        })
                        .collect::<Vec<_>>()
                };

                for lane in victims {
                    if let Ok(mut worker) = lane.lock() {
                        let _ = worker.close();
                    }
                }
            });
    }

    fn evict_for_capacity_locked(
        &self,
        state: &mut PoolState,
    ) -> Result<(), ConversationBlobRecoveryError> {
        while state.workers.len() >= self.max_workers {
            let candidate = state
                .workers
                .iter()
                .filter_map(|(path, lane)| {
                    if state.active_ops.get(path).copied().unwrap_or(0) != 0 {
                        return None;
                    }
                    let worker = lane.lock().ok()?;
                    Some((path.clone(), worker.idle_for()))
                })
                .max_by_key(|(_, idle)| *idle)
                .map(|(path, _)| path);

            let Some(path) = candidate else {
                return Err(ConversationBlobRecoveryError::new(
                    "SAND_AGENT_WORKER_POOL_CAPACITY",
                    "all agent store lanes are retained while the pool is at capacity",
                ));
            };
            if let Some(lane) = state.workers.remove(&path) {
                state.active_ops.remove(&path);
                state.next_request_ids.remove(&path);
                let mut worker = lane
                    .lock()
                    .map_err(|_| Self::poisoned("agent store lane"))?;
                worker.close()?;
            }
        }
        Ok(())
    }

    pub fn ensure(
        &self,
        agent_id: &str,
        blob_db_path: impl Into<PathBuf>,
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<WorkerLane, ConversationBlobRecoveryError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ConversationBlobRecoveryError::new(
                "SAND_AGENT_WORKER_POOL_CLOSED",
                "agent worker pool is closed",
            ));
        }
        let blob_db_path = blob_db_path.into();
        let mut state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        self.sweep_idle_locked(&mut state)?;

        if let Some(existing) = state.workers.get(&blob_db_path) {
            let lane = existing.clone();
            {
                let worker = lane
                    .lock()
                    .map_err(|_| Self::poisoned("agent store lane"))?;
                if !Self::worker_identity_matches(&worker, agent_id, legacy_blob_db_path) {
                    return Err(ConversationBlobRecoveryError::new(
                        "SAND_AGENT_WORKER_IDENTITY_CONFLICT",
                        format!(
                            "blob store {} is already owned by another agent or legacy path",
                            blob_db_path.display()
                        ),
                    ));
                }
            }
            return Ok(lane);
        }

        self.evict_for_capacity_locked(&mut state)?;
        let worker = AgentStoreWorker::open(
            agent_id,
            blob_db_path.clone(),
            self.busy_timeout_ms,
            legacy_blob_db_path.map(Path::to_path_buf),
        )?;
        let lane = Arc::new(Mutex::new(worker));
        state.workers.insert(blob_db_path.clone(), lane.clone());
        state.active_ops.entry(blob_db_path.clone()).or_insert(0);
        state.next_request_ids.entry(blob_db_path).or_insert(2);
        drop(state);
        self.start_sweep();
        Ok(lane)
    }

    fn retain_operation(&self, blob_db_path: &Path) -> Result<(), ConversationBlobRecoveryError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        *state
            .active_ops
            .entry(blob_db_path.to_path_buf())
            .or_insert(0) += 1;
        Ok(())
    }

    fn release_operation(&self, blob_db_path: &Path) -> Result<(), ConversationBlobRecoveryError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        let became_idle = match state.active_ops.get_mut(blob_db_path) {
            Some(active) => {
                *active = active.saturating_sub(1);
                *active == 0
            }
            None => false,
        };
        if became_idle {
            state.active_ops.remove(blob_db_path);
        }
        Ok(())
    }

    fn next_request_id(&self, blob_db_path: &Path) -> Result<u64, ConversationBlobRecoveryError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        let next = state
            .next_request_ids
            .get_mut(blob_db_path)
            .ok_or_else(|| ConversationBlobRecoveryError::new(
                "SAND_AGENT_WORKER_UNAVAILABLE",
                format!("agent store lane {} has no request identity", blob_db_path.display()),
            ))?;
        let request_id = *next;
        *next = next.saturating_add(1);
        Ok(request_id)
    }

    fn invalidate_failed_lane(&self, blob_db_path: &Path, error: &ConversationBlobRecoveryError) {
        if error.code != "SAND_AGENT_STORE_WORKER_CLOSED"
            && error.code != "SAND_AGENT_WORKER_POOL_POISONED"
        {
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            state.workers.remove(blob_db_path);
            state.active_ops.remove(blob_db_path);
            state.next_request_ids.remove(blob_db_path);
        }
    }

    fn with_worker_operation<T, F>(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        legacy_blob_db_path: Option<&Path>,
        operation: F,
    ) -> Result<T, ConversationBlobRecoveryError>
    where
        F: FnOnce(&mut AgentStoreWorker, u64) -> Result<T, ConversationBlobRecoveryError>,
    {
        self.retain_operation(blob_db_path)?;
        let result = (|| {
            let lane = self.ensure(agent_id, blob_db_path, legacy_blob_db_path)?;
            let request_id = self.next_request_id(blob_db_path)?;
            let mut worker = lane
                .lock()
                .map_err(|_| Self::poisoned("agent store lane"))?;
            operation(&mut worker, request_id)
        })();
        let release = self.release_operation(blob_db_path);
        if let Err(error) = &result {
            self.invalidate_failed_lane(blob_db_path, error);
        }
        match (result, release) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    pub fn active_worker_count(&self) -> Result<usize, ConversationBlobRecoveryError> {
        let state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        Ok(state.workers.len())
    }

    pub fn describe_workers(
        &self,
    ) -> Result<Vec<AgentWorkerDescription>, ConversationBlobRecoveryError> {
        let state = self
            .state
            .lock()
            .map_err(|_| Self::poisoned("agent worker pool"))?;
        let mut descriptions = Vec::with_capacity(state.workers.len());
        for (path, lane) in &state.workers {
            let worker = lane
                .lock()
                .map_err(|_| Self::poisoned("agent store lane"))?;
            descriptions.push(AgentWorkerDescription {
                agent_id: worker.agent_id().to_owned(),
                blob_db_path: worker.blob_db_path().to_path_buf(),
                idle_for_ms: worker
                    .idle_for()
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
                active_operations: state.active_ops.get(path).copied().unwrap_or(0),
                next_request_id: state.next_request_ids.get(path).copied().unwrap_or(1),
            });
        }
        descriptions.sort_by(|left, right| left.blob_db_path.cmp(&right.blob_db_path));
        Ok(descriptions)
    }

    pub fn get_blob_sync(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        blob_id: &[u8],
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.get_blob(blob_id),
        )
    }

    pub fn set_blob_sync(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        blob_id: &[u8],
        blob_data: &[u8],
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.set_blob(blob_id, blob_data),
        )
    }

    pub fn find_latest_root_blob_id(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.find_latest_root_blob_id(),
        )
    }

    pub fn mark_checkpoint_root(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        blob_id: &[u8],
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.mark_checkpoint_root(blob_id),
        )
    }

    pub fn clear_blobs(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.clear_blobs(),
        )
    }

    pub fn clear_stale_checkpoint_roots(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        retained_root_id_hex: &str,
        legacy_blob_db_path: Option<&Path>,
    ) -> Result<usize, ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| worker.clear_stale_checkpoint_roots(retained_root_id_hex),
        )
    }

    pub fn collect_conversation_garbage<Decoder>(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        retained_root_id_hex: &str,
        pending_write_retention_ms: u64,
        legacy_blob_db_path: Option<&Path>,
        decoder: &Decoder,
    ) -> Result<GarbageCollectionOutcome, ConversationBlobRecoveryError>
    where
        Decoder: BlobReferenceDecoder,
    {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            |worker, _request_id| {
                worker.collect_conversation_garbage(
                    retained_root_id_hex,
                    pending_write_retention_ms,
                    decoder,
                )
            },
        )
    }

    pub fn verify_legacy_blob_retirement(
        &self,
        agent_id: &str,
        blob_db_path: &Path,
        retained_root_id_hex: &str,
        legacy_blob_db_path: &Path,
    ) -> Result<LegacyBlobRetirementVerdict, ConversationBlobRecoveryError> {
        self.with_worker_operation(
            agent_id,
            blob_db_path,
            Some(legacy_blob_db_path),
            |worker, _request_id| {
                worker.verify_legacy_blob_retirement(retained_root_id_hex, legacy_blob_db_path)
            },
        )
    }

    pub fn flush_store(
        &self,
        blob_db_path: &Path,
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.retain_operation(blob_db_path)?;
        let result = (|| {
            let lane = {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| Self::poisoned("agent worker pool"))?;
                state.workers.get(blob_db_path).cloned()
            };
            let Some(lane) = lane else {
                return Ok(());
            };
            let _request_id = self.next_request_id(blob_db_path)?;
            let mut worker = lane
                .lock()
                .map_err(|_| Self::poisoned("agent store lane"))?;
            worker.flush()
        })();
        let release = self.release_operation(blob_db_path);
        if let Err(error) = &result {
            self.invalidate_failed_lane(blob_db_path, error);
        }
        match (result, release) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    pub fn close_store(
        &self,
        blob_db_path: &Path,
    ) -> Result<(), ConversationBlobRecoveryError> {
        let lane = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Self::poisoned("agent worker pool"))?;
            let lane = state.workers.remove(blob_db_path);
            state.active_ops.remove(blob_db_path);
            state.next_request_ids.remove(blob_db_path);
            lane
        };
        if let Some(lane) = lane {
            let mut worker = lane
                .lock()
                .map_err(|_| Self::poisoned("agent store lane"))?;
            worker.close()?;
        }
        Ok(())
    }

    pub fn close_all(&self) -> Result<(), ConversationBlobRecoveryError> {
        self.closed.store(true, Ordering::Release);
        let workers = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| Self::poisoned("agent worker pool"))?;
            state.active_ops.clear();
            state.next_request_ids.clear();
            state.workers.drain().map(|(_, lane)| lane).collect::<Vec<_>>()
        };
        for lane in workers {
            let mut worker = lane
                .lock()
                .map_err(|_| Self::poisoned("agent store lane"))?;
            worker.close()?;
        }
        Ok(())
    }
}

impl AgentWorkerBlobPool for AgentWorkerPool {
    type Error = ConversationBlobRecoveryError;

    async fn get_blob(
        &self,
        agent_id: &str,
        blob_db_path: &str,
        blob_id: &[u8],
        legacy_blob_db_path: Option<&str>,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        self.with_worker_operation(
            agent_id,
            Path::new(blob_db_path),
            legacy_blob_db_path.map(Path::new),
            |worker, _request_id| worker.get_blob(blob_id),
        )
    }

    async fn set_blob(
        &self,
        agent_id: &str,
        blob_db_path: &str,
        blob_id: &[u8],
        blob_data: &[u8],
        legacy_blob_db_path: Option<&str>,
    ) -> Result<(), Self::Error> {
        self.with_worker_operation(
            agent_id,
            Path::new(blob_db_path),
            legacy_blob_db_path.map(Path::new),
            |worker, _request_id| worker.set_blob(blob_id, blob_data),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_blob_store::WorkerBlobStore;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-ios-agent-worker-pool-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[tokio::test]
    async fn worker_blob_store_uses_real_agent_scoped_serial_lane() {
        let dir = temp_dir();
        let db = dir.join("agent.sqlite");
        let pool = AgentWorkerPool::default();
        let store = WorkerBlobStore::new(
            pool.clone(),
            "agent-a",
            db.to_string_lossy(),
            None,
        );

        store.set_blob(&[1, 2], b"payload").await.unwrap();
        assert_eq!(
            store.get_blob(&[1, 2]).await.unwrap().as_deref(),
            Some(b"payload".as_slice())
        );
        assert_eq!(pool.active_worker_count().unwrap(), 1);
        let descriptions = pool.describe_workers().unwrap();
        assert_eq!(descriptions[0].agent_id, "agent-a");
        assert_eq!(descriptions[0].active_operations, 0);
        assert_eq!(
            descriptions[0].next_request_id, 4,
            "implicit init + set + get consume correlated request identities",
        );
    }

    #[test]
    fn rejects_two_agent_identities_for_the_same_store_path() {
        let dir = temp_dir();
        let db = dir.join("shared.sqlite");
        let pool = AgentWorkerPool::default();

        pool.ensure("agent-a", &db, None).unwrap();
        let error = pool.ensure("agent-b", &db, None).unwrap_err();
        assert_eq!(error.code, "SAND_AGENT_WORKER_IDENTITY_CONFLICT");
    }

    #[test]
    fn capacity_evicts_an_unretained_old_lane_without_background_timer() {
        let dir = temp_dir();
        let first = dir.join("first.sqlite");
        let second = dir.join("second.sqlite");
        let pool = AgentWorkerPool::new(500, u64::MAX, 1);

        {
            let lane = pool.ensure("agent-a", &first, None).unwrap();
            drop(lane);
        }
        pool.ensure("agent-b", &second, None).unwrap();
        assert_eq!(pool.active_worker_count().unwrap(), 1);
        assert_eq!(
            pool.describe_workers().unwrap()[0].blob_db_path,
            second
        );
    }

    #[test]
    fn capacity_never_evicts_an_explicitly_active_lane() {
        let dir = temp_dir();
        let first = dir.join("active.sqlite");
        let second = dir.join("second.sqlite");
        let pool = AgentWorkerPool::new(500, u64::MAX, 1);

        pool.retain_operation(&first).unwrap();
        let lane = pool.ensure("agent-a", &first, None).unwrap();
        drop(lane);
        let error = pool.ensure("agent-b", &second, None).unwrap_err();
        assert_eq!(error.code, "SAND_AGENT_WORKER_POOL_CAPACITY");
        pool.release_operation(&first).unwrap();

        pool.ensure("agent-b", &second, None).unwrap();
        assert_eq!(pool.describe_workers().unwrap()[0].blob_db_path, second);
    }

    #[test]
    fn closed_lane_failure_propagates_and_next_request_rebuilds_lane() {
        let dir = temp_dir();
        let db = dir.join("closed.sqlite");
        let pool = AgentWorkerPool::new(500, u64::MAX, 2);
        let original = pool.ensure("agent-a", &db, None).unwrap();
        original.lock().unwrap().close().unwrap();

        let error = pool
            .find_latest_root_blob_id("agent-a", &db, None)
            .unwrap_err();
        assert_eq!(error.code, "SAND_AGENT_STORE_WORKER_CLOSED");

        let replacement = pool.ensure("agent-a", &db, None).unwrap();
        assert!(!Arc::ptr_eq(&original, &replacement));
        assert_eq!(pool.describe_workers().unwrap()[0].next_request_id, 2);
    }

    #[test]
    fn close_store_resets_request_identity_for_reopened_lane() {
        let dir = temp_dir();
        let db = dir.join("reopen.sqlite");
        let pool = AgentWorkerPool::new(500, u64::MAX, 2);

        pool.mark_checkpoint_root("agent-a", &db, b"root", None)
            .unwrap();
        assert_eq!(pool.describe_workers().unwrap()[0].next_request_id, 3);

        pool.close_store(&db).unwrap();
        assert_eq!(pool.active_worker_count().unwrap(), 0);

        pool.ensure("agent-a", &db, None).unwrap();
        assert_eq!(
            pool.describe_workers().unwrap()[0].next_request_id,
            2,
            "a reopened worker must restart request identity after its implicit init request",
        );
    }

    #[test]
    fn background_sweep_releases_idle_unretained_lane() {
        let dir = temp_dir();
        let db = dir.join("idle.sqlite");
        let pool = AgentWorkerPool::with_sweep_interval(500, 10, 4, 5);

        {
            let lane = pool.ensure("agent-a", &db, None).unwrap();
            drop(lane);
        }

        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(pool.active_worker_count().unwrap(), 0);
    }

    #[test]
    fn close_all_is_terminal_for_future_lanes() {
        let dir = temp_dir();
        let pool = AgentWorkerPool::default();
        pool.ensure("agent-a", dir.join("a.sqlite"), None).unwrap();
        pool.close_all().unwrap();

        let error = pool
            .ensure("agent-b", dir.join("b.sqlite"), None)
            .unwrap_err();
        assert_eq!(error.code, "SAND_AGENT_WORKER_POOL_CLOSED");
    }

    #[test]
    fn close_all_releases_every_lane() {
        let dir = temp_dir();
        let pool = AgentWorkerPool::default();
        pool.ensure("agent-a", dir.join("a.sqlite"), None).unwrap();
        pool.ensure("agent-b", dir.join("b.sqlite"), None).unwrap();
        pool.close_all().unwrap();
        assert_eq!(pool.active_worker_count().unwrap(), 0);
    }
}
