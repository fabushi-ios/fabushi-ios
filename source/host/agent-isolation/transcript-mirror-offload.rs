use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::conversation_state_binary::TranscriptMirrorConversationState;
use crate::host_diagnostics::{HostDiagnostic, report_host_diagnostic};
use crate::legacy_transcript_mirror::LegacyFileTranscriptMirror;
use crate::transcript_mirror_worker::execute_legacy_mirror_job;
use crate::transcript_occurrence_deriver::TranscriptOccurrenceBlobStore;
use serde_json::Value;

pub const DEFAULT_MIRROR_WORKERS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMirrorJob {
    pub conversation_id: String,
    pub state_blob_id: Vec<u8>,
    pub blob_db_paths: Vec<PathBuf>,
    pub transcripts_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptMirrorDispatch {
    pub worker_index: usize,
    /// Monotonic generation for the shard worker. A failed worker is restarted
    /// before the next dispatch and therefore receives a new generation.
    pub worker_generation: u64,
    pub job: TranscriptMirrorJob,
    /// Number of callers whose completion is represented by this write.
    pub waiter_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptMirrorSubmission {
    Dispatch(TranscriptMirrorDispatch),
    Coalesced {
        worker_index: usize,
        queued_waiter_count: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptMirrorOffloadError {
    Closed,
    StatePoisoned,
}

impl fmt::Display for TranscriptMirrorOffloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("transcript mirror offload pool is closed"),
            Self::StatePoisoned => formatter.write_str("transcript mirror offload state is poisoned"),
        }
    }
}

impl std::error::Error for TranscriptMirrorOffloadError {}

#[derive(Debug)]
struct QueuedMirrorJob {
    job: TranscriptMirrorJob,
    waiter_count: usize,
}

#[derive(Debug, Default)]
struct ConversationLane {
    queued: Option<QueuedMirrorJob>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MirrorWorkerState {
    generation: u64,
    alive: bool,
    active_dispatches: usize,
}

impl Default for MirrorWorkerState {
    fn default() -> Self {
        Self {
            generation: 0,
            alive: false,
            active_dispatches: 0,
        }
    }
}

/// iOS-native equivalent of Grok's transcript mirror worker pool.
///
/// Grok uses Node worker_threads. iOS must not emulate a background process or
/// daemon, so this type owns only scheduling state. The Host executes returned
/// dispatches using app-owned tasks. Each conversation has at most one active
/// write; newer checkpoints coalesce into a single queued write and retain the
/// number of callers that must be completed from that newest write.
#[derive(Debug)]
pub struct TranscriptMirrorOffloadPool {
    max_workers: usize,
    closed: AtomicBool,
    lanes: Mutex<HashMap<String, ConversationLane>>,
    workers: Mutex<Vec<MirrorWorkerState>>,
}

impl Default for TranscriptMirrorOffloadPool {
    fn default() -> Self {
        Self::new(DEFAULT_MIRROR_WORKERS)
    }
}

impl TranscriptMirrorOffloadPool {
    pub fn new(max_workers: usize) -> Self {
        Self {
            max_workers: max_workers.max(1),
            closed: AtomicBool::new(false),
            lanes: Mutex::new(HashMap::new()),
            workers: Mutex::new(vec![MirrorWorkerState::default(); max_workers.max(1)]),
        }
    }

    pub fn worker_index_for(&self, conversation_id: &str) -> usize {
        let mut hash = 0i32;
        for unit in conversation_id.encode_utf16() {
            hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
        }
        (hash.unsigned_abs() as usize) % self.max_workers
    }

    fn lease_worker(&self, worker_index: usize) -> Result<u64, TranscriptMirrorOffloadError> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        let worker = workers
            .get_mut(worker_index)
            .ok_or(TranscriptMirrorOffloadError::StatePoisoned)?;
        if !worker.alive {
            worker.generation = worker.generation.saturating_add(1).max(1);
            worker.alive = true;
        }
        worker.active_dispatches = worker.active_dispatches.saturating_add(1);
        Ok(worker.generation)
    }

    fn settle_worker(
        &self,
        worker_index: usize,
        failed: bool,
    ) -> Result<(), TranscriptMirrorOffloadError> {
        let mut workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        let worker = workers
            .get_mut(worker_index)
            .ok_or(TranscriptMirrorOffloadError::StatePoisoned)?;
        worker.active_dispatches = worker.active_dispatches.saturating_sub(1);
        if failed {
            worker.alive = false;
        }
        Ok(())
    }

    pub fn submit(
        &self,
        job: TranscriptMirrorJob,
    ) -> Result<TranscriptMirrorSubmission, TranscriptMirrorOffloadError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TranscriptMirrorOffloadError::Closed);
        }
        let worker_index = self.worker_index_for(&job.conversation_id);
        let conversation_id = job.conversation_id.clone();
        let mut lanes = self
            .lanes
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;

        match lanes.entry(conversation_id) {
            Entry::Vacant(entry) => {
                entry.insert(ConversationLane::default());
                let worker_generation = self.lease_worker(worker_index)?;
                Ok(TranscriptMirrorSubmission::Dispatch(TranscriptMirrorDispatch {
                    worker_index,
                    worker_generation,
                    job,
                    waiter_count: 1,
                }))
            }
            Entry::Occupied(mut entry) => {
                let lane = entry.get_mut();
                let queued_waiter_count = match lane.queued.as_mut() {
                    Some(queued) => {
                        queued.job = job;
                        queued.waiter_count = queued.waiter_count.saturating_add(1);
                        queued.waiter_count
                    }
                    None => {
                        lane.queued = Some(QueuedMirrorJob {
                            job,
                            waiter_count: 1,
                        });
                        1
                    }
                };
                Ok(TranscriptMirrorSubmission::Coalesced {
                    worker_index,
                    queued_waiter_count,
                })
            }
        }
    }

    /// Marks the current write complete. If a newer checkpoint was coalesced,
    /// returns the single next dispatch and keeps the lane active.
    pub fn complete(
        &self,
        conversation_id: &str,
    ) -> Result<Option<TranscriptMirrorDispatch>, TranscriptMirrorOffloadError> {
        self.complete_with_outcome(conversation_id, false)
    }

    /// Settles one active dispatch. A failed shard is marked unhealthy before
    /// the queued successor is leased, so the successor receives a fresh
    /// worker generation instead of inheriting stale worker state.
    pub fn complete_with_outcome(
        &self,
        conversation_id: &str,
        worker_failed: bool,
    ) -> Result<Option<TranscriptMirrorDispatch>, TranscriptMirrorOffloadError> {
        let worker_index = self.worker_index_for(conversation_id);
        let queued = {
            let mut lanes = self
                .lanes
                .lock()
                .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
            let queued = lanes
                .get_mut(conversation_id)
                .and_then(|lane| lane.queued.take());
            if queued.is_none() {
                lanes.remove(conversation_id);
            }
            queued
        };

        self.settle_worker(worker_index, worker_failed)?;

        if let Some(queued) = queued {
            let worker_generation = self.lease_worker(worker_index)?;
            return Ok(Some(TranscriptMirrorDispatch {
                worker_index,
                worker_generation,
                job: queued.job,
                waiter_count: queued.waiter_count,
            }));
        }
        Ok(None)
    }

    pub fn queued_waiter_count(
        &self,
        conversation_id: &str,
    ) -> Result<usize, TranscriptMirrorOffloadError> {
        let lanes = self
            .lanes
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        Ok(lanes
            .get(conversation_id)
            .and_then(|lane| lane.queued.as_ref())
            .map(|queued| queued.waiter_count)
            .unwrap_or(0))
    }

    pub fn active_worker_count(&self) -> Result<usize, TranscriptMirrorOffloadError> {
        let workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        Ok(workers.iter().filter(|worker| worker.alive).count())
    }

    pub fn active_dispatch_count(&self) -> Result<usize, TranscriptMirrorOffloadError> {
        let workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        Ok(workers.iter().map(|worker| worker.active_dispatches).sum())
    }

    pub fn worker_generation(
        &self,
        worker_index: usize,
    ) -> Result<u64, TranscriptMirrorOffloadError> {
        let workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        workers
            .get(worker_index)
            .map(|worker| worker.generation)
            .ok_or(TranscriptMirrorOffloadError::StatePoisoned)
    }

    pub fn active_lane_count(&self) -> Result<usize, TranscriptMirrorOffloadError> {
        let lanes = self
            .lanes
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        Ok(lanes.len())
    }

    /// Stops new submissions and discards queued work. The Host remains
    /// responsible for settling any task that was already dispatched.
    pub fn close(&self) -> Result<usize, TranscriptMirrorOffloadError> {
        self.closed.store(true, Ordering::Release);
        let mut lanes = self
            .lanes
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        let queued_waiters = lanes
            .values()
            .filter_map(|lane| lane.queued.as_ref())
            .map(|queued| queued.waiter_count)
            .sum();
        lanes.clear();
        drop(lanes);

        let mut workers = self
            .workers
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;
        for worker in workers.iter_mut() {
            worker.alive = false;
            worker.active_dispatches = 0;
        }
        Ok(queued_waiters)
    }
}


