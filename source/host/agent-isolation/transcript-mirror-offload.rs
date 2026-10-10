use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
        }
    }

    pub fn worker_index_for(&self, conversation_id: &str) -> usize {
        let mut hash = 0i32;
        for unit in conversation_id.encode_utf16() {
            hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
        }
        (hash.unsigned_abs() as usize) % self.max_workers
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
                Ok(TranscriptMirrorSubmission::Dispatch(TranscriptMirrorDispatch {
                    worker_index,
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
        let mut lanes = self
            .lanes
            .lock()
            .map_err(|_| TranscriptMirrorOffloadError::StatePoisoned)?;

        let queued = lanes
            .get_mut(conversation_id)
            .and_then(|lane| lane.queued.take());
        if let Some(queued) = queued {
            return Ok(Some(TranscriptMirrorDispatch {
                worker_index: self.worker_index_for(conversation_id),
                job: queued.job,
                waiter_count: queued.waiter_count,
            }));
        }
        lanes.remove(conversation_id);
        Ok(None)
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
        Ok(queued_waiters)
    }
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
}
