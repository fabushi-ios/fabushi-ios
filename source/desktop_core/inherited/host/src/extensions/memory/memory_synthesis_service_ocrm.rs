//! OCRM Memory Synthesis Service Submodule (OCRM-MODULAR-01)
//!
//! Decomposed from `memory_synthesis_service.rs` to satisfy single responsibility
//! and cohesive file size budget. Implements `MemorySynthesisService` background thread loop,
//! queue management, scheduling, and `MemorySynthesisBridge` integration.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc, Condvar, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone, Utc};
use serde_json::Value;

use super::memory_service::{
    FileMemoryStore, MemorySynthesisBridge, SynthesisApplyResult, SynthesisSnapshot,
};
use super::memory_synthesis_service::*;

pub struct MemorySynthesisOptions {
    pub list_targets: ListTargets,
    pub get_target: GetTarget,
    pub propose: Propose,
    pub verify: Option<Verify>,
    pub debounce: Duration,
    pub deadline: Duration,
    pub poll_interval: Duration,
    pub retry_attempts: usize,
    pub retry_initial: Duration,
    pub retry_max: Duration,
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub report: Option<Report>,
}

impl MemorySynthesisOptions {
    pub fn new(list_targets: ListTargets, get_target: GetTarget, propose: Propose) -> Self {
        Self {
            list_targets,
            get_target,
            propose,
            verify: None,
            debounce: Duration::from_millis(MEMORY_SYNTHESIS_DEBOUNCE_MS),
            deadline: Duration::from_millis(MEMORY_SYNTHESIS_DEADLINE_MS),
            poll_interval: Duration::from_millis(MEMORY_SYNTHESIS_POLL_INTERVAL_MS),
            retry_attempts: MEMORY_SYNTHESIS_RETRY_ATTEMPTS,
            retry_initial: Duration::from_millis(MEMORY_SYNTHESIS_RETRY_INITIAL_MS),
            retry_max: Duration::from_millis(MEMORY_SYNTHESIS_RETRY_MAX_MS),
            now: Arc::new(|| Utc::now().timestamp_millis()),
            report: None,
        }
    }
}

#[derive(Default)]
struct MemorySynthesisState {
    queue: MemorySynthesisQueue,
    started: bool,
    disposed: bool,
    active: bool,
}

enum BackgroundCommand {
    Debounce,
    Stop,
}

pub struct MemorySynthesisService {
    options: MemorySynthesisOptions,
    state: Mutex<MemorySynthesisState>,
    cancel: SynthesisCancelSignal,
    background_tx: Mutex<Option<mpsc::Sender<BackgroundCommand>>>,
    background_worker: Mutex<Option<JoinHandle<()>>>,
}

impl MemorySynthesisService {
    pub fn new(options: MemorySynthesisOptions) -> Self {
        Self {
            options,
            state: Mutex::new(MemorySynthesisState::default()),
            cancel: SynthesisCancelSignal::default(),
            background_tx: Mutex::new(None),
            background_worker: Mutex::new(None),
        }
    }

    pub fn start(&self) {
        {
            let mut state = self.state.lock().expect("memory synthesis state poisoned");
            if state.started || state.disposed {
                return;
            }
            state.started = true;
        }
        self.queue_temporal_targets();
    }

    pub fn start_background(self: &Arc<Self>) {
        self.start();
        let (tx, rx) = mpsc::channel();
        {
            let mut slot = self
                .background_tx
                .lock()
                .expect("memory synthesis background lock poisoned");
            if slot.is_some() {
                return;
            }
            *slot = Some(tx.clone());
        }

        let weak = Arc::downgrade(self);
        let debounce = self.options.debounce;
        let poll_interval = self.options.poll_interval;
        let worker = thread::Builder::new()
            .name("sand-memory-synthesis".into())
            .spawn(move || memory_synthesis_background_loop(weak, rx, debounce, poll_interval))
            .expect("memory synthesis background worker must start");
        *self
            .background_worker
            .lock()
            .expect("memory synthesis worker lock poisoned") = Some(worker);

        if !self
            .state
            .lock()
            .expect("memory synthesis state poisoned")
            .queue
            .is_empty()
        {
            let _ = tx.send(BackgroundCommand::Debounce);
        }
    }