pub type TranscriptMirrorWorkerExecutor =
    Arc<dyn Fn(&TranscriptMirrorJob) -> Result<bool, String> + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffloadingTranscriptMirrorOptions {
    pub blob_db_paths: Vec<PathBuf>,
    pub transcripts_dir: PathBuf,
}

/// iOS-native production wrapper for the legacy transcript fallback.
///
/// Desktop owns long-lived worker threads. iOS keeps the same observable
/// contract without introducing another daemon/runtime owner: incremental
/// inline writes stay on the Host lane, full worker fallbacks are submitted
/// through TranscriptMirrorOffloadPool, and the supplied executor performs the
/// app-owned task. Cursor progress is committed only after a successful write.
pub struct OffloadingTranscriptMirror {
    inline: LegacyFileTranscriptMirror,
    pool: Arc<TranscriptMirrorOffloadPool>,
    executor: TranscriptMirrorWorkerExecutor,
    options: OffloadingTranscriptMirrorOptions,
    previous_root_prompt_count: Mutex<usize>,
    write_lane: Mutex<()>,
}

impl OffloadingTranscriptMirror {
    pub fn production(
        options: OffloadingTranscriptMirrorOptions,
        previous_root_prompt_count: usize,
    ) -> Self {
        Self::with_executor(
            Arc::new(TranscriptMirrorOffloadPool::default()),
            options,
            previous_root_prompt_count,
            Arc::new(|job| {
                execute_legacy_mirror_job(job)
                    .map_err(|error| error.to_string())
            }),
        )
    }

