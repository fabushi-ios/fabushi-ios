use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProviderCheckpoint,
};

use super::routed_provider_runtime::RoutedProviderCancellation;
use super::tools::box_help_tool::WAITING_USER_CANCELLATION_PREFIX;
use super::{
    AttemptCheckpoint, AttemptProgress, RetryDecision, StreamAttemptGeneration,
    StreamAttemptPolicy, StreamAttemptRuntime,
    StreamFailureKind, TransientStreamError, TurnRunFinished, TurnRunOptions,
    TurnRunShell, TurnRunShellError, TurnRunStarted,
};

pub const DEFAULT_WATCHDOG_POLL_INTERVAL: Duration =
    Duration::from_millis(10);

pub struct ProductionTurnRunShellPreparedTurn<T> {
    pub started: TurnRunStarted,
    pub prompt: String,
    pub prepared: T,
}

/// Bind one concrete production turn to the canonical TurnRunShell lifecycle.
///
/// The caller freezes all turn-scoped owner/input state in `prepare`, then
/// executes only that prepared snapshot. Settlement and unwind remain owned by
/// this adapter so production code cannot accidentally create a parallel turn
/// lifecycle around the generated Agent stream.
pub fn run_production_turn_shell_lifecycle<Prepared, Prepare, Execute, Unwind>(
    shell: &mut TurnRunShell,
    last_finished: &mut Option<TurnRunFinished>,
    messages: &[ProviderMessage],
    options: TurnRunOptions,
    upgrade_quiescing: Arc<AtomicBool>,
    turn_quiesced: Arc<AtomicBool>,
    prepare: Prepare,
    execute: Execute,
    mut on_unwind: Unwind,
) -> Result<String, ProviderSessionError>
where
    Prepare: FnOnce(&str, &TurnRunStarted) -> Result<Prepared, ProviderSessionError>,
    Execute: FnOnce(
        ProductionTurnRunShellPreparedTurn<Prepared>,
    ) -> Result<String, ProviderSessionError>,
    Unwind: FnMut(&TurnRunStarted),
{
    let prompt = latest_non_empty_user_prompt(messages).ok_or_else(|| {
        ProviderSessionError::Configuration(
            "Runner production turn requires a non-empty user prompt.".into(),
        )
    })?;
    let started = shell
        .begin_run(prompt, options)
        .map_err(turn_shell_error)?;
    shell
        .mark_dispatched(&started.owner)
        .map_err(turn_shell_error)?;

    let result = match prepare(prompt.trim(), &started) {
        Ok(prepared) => execute(ProductionTurnRunShellPreparedTurn {
            started: started.clone(),
            prompt: prompt.trim().to_string(),
            prepared,
        }),
        Err(error) => Err(error),
    };

    if upgrade_quiescing.load(Ordering::Acquire)
        || turn_quiesced.load(Ordering::Acquire)
    {
        if let Err(error) = shell.mark_quiesced_for_upgrade(&started.owner) {
            on_unwind(&started);
            return Err(turn_shell_error(error));
        }
    }

    let settlement = match settle_production_turn(shell, &started, &result) {
        Ok(settlement) => settlement,
        Err(error) => {
            on_unwind(&started);
            return Err(turn_shell_error(error));
        }
    };
    *last_finished = Some(settlement);
    on_unwind(&started);
    result
}

fn settle_production_turn(
    shell: &mut TurnRunShell,
    started: &TurnRunStarted,
    result: &Result<String, ProviderSessionError>,
) -> Result<TurnRunFinished, TurnRunShellError> {
    match result {
        Ok(_) => shell.finish_completed(&started.owner),
        Err(ProviderSessionError::Cancelled(reason))
            if reason.starts_with(WAITING_USER_CANCELLATION_PREFIX) =>
        {
            shell
                .end_turn_awaiting_user(&started.owner, reason.clone())
                .and_then(|()| shell.finish_cancelled(&started.owner))
        }
        Err(ProviderSessionError::Cancelled(_)) => {
            shell.finish_cancelled(&started.owner)
        }
        Err(error) => shell.finish_failed(
            &started.owner,
            matches!(error, ProviderSessionError::Transport(_)),
            error.to_string(),
        ),
    }
}

