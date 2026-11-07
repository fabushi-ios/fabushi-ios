use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StoreDbCaptureFailurePhase {
    Capture,
    BlobUpload,
    ManifestCommit,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreDbCaptureTrace {
    pub queue_duration_ms: u64,
    pub capture_duration_ms: u64,
    pub blob_upload_duration_ms: u64,
    pub manifest_commit_duration_ms: u64,
    pub failure_phase: Option<StoreDbCaptureFailurePhase>,
}

pub fn create_store_db_capture_trace() -> StoreDbCaptureTrace {
    StoreDbCaptureTrace::default()
}

pub fn record_store_db_capture_failure(
    trace: &mut StoreDbCaptureTrace,
    phase: StoreDbCaptureFailurePhase,
) {
    if trace.failure_phase.is_none_or(|current| phase > current) {
        trace.failure_phase = Some(phase);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbBundleTarget {
    pub rel_path: String,
    pub abs_path: String,
    pub effective_size: u64,
}

#[derive(Debug, Default, Clone)]
pub struct AgentDbCaptureQueues {
    locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

impl AgentDbCaptureQueues {
    pub fn run_serialized<T>(&self, agent_id: &str, operation: impl FnOnce() -> T) -> T {
        self.run_serialized_with_queue_duration(agent_id, |_| operation())
    }

    pub fn run_serialized_with_queue_duration<T>(
        &self,
        agent_id: &str,
        operation: impl FnOnce(u64) -> T,
    ) -> T {
        let queued_at = Instant::now();
        let (agent_lock, was_queued) = {
            let mut locks = self
                .locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let was_queued = locks.contains_key(agent_id);
            let agent_lock = Arc::clone(
                locks
                    .entry(agent_id.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            );
            (agent_lock, was_queued)
        };
        let guard = agent_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let queue_duration_ms = if was_queued {
            queued_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        } else {
            0
        };
        let result = operation(queue_duration_ms);
        drop(guard);

        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if Arc::strong_count(&agent_lock) == 2
            && locks
                .get(agent_id)
                .is_some_and(|current| Arc::ptr_eq(current, &agent_lock))
        {
            locks.remove(agent_id);
        }
        result
    }

    pub fn active_agent_count(&self) -> usize {
        self.locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDbCaptureSignature {
    pub write_generation: u64,
    pub mtime_ms: u64,
    pub size: u64,
    pub sha: String,
    pub mode: u32,
    pub wal_size: u64,
    pub wal_mtime_ms: u64,
}

pub fn bundle_identity_matches(
    before: &StoreDbCaptureSignature,
    after: &StoreDbCaptureSignature,
) -> bool {
    before == after
}

pub fn elapsed_duration_ms(now_ms: u64, started_at_ms: u64) -> u64 {
    now_ms.saturating_sub(started_at_ms)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    use std::thread;
    use std::time::Duration;

    #[test]
    fn same_agent_capture_queue_is_serial() {
        let queues = Arc::new(AgentDbCaptureQueues::default());
        let barrier = Arc::new(Barrier::new(3));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..2 {
            let queues = Arc::clone(&queues);
            let barrier = Arc::clone(&barrier);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            handles.push(thread::spawn(move || {
                barrier.wait();
                queues.run_serialized("agent-a", || {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(current, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(30));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }));
        }

        barrier.wait();
        for handle in handles {
            handle.join().expect("capture worker");
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(queues.active_agent_count(), 0);
    }
}