    pub fn with_executor(
        pool: Arc<TranscriptMirrorOffloadPool>,
        options: OffloadingTranscriptMirrorOptions,
        previous_root_prompt_count: usize,
        executor: TranscriptMirrorWorkerExecutor,
    ) -> Self {
        Self {
            inline: LegacyFileTranscriptMirror::new(&options.transcripts_dir),
            pool,
            executor,
            options,
            previous_root_prompt_count: Mutex::new(previous_root_prompt_count),
            write_lane: Mutex::new(()),
        }
    }

    pub fn previous_root_prompt_count(&self) -> usize {
        self.previous_root_prompt_count
            .lock()
            .map(|value| *value)
            .unwrap_or_default()
    }

    pub fn write_inline<Store: TranscriptOccurrenceBlobStore>(
        &self,
        conversation_id: &str,
        state: &TranscriptMirrorConversationState,
        blob_store: &Store,
    ) -> Result<(), String> {
        let _lane = self
            .write_lane
            .lock()
            .map_err(|_| "transcript mirror write lane is poisoned".to_string())?;
        let previous = self.previous_root_prompt_count();
        if let Some(count) = self
            .inline
            .write_incremental(conversation_id, state, blob_store, previous)
            .map_err(|error| error.to_string())?
        {
            self.set_previous_root_prompt_count(count);
            return Ok(());
        }

        let written = self
            .inline
            .write_full(conversation_id, state, blob_store)
            .map_err(|error| error.to_string())?;
        if written {
            self.set_previous_root_prompt_count(state.root_prompt_messages_json.len());
        } else {
            warn_stale_mirror(conversation_id, "full-write-failed", None);
        }
        Ok(())
    }