    fn queue_temporal_targets(&self) {
        let now = (self.options.now)();
        let targets = (self.options.list_targets)();
        let mut state = self.state.lock().expect("memory synthesis state poisoned");
        if !state.started || state.disposed {
            return;
        }
        let active = state.active;
        let mut queued = 0usize;
        for (agent_id, target) in targets {
            if queued >= MAX_TEMPORAL_TARGETS_PER_SWEEP {
                break;
            }
            if target.has_memories() && target.is_temporal_review_due(now) {
                if state.queue.queue_temporal(&agent_id, active) {
                    queued += 1;
                }
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        let state = self.state.lock().expect("memory synthesis state poisoned");
        state.started && !state.disposed
    }

    pub fn record_turn(
        &self,
        agent_id: &str,
        evidence_id: Option<String>,
        user: &str,
        assistant: &str,
        occurred_at: i64,
    ) {
        let dropped = {
            let mut state = self.state.lock().expect("memory synthesis state poisoned");
            if !state.started || state.disposed {
                return;
            }
            let active = state.active;
            state.queue.record_turn(
                agent_id,
                evidence_id.as_deref(),
                occurred_at,
                user,
                assistant,
                active,
            )
        };
        for (dropped_agent, evidence_count) in dropped {
            let started = (self.options.now)();
            self.report(
                SynthesisReportOutcome::Dropped,
                Some(dropped_agent),
                evidence_count,
                0,
                0,
                started,
            );
        }
        if let Some(tx) = self
            .background_tx
            .lock()
            .expect("memory synthesis background lock poisoned")
            .as_ref()
            .cloned()
        {
            let _ = tx.send(BackgroundCommand::Debounce);
        }
    }

    pub fn run_now(&self) -> Vec<SynthesisOutcome> {
        {
            let mut state = self.state.lock().expect("memory synthesis state poisoned");
            if !state.started || state.disposed || state.active {
                return Vec::new();
            }
            state.active = true;
        }

        let agent_ids = {
            self.state
                .lock()
                .expect("memory synthesis state poisoned")
                .queue
                .agent_ids()
        };
        let mut outcomes = Vec::with_capacity(agent_ids.len());
        for agent_id in agent_ids {
            if self.cancel.is_cancelled() {
                break;
            }
            outcomes.push(self.run_agent(&agent_id));
        }

        let needs_another_pass = {
            let mut state = self.state.lock().expect("memory synthesis state poisoned");
            state.active = false;
            state.queue.take_needs_another_pass() && !state.disposed
        };
        if needs_another_pass {
            if let Some(tx) = self
                .background_tx
                .lock()
                .expect("memory synthesis background lock poisoned")
                .as_ref()
                .cloned()
            {
                let _ = tx.send(BackgroundCommand::Debounce);
            }
        }
        outcomes
    }

    pub fn dispose(&self) {
        self.cancel.cancel_lifetime();
        if let Some(tx) = self
            .background_tx
            .lock()
            .expect("memory synthesis background lock poisoned")
            .take()
        {
            let _ = tx.send(BackgroundCommand::Stop);
        }
        if let Some(worker) = self
            .background_worker
            .lock()
            .expect("memory synthesis worker lock poisoned")
            .take()
        {
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
        let mut state = self.state.lock().expect("memory synthesis state poisoned");
        state.disposed = true;
        state.queue = MemorySynthesisQueue::default();
    }

    fn run_agent(&self, agent_id: &str) -> SynthesisOutcome {
        let (evidence, temporal) = {
            let state = self.state.lock().expect("memory synthesis state poisoned");
            let Some(snapshot) = state.queue.snapshot(agent_id) else {
                return SynthesisOutcome::NoWork;
            };
            snapshot
        };
        let Some(target) = (self.options.get_target)(agent_id) else {
            self.finish(agent_id, &evidence, temporal);
            return SynthesisOutcome::NoWork;
        };
        let snapshot = target.prepare_synthesis();
        let started = (self.options.now)();

        if snapshot.memories.is_empty() && evidence.is_empty() {
            if temporal {
                target.mark_temporal_review(started);
            }
            self.finish(agent_id, &evidence, temporal);
            self.report(
                SynthesisReportOutcome::NoWork,
                Some(agent_id.to_string()),
                0,
                0,
                0,
                started,
            );
            return SynthesisOutcome::NoWork;
        }

        let request = SynthesisProposalRequest {
            today: Utc
                .timestamp_millis_opt(started)
                .single()
                .unwrap_or_else(Utc::now)
                .format("%Y-%m-%d")
                .to_string(),
            current_memories: snapshot.memories.clone(),
            new_evidence: evidence.clone(),
        };

        let changes = match self.perform_with_retry(
            request,
            snapshot.clone(),
            evidence.clone(),
            temporal,
        ) {
            Ok(changes) => changes,
            Err(error) => {
                if self.cancel.is_cancelled() {
                    return SynthesisOutcome::NoWork;
                }
                if temporal {
                    target.mark_temporal_review(started);
                }
                self.finish(agent_id, &evidence, temporal);
                let outcome = error.outcome();
                self.report(
                    error.report_outcome(),
                    Some(agent_id.to_string()),
                    evidence.len(),
                    snapshot.memories.len(),
                    error.proposed_count(),
                    started,
                );
                return outcome;
            }
        };

        if changes.is_empty() {
            if temporal {
                target.mark_temporal_review(started);
            }
            self.finish(agent_id, &evidence, temporal);
            self.report(
                SynthesisReportOutcome::NoWork,
                Some(agent_id.to_string()),
                evidence.len(),
                snapshot.memories.len(),
                0,
                started,
            );
            return SynthesisOutcome::NoWork;
        }

        let store_changes = changes
            .iter()
            .map(MemoryChange::to_store_change)
            .collect::<Vec<_>>();
        let outcome = match target.apply_synthesis(&snapshot, &store_changes, started) {
            SynthesisApplyResult::Committed => SynthesisOutcome::Committed,
            SynthesisApplyResult::Stale => SynthesisOutcome::Stale,
            SynthesisApplyResult::Invalid => SynthesisOutcome::InvalidOutput,
        };
        if outcome == SynthesisOutcome::Stale {
            self.state
                .lock()
                .expect("memory synthesis state poisoned")
                .queue
                .mark_stale();
        } else {
            if temporal && outcome != SynthesisOutcome::Committed {
                target.mark_temporal_review(started);
            }
            self.finish(agent_id, &evidence, temporal);
        }
        let report_outcome = match outcome {
            SynthesisOutcome::Committed => SynthesisReportOutcome::Committed,
            SynthesisOutcome::NoWork => SynthesisReportOutcome::NoWork,
            SynthesisOutcome::InvalidOutput => SynthesisReportOutcome::InvalidOutput,
            SynthesisOutcome::Rejected => SynthesisReportOutcome::Rejected,
            SynthesisOutcome::Stale => SynthesisReportOutcome::Stale,
            SynthesisOutcome::Failed => SynthesisReportOutcome::Failed,
        };
        self.report(
            report_outcome,
            Some(agent_id.to_string()),
            evidence.len(),
            snapshot.memories.len(),
            changes.len(),
            started,
        );
        outcome
    }

    fn perform_with_retry(
        &self,
        request: SynthesisProposalRequest,
        snapshot: SynthesisSnapshot,
        evidence: Vec<MemoryEvidence>,
        temporal: bool,
    ) -> Result<Vec<MemoryChange>, SynthesisAttemptError> {
        let attempts = self.options.retry_attempts.max(1);
        let mut delay = self.options.retry_initial;
        for attempt in 0..attempts {
            if self.cancel.is_cancelled() {
                return Err(SynthesisAttemptError::Failed);
            }
            match self.run_attempt_with_deadline(
                request.clone(),
                snapshot.clone(),
                evidence.clone(),
                temporal,
            ) {
                Ok(changes) => return Ok(changes),
                Err(error) if attempt + 1 < attempts => {
                    if !self.wait_retry_delay(delay.min(self.options.retry_max)) {
                        return Err(SynthesisAttemptError::Failed);
                    }
                    delay = delay
                        .checked_mul(2)
                        .unwrap_or(self.options.retry_max)
                        .min(self.options.retry_max);
                    let _ = error;
                }
                Err(error) => return Err(error),
            }
        }
        Err(SynthesisAttemptError::Failed)
    }

    fn run_attempt_with_deadline(
        &self,
        request: SynthesisProposalRequest,
        snapshot: SynthesisSnapshot,
        evidence: Vec<MemoryEvidence>,
        temporal: bool,
    ) -> Result<Vec<MemoryChange>, SynthesisAttemptError> {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let propose = Arc::clone(&self.options.propose);
        let verify = self.options.verify.clone();
        let cancel = self.cancel.child();
        let worker_cancel = cancel.clone();
        thread::Builder::new()
            .name("sand-memory-synthesis-inference".into())
            .spawn(move || {
                let result = perform_synthesis_attempt(
                    propose,
                    verify,
                    request,
                    snapshot,
                    evidence,
                    temporal,
                    worker_cancel,
                );
                let _ = result_tx.send(result);
            })
            .map_err(|_| SynthesisAttemptError::Failed)?;

        match result_rx.recv_timeout(self.options.deadline) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                cancel.cancel();
                Err(SynthesisAttemptError::Failed)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SynthesisAttemptError::Failed),
        }
    }

    fn wait_retry_delay(&self, delay: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < delay {
            if self.cancel.is_cancelled() {
                return false;
            }
            let remaining = delay.saturating_sub(started.elapsed());
            thread::sleep(remaining.min(Duration::from_millis(25)));
        }
        !self.cancel.is_cancelled()
    }

    fn report(
        &self,
        outcome: SynthesisReportOutcome,
        agent_id: Option<String>,
        evidence_count: usize,
        input_memory_count: usize,
        change_count: usize,
        started_at: i64,
    ) {
        let Some(report) = &self.options.report else {
            return;
        };
        report(SynthesisReport {
            outcome,
            agent_id,
            evidence_count,
            input_memory_count,
            change_count,
            duration_ms: (self.options.now)().saturating_sub(started_at).max(0),
        });
    }

    fn finish(&self, agent_id: &str, evidence: &[MemoryEvidence], temporal: bool) {
        let consumed = evidence
            .iter()
            .map(|item| item.id.clone())
            .collect::<HashSet<_>>();
        self.state
            .lock()
            .expect("memory synthesis state poisoned")
            .queue
            .finish(agent_id, &consumed, temporal);
    }
}


fn memory_synthesis_background_loop(
    service: Weak<MemorySynthesisService>,
    rx: mpsc::Receiver<BackgroundCommand>,
    debounce: Duration,
    poll_interval: Duration,
) {
    let mut next_run: Option<Instant> = None;
    let mut next_poll = Instant::now()
        .checked_add(poll_interval)
        .unwrap_or_else(Instant::now);

    loop {
        let now = Instant::now();
        let deadline = match next_run {
            Some(run_at) if run_at <= next_poll => run_at,
            _ => next_poll,
        };
        let timeout = deadline.saturating_duration_since(now);
        match rx.recv_timeout(timeout) {
            Ok(BackgroundCommand::Debounce) => {
                next_run = Some(
                    Instant::now()
                        .checked_add(debounce)
                        .unwrap_or_else(Instant::now),
                );
            }
            Ok(BackgroundCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let Some(service) = service.upgrade() else {
                    return;
                };
                if service.cancel.is_cancelled() {
                    return;
                }
                let now = Instant::now();
                let poll_due = now >= next_poll;
                let run_due = next_run.is_some_and(|run_at| now >= run_at);
                if poll_due {
                    service.queue_temporal_targets();
                    next_poll = now.checked_add(poll_interval).unwrap_or(now);
                }
                if run_due {
                    next_run = None;
                }
                if poll_due || run_due {
                    let _ = service.run_now();
                }
            }
        }
    }
}

impl MemorySynthesisBridge for MemorySynthesisService {
    fn is_enabled(&self) -> bool {
        MemorySynthesisService::is_enabled(self)
    }

    fn record_turn(
        &self,
        agent_id: &str,
        evidence_id: Option<String>,
        user: &str,
        assistant: &str,
        occurred_at: i64,
    ) {
        MemorySynthesisService::record_turn(
            self,
            agent_id,
            evidence_id,
            user,
            assistant,
            occurred_at,
        );
    }
}
