use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::{AttemptCheckpoint, TransientStreamError};


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamAttemptGeneration(u64);

impl StreamAttemptGeneration {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Canonical Runner owner for one generated-Agent stream attempt lifecycle.
///
/// The frozen TypeScript owner keeps an object identity fence plus resettable
/// first-token timer hooks around each attempt. Rust cannot retain borrowed
/// callbacks after run_attempt returns, but production provider callbacks may
/// still cross worker/thread boundaries. Keep the fence explicit so stale
/// callbacks are rejected by behavior rather than relying only on lifetimes.
#[derive(Debug)]
pub struct StreamAttemptRuntime {
    policy: StreamAttemptPolicy,
    active_generation: AtomicU64,
    next_generation: AtomicU64,
    stream_output_produced: AtomicBool,
    deadline_epoch: AtomicU64,
    deadline_disarmed: AtomicBool,
}

impl StreamAttemptRuntime {
    pub fn new(policy: StreamAttemptPolicy) -> Self {
        Self {
            policy,
            active_generation: AtomicU64::new(0),
            next_generation: AtomicU64::new(0),
            stream_output_produced: AtomicBool::new(false),
            deadline_epoch: AtomicU64::new(0),
            deadline_disarmed: AtomicBool::new(true),
        }
    }

    pub fn policy(&self) -> &StreamAttemptPolicy {
        &self.policy
    }

    pub fn begin_attempt(&self) -> StreamAttemptGeneration {
        let generation = self
            .next_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        // Fence the prior attempt before publishing any mutable state for the
        // next one. A late callback from the previous provider generation must
        // never observe freshly reset output/deadline state as its own.
        self.active_generation.store(0, Ordering::Release);
        self.stream_output_produced.store(false, Ordering::Release);
        self.deadline_disarmed.store(false, Ordering::Release);
        self.deadline_epoch.fetch_add(1, Ordering::AcqRel);
        self.active_generation.store(generation, Ordering::Release);
        StreamAttemptGeneration(generation)
    }

    pub fn is_current(&self, generation: StreamAttemptGeneration) -> bool {
        generation.0 != 0
            && self.active_generation.load(Ordering::Acquire) == generation.0
    }

    pub fn stream_output_produced(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        self.is_current(generation)
            && self.stream_output_produced.load(Ordering::Acquire)
    }

    pub fn mark_stream_output(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.stream_output_produced.store(true, Ordering::Release);
        self.deadline_disarmed.store(true, Ordering::Release);
        true
    }