    pub fn write_worker(
        &self,
        conversation_id: &str,
        state: &TranscriptMirrorConversationState,
        state_blob_id: Option<&[u8]>,
    ) -> Result<(), String> {
        let _lane = self
            .write_lane
            .lock()
            .map_err(|_| "transcript mirror write lane is poisoned".to_string())?;
        let Some(state_blob_id) = state_blob_id.filter(|value| !value.is_empty()) else {
            warn_stale_mirror(conversation_id, "worker-unavailable", None);
            return Ok(());
        };

        let submission = self
            .pool
            .submit(TranscriptMirrorJob {
                conversation_id: conversation_id.to_string(),
                state_blob_id: state_blob_id.to_vec(),
                blob_db_paths: self.options.blob_db_paths.clone(),
                transcripts_dir: self.options.transcripts_dir.clone(),
            })
            .map_err(|error| error.to_string())?;

        let TranscriptMirrorSubmission::Dispatch(mut dispatch) = submission else {
            // The wrapper serializes a conversation through write_lane. A
            // coalesced result can only come from another owner sharing the
            // pool, so it must not claim cursor progress for this wrapper.
            warn_stale_mirror(conversation_id, "worker-coalesced", None);
            return Ok(());
        };

        let mut is_primary_dispatch = true;
        let mut primary_succeeded = false;
        loop {
            let result = (self.executor)(&dispatch.job);
            let worker_failed = result.is_err();
            let succeeded = matches!(result, Ok(true));
            if is_primary_dispatch {
                primary_succeeded = succeeded;
                is_primary_dispatch = false;
            }
            let error_class = worker_failed.then_some("Error");
            if !succeeded {
                warn_stale_mirror(conversation_id, "worker-write-failed", error_class);
            }

            let next = self
                .pool
                .complete_with_outcome(conversation_id, worker_failed)
                .map_err(|error| error.to_string())?;
            if next.is_none() {
                if primary_succeeded {
                    self.set_previous_root_prompt_count(state.root_prompt_messages_json.len());
                }
                return Ok(());
            }
            // A queued successor belongs to another submitter sharing the pool.
            // Settle it for worker-health/lane correctness, but never advance
            // this wrapper's cursor from another caller's checkpoint.
            dispatch = next.expect("checked above");
        }
    }

    pub fn close(&self) -> Result<usize, String> {
        self.pool.close().map_err(|error| error.to_string())
    }

    fn set_previous_root_prompt_count(&self, count: usize) {
        if let Ok(mut previous) = self.previous_root_prompt_count.lock() {
            *previous = count;
        }
    }
}

fn warn_stale_mirror(
    conversation_id: &str,
    reason: &str,
    error_class: Option<&str>,
) {
    let mut diagnostic = HostDiagnostic::new();
    diagnostic.insert(
        "kind".into(),
        Value::String("transcript_mirror_stale".into()),
    );
    diagnostic.insert(
        "agentId".into(),
        Value::String(conversation_id.to_string()),
    );
    diagnostic.insert("reason".into(), Value::String(reason.to_string()));
    if let Some(error_class) = error_class {
        diagnostic.insert(
            "errorClass".into(),
            Value::String(error_class.to_string()),
        );
    }
    report_host_diagnostic(&diagnostic);
}


#[cfg(test)]
mod tests {
    use super::*;