fn latest_non_empty_user_prompt(messages: &[ProviderMessage]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role == "user" && !message.content.trim().is_empty()
        })
        .map(|message| message.content.as_str())
}

fn turn_shell_error(error: TurnRunShellError) -> ProviderSessionError {
    ProviderSessionError::Protocol(format!(
        "Runner turn lifecycle failed: {error}"
    ))
}

pub trait RoutedProviderCheckpointStore: Send + Sync {
    /// Persist the checkpoint before it becomes eligible for resume.
    ///
    /// The returned cursor must identify the durable checkpoint payload.
    fn persist(
        &self,
        checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError>;
}

pub trait RoutedProviderAttemptExecutor {
    fn run_attempt(
        &mut self,
        resume_from: Option<&RoutedProviderCheckpoint>,
        on_text_delta: &mut dyn FnMut(&str, &str),
        on_checkpoint: &mut dyn FnMut(
            &RoutedProviderCheckpoint,
        ) -> Result<(), ProviderSessionError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<String, ProviderSessionError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryEvent {
    pub attempt: u32,
    pub next_attempt: u32,
    pub delay_ms: u64,
    pub resume_from_checkpoint: bool,
    pub watchdog_expired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRetryOutcome {
    Retried,
    Exhausted,
    GaveUpIneligible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryReport {
    pub outcome: ProviderRetryOutcome,
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: Option<u64>,
    pub server_paced: bool,
    pub resume_from_checkpoint: bool,
    pub watchdog_expired: bool,
    pub error_type: String,
    pub error_code: String,
    pub cause: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ProductionTurnRunShellAdapter {
    pub policy: StreamAttemptPolicy,
    pub watchdog_poll_interval: Duration,
}

impl Default for ProductionTurnRunShellAdapter {
    fn default() -> Self {
        Self {
            policy: StreamAttemptPolicy::default(),
            watchdog_poll_interval: DEFAULT_WATCHDOG_POLL_INTERVAL,
        }
    }
}

impl ProductionTurnRunShellAdapter {
    pub fn run(
        &self,
        cancellation: &RoutedProviderCancellation,
        checkpoint_store: &dyn RoutedProviderCheckpointStore,
        executor: &mut dyn RoutedProviderAttemptExecutor,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.run_with_retry(
            cancellation,
            checkpoint_store,
            executor,
            on_text_delta,
            &mut |_| {},
        )
    }

    pub fn run_with_retry(
        &self,
        cancellation: &RoutedProviderCancellation,
        checkpoint_store: &dyn RoutedProviderCheckpointStore,
        executor: &mut dyn RoutedProviderAttemptExecutor,
        on_text_delta: &mut dyn FnMut(&str, &str),
        on_retry: &mut dyn FnMut(&ProviderRetryEvent),
    ) -> Result<String, ProviderSessionError> {
        self.run_with_retry_reporting(
            cancellation,
            checkpoint_store,
            executor,
            on_text_delta,
            on_retry,
            &mut |_| {},
        )
    }

    pub fn run_with_retry_reporting(
        &self,
        cancellation: &RoutedProviderCancellation,
        checkpoint_store: &dyn RoutedProviderCheckpointStore,
        executor: &mut dyn RoutedProviderAttemptExecutor,
        on_text_delta: &mut dyn FnMut(&str, &str),
        on_retry: &mut dyn FnMut(&ProviderRetryEvent),
        on_report: &mut dyn FnMut(&ProviderRetryReport),
    ) -> Result<String, ProviderSessionError> {
        self.run_with_attempt_runtime_reporting(
            Arc::new(StreamAttemptRuntime::new(self.policy.clone())),
            cancellation,
            checkpoint_store,
            executor,
            on_text_delta,
            on_retry,
            on_report,
        )
    }

    pub fn run_with_attempt_runtime_reporting(
        &self,
        runtime: Arc<StreamAttemptRuntime>,
        cancellation: &RoutedProviderCancellation,
        checkpoint_store: &dyn RoutedProviderCheckpointStore,
        executor: &mut dyn RoutedProviderAttemptExecutor,
        on_text_delta: &mut dyn FnMut(&str, &str),
        on_retry: &mut dyn FnMut(&ProviderRetryEvent),
        on_report: &mut dyn FnMut(&ProviderRetryReport),
    ) -> Result<String, ProviderSessionError> {
        let mut attempt = 1_u32;
        let mut resume_from: Option<RoutedProviderCheckpoint> = None;

        loop {
            if cancellation.is_cancelled() {
                return Err(cancelled_error(cancellation));
            }

            let generation = runtime.begin_attempt();
            let attempt_done = Arc::new(AtomicBool::new(false));
            let watchdog_expired = Arc::new(AtomicBool::new(false));
            let watchdog_timeout = first_output_timeout_for_attempt(
                runtime.policy().first_output_timeout,
                attempt,
            );
            let watchdog = spawn_first_output_watchdog(
                watchdog_timeout,
                self.watchdog_poll_interval,
                Arc::clone(&runtime),
                generation,
                Arc::clone(&attempt_done),
                Arc::clone(&watchdog_expired),
                cancellation.clone(),
            );

            let progress = RefCell::new(AttemptProgress::default());
            let accepted_resume =
                RefCell::new(resume_from.clone());
            let runtime_for_delta = Arc::clone(&runtime);
            let mut guarded_delta = |delta: &str, accumulated: &str| {
                if !runtime_for_delta.mark_stream_output(generation) {
                    return;
                }
                let mut progress = progress.borrow_mut();
                progress.record_output(delta.len());
                // Output after a prior checkpoint invalidates that checkpoint
                // for this attempt until a newer boundary is durably accepted.
                progress.checkpoint = None;
                drop(progress);
                on_text_delta(delta, accumulated);
            };
            let runtime_for_checkpoint = Arc::clone(&runtime);
            let mut accept_checkpoint =
                |checkpoint: &RoutedProviderCheckpoint| {
                    if !runtime_for_checkpoint.is_current(generation) {
                        return Ok(());
                    }
                    let cursor = checkpoint_store.persist(checkpoint)?;
                    // A durable tool-boundary checkpoint is observable provider
                    // progress even when no text delta preceded it. It also
                    // begins a fresh provider continuation, so persist first,
                    // record the resumable boundary, then re-arm the watchdog
                    // for the first output after that tool result. This keeps a
                    // post-tool SSE stall bounded without making an unpersisted
                    // checkpoint eligible for resume.
                    runtime_for_checkpoint.mark_stream_output(generation);
                    runtime_for_checkpoint
                        .rearm_output_deadline_after_checkpoint(generation);
                    let mut progress = progress.borrow_mut();
                    progress.record_output(0);
                    progress.checkpoint = Some(AttemptCheckpoint::new(
                        cursor,
                        checkpoint.emitted_text_bytes(),
                        checkpoint.tool_calls_completed(),
                    ));
                    drop(progress);
                    *accepted_resume.borrow_mut() =
                        Some(checkpoint.clone());
                    Ok(())
                };

            let timed_out_for_attempt = Arc::clone(&watchdog_expired);
            let runtime_for_cancel = Arc::clone(&runtime);
            let should_cancel = || {
                cancellation.is_cancelled()
                    || timed_out_for_attempt.load(Ordering::Acquire)
                    || !runtime_for_cancel.is_current(generation)
            };
            let result = executor.run_attempt(
                resume_from.as_ref(),
                &mut guarded_delta,
                &mut accept_checkpoint,
                &should_cancel,
            );

            attempt_done.store(true, Ordering::Release);
            if let Some(watchdog) = watchdog {
                let _ = watchdog.join();
            }
            runtime.settle_attempt(generation);

            if cancellation.is_cancelled() {
                return Err(cancelled_error(cancellation));
            }

            let timed_out =
                watchdog_expired.load(Ordering::Acquire);
            let progress = progress.into_inner();
            resume_from = accepted_resume.into_inner();

            match result {
                Ok(value) if !timed_out => return Ok(value),
                Ok(_) => {
                    let error = ProviderSessionError::Transport(
                        "Runner first-output watchdog timed out before provider output."
                            .into(),
                    );
                    if !self.retry(
                        runtime.policy(),
                        &mut attempt,
                        &progress,
                        &error,
                        true,
                        cancellation,
                        resume_from.as_ref(),
                        on_retry,
                        on_report,
                    )? {
                        return Err(error);
                    }
                }
                Err(error) => {
                    let error = if timed_out {
                        ProviderSessionError::Transport(
                            "Runner first-output watchdog timed out before provider output."
                                .into(),
                        )
                    } else {
                        error
                    };
                    if !self.retry(
                        runtime.policy(),
                        &mut attempt,
                        &progress,
                        &error,
                        timed_out,
                        cancellation,
                        resume_from.as_ref(),
                        on_retry,
                        on_report,
                    )? {
                        return Err(error);
                    }
                }
            }
        }
    }

    fn retry(
        &self,
        policy: &StreamAttemptPolicy,
        attempt: &mut u32,
        progress: &AttemptProgress,
        error: &ProviderSessionError,
        watchdog_expired: bool,
        cancellation: &RoutedProviderCancellation,
        accepted_resume: Option<&RoutedProviderCheckpoint>,
        on_retry: &mut dyn FnMut(&ProviderRetryEvent),
        on_report: &mut dyn FnMut(&ProviderRetryReport),
    ) -> Result<bool, ProviderSessionError> {
        let transient =
            classify_provider_failure(error, watchdog_expired);
        let decision = policy.retry_decision(*attempt, progress, &transient);
        let (delay, resume_from_checkpoint) = match decision {
            RetryDecision::RetryAfter(delay) => (delay, false),
            RetryDecision::ResumeAfter {
                delay,
                checkpoint: _,
            } => {
                if accepted_resume.is_none() {
                    return Ok(false);
                }
                (delay, true)
            }
            RetryDecision::Fail => {
                let outcome = if *attempt > 1 && *attempt >= policy.max_attempts {
                    Some(ProviderRetryOutcome::Exhausted)
                } else if *attempt > 1 || transient.retryable() {
                    Some(ProviderRetryOutcome::GaveUpIneligible)
                } else {
                    None
                };
                if let Some(outcome) = outcome {
                    on_report(&ProviderRetryReport {
                        outcome,
                        attempt: *attempt,
                        max_attempts: policy.max_attempts,
                        delay_ms: None,
                        server_paced: false,
                        resume_from_checkpoint: accepted_resume.is_some(),
                        watchdog_expired,
                        error_type: retry_error_type(error, watchdog_expired).into(),
                        error_code: retry_error_code(transient.kind).into(),
                        cause: error.to_string(),
                        error: error.to_string(),
                    });
                }
                return Ok(false);
            }
        };
        let next_attempt = attempt.saturating_add(1);
        let delay_ms = delay.as_millis().min(u64::MAX as u128) as u64;
        let server_paced = transient.retry_after_ms.is_some();
        on_report(&ProviderRetryReport {
            outcome: ProviderRetryOutcome::Retried,
            attempt: *attempt,
            max_attempts: policy.max_attempts,
            delay_ms: Some(delay_ms),
            server_paced,
            resume_from_checkpoint,
            watchdog_expired,
            error_type: retry_error_type(error, watchdog_expired).into(),
            error_code: retry_error_code(transient.kind).into(),
            cause: error.to_string(),
            error: error.to_string(),
        });
        on_retry(&ProviderRetryEvent {
            attempt: *attempt,
            next_attempt,
            delay_ms,
            resume_from_checkpoint,
            watchdog_expired,
        });
        sleep_with_cancellation(delay, cancellation)?;
        *attempt = next_attempt;
        Ok(true)
    }
}

pub fn first_output_timeout_for_attempt(
    base: Duration,
    attempt: u32,
) -> Duration {
    base.saturating_mul(1_u32 << attempt.saturating_sub(1).min(8))
}

fn cancelled_error(
    cancellation: &RoutedProviderCancellation,
) -> ProviderSessionError {
    ProviderSessionError::Cancelled(
        cancellation
            .reason()
            .unwrap_or_else(|| "Runner provider request cancelled".into()),
    )
}

fn spawn_first_output_watchdog(
    timeout: Duration,
    poll_interval: Duration,
    runtime: Arc<StreamAttemptRuntime>,
    generation: StreamAttemptGeneration,
    attempt_done: Arc<AtomicBool>,
    expired: Arc<AtomicBool>,
    cancellation: RoutedProviderCancellation,
) -> Option<thread::JoinHandle<()>> {
    if timeout.is_zero() {
        expired.store(true, Ordering::Release);
        runtime.settle_attempt(generation);
        return None;
    }
    Some(thread::spawn(move || {
        let mut epoch = runtime.deadline_epoch(generation).unwrap_or_default();
        let mut started = Instant::now();
        while !attempt_done.load(Ordering::Acquire)
            && runtime.is_current(generation)
            && !cancellation.is_cancelled()
        {
            if !runtime.deadline_armed(generation) {
                thread::sleep(if poll_interval.is_zero() {
                    Duration::from_millis(1)
                } else {
                    poll_interval
                });
                continue;
            }
            let current_epoch = match runtime.deadline_epoch(generation) {
                Some(current_epoch) => current_epoch,
                None => break,
            };
            if current_epoch != epoch {
                epoch = current_epoch;
                started = Instant::now();
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                expired.store(true, Ordering::Release);
                runtime.settle_attempt(generation);
                break;
            }
            let remaining = timeout.saturating_sub(elapsed);
            let sleep_for = if poll_interval.is_zero() {
                remaining.min(Duration::from_millis(1))
            } else {
                remaining.min(poll_interval)
            };
            thread::sleep(sleep_for);
        }
    }))
}

fn sleep_with_cancellation(
    delay: Duration,
    cancellation: &RoutedProviderCancellation,
) -> Result<(), ProviderSessionError> {
    let started = Instant::now();
    while started.elapsed() < delay {
        if cancellation.is_cancelled() {
            return Err(cancelled_error(cancellation));
        }
        let remaining = delay.saturating_sub(started.elapsed());
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    Ok(())
}

fn retry_error_type(
    error: &ProviderSessionError,
    watchdog_expired: bool,
) -> &'static str {
    if watchdog_expired {
        return "FirstTokenStallError";
    }
    match error {
        ProviderSessionError::Cancelled(_) => "CancelledError",
        ProviderSessionError::Authentication(_) => "AuthenticationError",
        ProviderSessionError::Configuration(_) => "ConfigurationError",
        ProviderSessionError::Protocol(_) => "ProtocolError",
        ProviderSessionError::Tool(_) => "ToolError",
        ProviderSessionError::ProviderRefusal(_) => "ProviderRefusalError",
        ProviderSessionError::Transport(_) => "TransportError",
    }
}

fn retry_error_code(kind: StreamFailureKind) -> &'static str {
    match kind {
        StreamFailureKind::Capacity | StreamFailureKind::RateLimit => "SAND-E0401",
        StreamFailureKind::Timeout
        | StreamFailureKind::Transport
        | StreamFailureKind::Server => "SAND-E0406",
        StreamFailureKind::Authentication
        | StreamFailureKind::InvalidRequest
        | StreamFailureKind::Protocol => "SAND-E0405",
        StreamFailureKind::Cancelled | StreamFailureKind::Unknown => "SAND-E0407",
    }
}

fn classify_provider_failure(
    error: &ProviderSessionError,
    watchdog_expired: bool,
) -> TransientStreamError {
    if watchdog_expired {
        return TransientStreamError {
            kind: StreamFailureKind::Timeout,
            message: error.to_string(),
            retry_after_ms: None,
        };
    }

    let message = error.to_string();
    match error {
        ProviderSessionError::Cancelled(_) => TransientStreamError {
            kind: StreamFailureKind::Cancelled,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Authentication(_) => TransientStreamError {
            kind: StreamFailureKind::Authentication,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Configuration(_) => TransientStreamError {
            kind: StreamFailureKind::InvalidRequest,
            message,
            retry_after_ms: None,
        },
        ProviderSessionError::Protocol(_)
        | ProviderSessionError::Tool(_)
        | ProviderSessionError::ProviderRefusal(_) => {
            TransientStreamError {
                kind: StreamFailureKind::Protocol,
                message,
                retry_after_ms: None,
            }
        }
        ProviderSessionError::Transport(_) => {
            let status = [408_u16, 429, 500, 502, 503, 504]
                .into_iter()
                .find(|status| {
                    message.contains(&format!("({status}"))
                        || message.contains(&format!(" {status} "))
                });
            TransientStreamError::classify(message, status, None)
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::inference::codex_direct_responses::{
        CodexDirectCheckpoint, CodexDirectUsage,
    };
    use serde_json::json;

    struct MemoryCheckpointStore;

    impl RoutedProviderCheckpointStore for MemoryCheckpointStore {
        fn persist(
            &self,
            _checkpoint: &RoutedProviderCheckpoint,
        ) -> Result<String, ProviderSessionError> {
            Ok("checkpoint-1".into())
        }
    }

    struct PostToolStallExecutor {
        attempts: u32,
    }

    impl RoutedProviderAttemptExecutor for PostToolStallExecutor {
        fn run_attempt(
            &mut self,
            resume_from: Option<&RoutedProviderCheckpoint>,
            on_text_delta: &mut dyn FnMut(&str, &str),
            on_checkpoint: &mut dyn FnMut(
                &RoutedProviderCheckpoint,
            ) -> Result<(), ProviderSessionError>,
            should_cancel: &dyn Fn() -> bool,
        ) -> Result<String, ProviderSessionError> {
            self.attempts = self.attempts.saturating_add(1);
            if self.attempts == 1 {
                assert!(resume_from.is_none());
                on_text_delta("working", "working");
                on_checkpoint(&RoutedProviderCheckpoint::Fabushi(
                    CodexDirectCheckpoint {
                        input: vec![json!({"role":"user","content":"inspect"})],
                        text: "working".into(),
                        response_id: "response-1".into(),
                        usage: CodexDirectUsage::default(),
                        completed_steps: 1,
                        tool_calls_completed: 1,
                    },
                ))?;
                let started = Instant::now();
                while !should_cancel() && started.elapsed() < Duration::from_secs(1) {
                    thread::sleep(Duration::from_millis(1));
                }
                assert!(should_cancel(), "post-tool continuation must be watchdog-bounded");
                return Err(ProviderSessionError::Cancelled(
                    "provider continuation watchdog".into(),
                ));
            }

            assert!(resume_from.is_some(), "retry must resume the durable tool checkpoint");
            on_text_delta("done", "workingdone");
            Ok("terminal".into())
        }
    }

    #[test]
    fn post_tool_provider_stall_resumes_from_durable_checkpoint() {
        let adapter = ProductionTurnRunShellAdapter {
            policy: StreamAttemptPolicy {
                first_output_timeout: Duration::from_millis(20),
                max_attempts: 2,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(1),
                max_retry_after: Duration::from_millis(1),
            },
            watchdog_poll_interval: Duration::from_millis(1),
        };
        let cancellation = RoutedProviderCancellation::default();
        let mut executor = PostToolStallExecutor { attempts: 0 };
        let mut retries = Vec::new();

        let result = adapter
            .run_with_retry(
                &cancellation,
                &MemoryCheckpointStore,
                &mut executor,
                &mut |_, _| {},
                &mut |event| retries.push(event.clone()),
            )
            .expect("watchdog retry should resume and complete");

        assert_eq!(result, "terminal");
        assert_eq!(executor.attempts, 2);
        assert_eq!(retries.len(), 1);
        assert!(retries[0].watchdog_expired);
        assert!(retries[0].resume_from_checkpoint);
    }
}