    /// A durable tool-boundary checkpoint proves that this attempt made
    /// forward progress, but it also starts a new provider continuation whose
    /// first output must remain bounded. Re-arm the same attempt-generation
    /// deadline without clearing the attempt's aggregate output evidence.
    pub fn rearm_output_deadline_after_checkpoint(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.deadline_disarmed.store(false, Ordering::Release);
        self.deadline_epoch.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// Equivalent to the frozen reset-first-token-deadline hook. A reset is
    /// ignored after output or after this attempt has been superseded.
    pub fn reset_first_output_deadline(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        if !self.is_current(generation)
            || self.stream_output_produced.load(Ordering::Acquire)
            || self.deadline_disarmed.load(Ordering::Acquire)
        {
            return false;
        }
        self.deadline_epoch.fetch_add(1, Ordering::AcqRel);
        true
    }

    pub fn disarm_first_output_deadline(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.deadline_disarmed.store(true, Ordering::Release);
        true
    }

    pub fn deadline_epoch(
        &self,
        generation: StreamAttemptGeneration,
    ) -> Option<u64> {
        self.is_current(generation)
            .then(|| self.deadline_epoch.load(Ordering::Acquire))
    }

    pub fn deadline_armed(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        self.is_current(generation)
            && !self.deadline_disarmed.load(Ordering::Acquire)
    }

    pub fn settle_attempt(
        &self,
        generation: StreamAttemptGeneration,
    ) -> bool {
        if self
            .active_generation
            .compare_exchange(
                generation.0,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        self.deadline_disarmed.store(true, Ordering::Release);
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AttemptProgress {
    pub output_events: u64,
    pub output_bytes: usize,
    pub checkpoint: Option<AttemptCheckpoint>,
}

impl AttemptProgress {
    pub fn record_output(&mut self, bytes: usize) {
        self.output_events = self.output_events.saturating_add(1);
        self.output_bytes = self.output_bytes.saturating_add(bytes);
    }

    pub fn saw_output(&self) -> bool {
        self.output_events > 0 || self.output_bytes > 0
    }

    pub fn resumable(&self) -> bool {
        self.checkpoint.as_ref().is_some_and(AttemptCheckpoint::is_resumable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamAttemptPolicy {
    pub first_output_timeout: Duration,
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub max_retry_after: Duration,
}

impl Default for StreamAttemptPolicy {
    fn default() -> Self {
        Self {
            first_output_timeout: Duration::from_secs(150),
            max_attempts: 3,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(4),
            max_retry_after: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    RetryAfter(Duration),
    ResumeAfter { delay: Duration, checkpoint: AttemptCheckpoint },
    Fail,
}

impl StreamAttemptPolicy {
    pub fn retry_decision(
        &self,
        attempt: u32,
        progress: &AttemptProgress,
        error: &TransientStreamError,
    ) -> RetryDecision {
        if attempt >= self.max_attempts || !error.retryable() {
            return RetryDecision::Fail;
        }

        let delay = error
            .retry_after_ms
            .map(Duration::from_millis)
            .map(|value| value.min(self.max_retry_after))
            .unwrap_or_else(|| {
                let exponent = attempt.saturating_sub(1).min(8);
                self.initial_backoff
                    .saturating_mul(1_u32 << exponent)
                    .min(self.max_backoff)
            });

        if !progress.saw_output() {
            return RetryDecision::RetryAfter(delay);
        }
        if let Some(checkpoint) = progress.checkpoint.clone().filter(AttemptCheckpoint::is_resumable) {
            return RetryDecision::ResumeAfter { delay, checkpoint };
        }
        RetryDecision::Fail
    }
}

#[derive(Debug)]
pub struct StreamWatchdog {
    started: Instant,
    first_output_timeout: Duration,
    first_output_seen: bool,
}

impl StreamWatchdog {
    pub fn new(policy: &StreamAttemptPolicy) -> Self {
        Self {
            started: Instant::now(),
            first_output_timeout: policy.first_output_timeout,
            first_output_seen: false,
        }
    }

    pub fn mark_output(&mut self) {
        self.first_output_seen = true;
    }

    pub fn expired(&self) -> bool {
        !self.first_output_seen && self.started.elapsed() >= self.first_output_timeout
    }

    pub fn expired_at(&self, elapsed: Duration) -> bool {
        !self.first_output_seen && elapsed >= self.first_output_timeout
    }
}


use std::future::Future;
use std::pin::Pin;

pub type OuterStreamFuture<'a, T> =
    Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamCancelReason {
    pub intentional: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OuterCheckpointDisposition {
    Persisted,
    IgnoredStaleGeneration,
}

/// Frozen outer persistence contract around a generated Agent stream attempt.
///
/// This is deliberately independent from the provider-retry checkpoint store:
/// State is the real Agent ConversationStateStructure projection, while the
/// routed-provider store remains only a retry/resume transport checkpoint.
pub trait OuterStreamPersistence<Context, State>: Send + Sync {
    fn generation(&self) -> u64;
    fn run_generation(&self) -> u64;

    fn prepare_checkpoint_for_persistence(&self, checkpoint: &mut State);

    fn persist_step_checkpoint<'a>(
        &'a self,
        context: &'a Context,
        checkpoint: &'a State,
    ) -> OuterStreamFuture<'a, Result<(), String>>;

    fn note_checkpoint(&self, _checkpoint: &State) {}

    fn persist_final_state<'a>(
        &'a self,
        context: &'a Context,
        checkpoint: &'a State,
    ) -> OuterStreamFuture<'a, Result<(), String>>;

    fn commit_disk_pressure_reminder(&self);
    fn release_disk_pressure_reminder(&self);

    fn note_automation_status_reminder(&self) {}

    fn is_awaiting_user_selection(&self) -> bool;
    fn is_quiescing_for_upgrade(&self) -> bool;
    fn mark_quiesced_for_upgrade(&self);
    fn cancel_run(&self, cancellation: StreamCancelReason);
}

pub async fn persist_outer_stream_checkpoint<P, Context, State>(
    persistence: &P,
    context: &Context,
    checkpoint: &mut State,
) -> Result<OuterCheckpointDisposition, String>
where
    P: OuterStreamPersistence<Context, State>,
{
    if persistence.run_generation() != persistence.generation() {
        return Ok(OuterCheckpointDisposition::IgnoredStaleGeneration);
    }

    persistence.prepare_checkpoint_for_persistence(checkpoint);
    persistence
        .persist_step_checkpoint(context, checkpoint)
        .await?;
    persistence.note_checkpoint(checkpoint);
    persistence.commit_disk_pressure_reminder();
    persistence.note_automation_status_reminder();

    if persistence.is_awaiting_user_selection() {
        persistence.cancel_run(StreamCancelReason {
            intentional: true,
            reason: "awaiting user selection".into(),
        });
    } else if persistence.is_quiescing_for_upgrade() {
        persistence.mark_quiesced_for_upgrade();
        persistence.cancel_run(StreamCancelReason {
            intentional: true,
            reason: "quiescing for forced host upgrade".into(),
        });
    }

    Ok(OuterCheckpointDisposition::Persisted)
}

pub async fn persist_outer_stream_final_state<P, Context, State>(
    persistence: &P,
    context: &Context,
    final_state: &State,
) -> Result<OuterCheckpointDisposition, String>
where
    P: OuterStreamPersistence<Context, State>,
{
    if persistence.run_generation() != persistence.generation() {
        return Ok(OuterCheckpointDisposition::IgnoredStaleGeneration);
    }
    persistence.persist_final_state(context, final_state).await?;
    persistence.commit_disk_pressure_reminder();
    Ok(OuterCheckpointDisposition::Persisted)
}

/// Mirrors the frozen outer finally-owner: callers must invoke this exactly
/// once after attempt cleanup/final persistence, including failure paths.
pub fn release_outer_stream_persistence<P, Context, State>(persistence: &P)
where
    P: OuterStreamPersistence<Context, State>,
{
    persistence.release_disk_pressure_reminder();
}