    fn job(conversation_id: &str, checkpoint: u8) -> TranscriptMirrorJob {
        TranscriptMirrorJob {
            conversation_id: conversation_id.into(),
            state_blob_id: vec![checkpoint],
            blob_db_paths: vec![PathBuf::from("conversation.sqlite")],
            transcripts_dir: PathBuf::from("transcripts"),
        }
    }

    #[test]
    fn matches_reference_utf16_hash_sharding() {
        let pool = TranscriptMirrorOffloadPool::new(2);
        assert_eq!(pool.worker_index_for("a"), 1);
        assert_eq!(pool.worker_index_for("b"), 0);
        assert!(pool.worker_index_for("agent-🙂") < 2);
    }

    #[test]
    fn serializes_each_conversation_and_coalesces_to_newest_checkpoint() {
        let pool = TranscriptMirrorOffloadPool::default();
        let first = pool.submit(job("conversation-a", 1)).unwrap();
        assert!(matches!(
            first,
            TranscriptMirrorSubmission::Dispatch(TranscriptMirrorDispatch {
                waiter_count: 1,
                ..
            })
        ));

        assert!(matches!(
            pool.submit(job("conversation-a", 2)).unwrap(),
            TranscriptMirrorSubmission::Coalesced {
                queued_waiter_count: 1,
                ..
            }
        ));
        assert!(matches!(
            pool.submit(job("conversation-a", 3)).unwrap(),
            TranscriptMirrorSubmission::Coalesced {
                queued_waiter_count: 2,
                ..
            }
        ));

        let next = pool.complete("conversation-a").unwrap().unwrap();
        assert_eq!(next.job.state_blob_id, vec![3]);
        assert_eq!(next.waiter_count, 2);
        assert_eq!(pool.active_lane_count().unwrap(), 1);

        assert!(pool.complete("conversation-a").unwrap().is_none());
        assert_eq!(pool.active_lane_count().unwrap(), 0);
    }

    #[test]
    fn tracks_queued_waiters_active_dispatches_and_live_workers() {
        let pool = TranscriptMirrorOffloadPool::new(2);
        let first = match pool.submit(job("conversation-a", 1)).unwrap() {
            TranscriptMirrorSubmission::Dispatch(dispatch) => dispatch,
            other => panic!("expected dispatch, got {other:?}"),
        };
        assert_eq!(first.worker_generation, 1);
        assert_eq!(pool.active_worker_count().unwrap(), 1);
        assert_eq!(pool.active_dispatch_count().unwrap(), 1);

        pool.submit(job("conversation-a", 2)).unwrap();
        pool.submit(job("conversation-a", 3)).unwrap();
        assert_eq!(pool.queued_waiter_count("conversation-a").unwrap(), 2);
        assert_eq!(pool.active_dispatch_count().unwrap(), 1);

        let next = pool.complete("conversation-a").unwrap().unwrap();
        assert_eq!(next.worker_generation, 1);
        assert_eq!(next.waiter_count, 2);
        assert_eq!(pool.active_dispatch_count().unwrap(), 1);
        assert!(pool.complete("conversation-a").unwrap().is_none());
        assert_eq!(pool.active_dispatch_count().unwrap(), 0);
        assert_eq!(pool.active_worker_count().unwrap(), 1);
    }

    #[test]
    fn failed_worker_restarts_before_queued_successor_and_next_submission() {
        let pool = TranscriptMirrorOffloadPool::new(1);
        let first = match pool.submit(job("conversation-a", 1)).unwrap() {
            TranscriptMirrorSubmission::Dispatch(dispatch) => dispatch,
            other => panic!("expected dispatch, got {other:?}"),
        };
        assert_eq!(first.worker_generation, 1);
        pool.submit(job("conversation-a", 2)).unwrap();

        let restarted = pool
            .complete_with_outcome("conversation-a", true)
            .unwrap()
            .unwrap();
        assert_eq!(restarted.worker_index, first.worker_index);
        assert_eq!(restarted.worker_generation, 2);
        assert_eq!(pool.worker_generation(first.worker_index).unwrap(), 2);
        assert_eq!(pool.active_worker_count().unwrap(), 1);

        assert!(pool.complete_with_outcome("conversation-a", true).unwrap().is_none());
        assert_eq!(pool.active_worker_count().unwrap(), 0);

        let third = match pool.submit(job("conversation-b", 3)).unwrap() {
            TranscriptMirrorSubmission::Dispatch(dispatch) => dispatch,
            other => panic!("expected dispatch, got {other:?}"),
        };
        assert_eq!(third.worker_generation, 3);
    }

    #[test]
    fn different_conversations_have_independent_lanes_and_close_fails_closed() {
        let pool = TranscriptMirrorOffloadPool::default();
        assert!(matches!(
            pool.submit(job("conversation-a", 1)).unwrap(),
            TranscriptMirrorSubmission::Dispatch(_)
        ));
        assert!(matches!(
            pool.submit(job("conversation-b", 2)).unwrap(),
            TranscriptMirrorSubmission::Dispatch(_)
        ));
        assert_eq!(pool.active_lane_count().unwrap(), 2);
        assert_eq!(pool.close().unwrap(), 0);
        assert_eq!(
            pool.submit(job("conversation-c", 3)).unwrap_err(),
            TranscriptMirrorOffloadError::Closed
        );
    }

    #[derive(Default)]
    struct InlineStore(std::collections::HashMap<Vec<u8>, Vec<u8>>);

    impl TranscriptOccurrenceBlobStore for InlineStore {
        fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
            self.0.get(id).cloned()
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-ios-transcript-offload-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn wrapper_prefers_incremental_and_falls_back_to_full_inline() {
        let root = temp_root("inline");
        let mut store = InlineStore::default();
        store.0.insert(
            vec![1],
            br#"{"role":"user","content":"one"}"#.to_vec(),
        );
        store.0.insert(
            vec![2],
            br#"{"role":"assistant","content":"two"}"#.to_vec(),
        );
        let mirror = OffloadingTranscriptMirror::with_executor(
            Arc::new(TranscriptMirrorOffloadPool::default()),
            OffloadingTranscriptMirrorOptions {
                blob_db_paths: vec![],
                transcripts_dir: root.clone(),
            },
            0,
            Arc::new(|_| panic!("inline fallback must not offload")),
        );

        let first = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1]],
            turns: vec![],
            summary_archives: vec![],
        };
        mirror.write_inline("agent-inline", &first, &store).unwrap();
        assert_eq!(mirror.previous_root_prompt_count(), 1);

        let second = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1], vec![2]],
            turns: vec![],
            summary_archives: vec![],
        };
        mirror.write_inline("agent-inline", &second, &store).unwrap();
        assert_eq!(mirror.previous_root_prompt_count(), 2);
        let body = std::fs::read_to_string(
            root.join("agent-inline").join("agent-inline.jsonl"),
        )
        .unwrap();
        assert!(body.contains("one"));
        assert!(body.contains("two"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn worker_success_advances_cursor_only_after_executor_settles() {
        let root = temp_root("worker-success");
        let calls = Arc::new(Mutex::new(Vec::<TranscriptMirrorJob>::new()));
        let captured = calls.clone();
        let mirror = OffloadingTranscriptMirror::with_executor(
            Arc::new(TranscriptMirrorOffloadPool::new(1)),
            OffloadingTranscriptMirrorOptions {
                blob_db_paths: vec![root.join("conversation.sqlite")],
                transcripts_dir: root.join("transcripts"),
            },
            0,
            Arc::new(move |job| {
                captured.lock().unwrap().push(job.clone());
                Ok(true)
            }),
        );
        let state = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![7], vec![8]],
            turns: vec![],
            summary_archives: vec![],
        };

        mirror
            .write_worker("agent-worker", &state, Some(&[9, 9]))
            .unwrap();
        assert_eq!(mirror.previous_root_prompt_count(), 2);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].conversation_id, "agent-worker");
        assert_eq!(calls[0].state_blob_id, vec![9, 9]);
        drop(calls);
        assert_eq!(mirror.pool.active_dispatch_count().unwrap(), 0);
        assert_eq!(mirror.pool.active_lane_count().unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn queued_successor_cannot_promote_a_failed_primary_cursor() {
        let root = temp_root("queued-successor");
        let pool = Arc::new(TranscriptMirrorOffloadPool::new(1));
        let executor_pool = pool.clone();
        let calls = Arc::new(Mutex::new(0usize));
        let executor_calls = calls.clone();
        let mirror = OffloadingTranscriptMirror::with_executor(
            pool.clone(),
            OffloadingTranscriptMirrorOptions {
                blob_db_paths: vec![root.join("conversation.sqlite")],
                transcripts_dir: root.join("transcripts"),
            },
            0,
            Arc::new(move |job| {
                let mut count = executor_calls.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    assert_eq!(job.state_blob_id, vec![1]);
                    assert_eq!(
                        executor_pool
                            .submit(TranscriptMirrorJob {
                                conversation_id: job.conversation_id.clone(),
                                state_blob_id: vec![2],
                                blob_db_paths: job.blob_db_paths.clone(),
                                transcripts_dir: job.transcripts_dir.clone(),
                            })
                            .unwrap(),
                        TranscriptMirrorSubmission::Coalesced
                    );
                    Ok(false)
                } else {
                    assert_eq!(job.state_blob_id, vec![2]);
                    Ok(true)
                }
            }),
        );
        let state = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![7], vec![8], vec![9]],
            turns: vec![],
            summary_archives: vec![],
        };

        mirror
            .write_worker("agent-worker", &state, Some(&[1]))
            .unwrap();

        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(mirror.previous_root_prompt_count(), 0);
        assert_eq!(pool.active_dispatch_count().unwrap(), 0);
        assert_eq!(pool.queued_job_count().unwrap(), 0);
        assert_eq!(pool.active_lane_count().unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn worker_missing_false_and_error_results_never_claim_progress() {
        let state = TranscriptMirrorConversationState {
            root_prompt_messages_json: vec![vec![1]],
            turns: vec![],
            summary_archives: vec![],
        };

        for (label, executor) in [
            (
                "false",
                Arc::new(|_: &TranscriptMirrorJob| Ok(false))
                    as TranscriptMirrorWorkerExecutor,
            ),
            (
                "error",
                Arc::new(|_: &TranscriptMirrorJob| Err("worker failed".into()))
                    as TranscriptMirrorWorkerExecutor,
            ),
        ] {
            let root = temp_root(label);
            let mirror = OffloadingTranscriptMirror::with_executor(
                Arc::new(TranscriptMirrorOffloadPool::new(1)),
                OffloadingTranscriptMirrorOptions {
                    blob_db_paths: vec![root.join("conversation.sqlite")],
                    transcripts_dir: root.join("transcripts"),
                },
                0,
                executor,
            );
            mirror
                .write_worker("agent-worker", &state, Some(&[4]))
                .unwrap();
            assert_eq!(mirror.previous_root_prompt_count(), 0);
            assert_eq!(mirror.pool.active_dispatch_count().unwrap(), 0);
            let _ = std::fs::remove_dir_all(root);
        }

        let root = temp_root("missing");
        let mirror = OffloadingTranscriptMirror::with_executor(
            Arc::new(TranscriptMirrorOffloadPool::new(1)),
            OffloadingTranscriptMirrorOptions {
                blob_db_paths: vec![root.join("conversation.sqlite")],
                transcripts_dir: root.join("transcripts"),
            },
            0,
            Arc::new(|_| panic!("missing state id must not execute worker")),
        );
        mirror.write_worker("agent-worker", &state, None).unwrap();
        assert_eq!(mirror.previous_root_prompt_count(), 0);
        assert_eq!(mirror.pool.active_dispatch_count().unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
