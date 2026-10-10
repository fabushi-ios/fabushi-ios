//! Provider-neutral Mahayana runtime telemetry.
//!
//! Counters deliberately contain no prompts, secrets, model payloads, tool
//! arguments, file contents, or vendor identifiers.  Product surfaces can
//! sample this structure without crossing trust boundaries or collecting user
//! content by default.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPersistenceOperation {
    Checkpoint,
    Replay,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeMetricsSnapshot {
    pub sessions_opened: u64,
    pub operations_started: u64,
    pub operations_completed: u64,
    pub operations_failed: u64,
    pub operations_suspended: u64,
    pub operations_resumed: u64,
    pub tool_calls_started: u64,
    pub tool_calls_completed: u64,
    pub tool_calls_failed: u64,
    pub approvals_requested: u64,
    pub approvals_approved: u64,
    pub approvals_rejected: u64,
    pub approvals_timed_out: u64,
    pub approvals_interrupted: u64,
    pub model_calls: u64,
    pub model_failures: u64,
    pub model_latency_millis_total: u64,
    pub closing_send_nudges: u64,
    pub turn_empty_deliveries: u64,
    pub empty_delivery_reply_nudge_attempts_total: u64,
    pub empty_delivery_tool_calls_total: u64,
    pub empty_delivery_stream_output_turns: u64,
    pub empty_delivery_duration_millis_total: u64,
    pub session_checkpoints_succeeded: u64,
    pub session_checkpoints_failed: u64,
    pub session_checkpoint_bytes_total: u64,
    pub session_checkpoint_duration_millis_total: u64,
    pub session_replays_succeeded: u64,
    pub session_replays_failed: u64,
    pub session_replay_bytes_total: u64,
    pub session_replay_duration_millis_total: u64,
}

impl RuntimeMetricsSnapshot {
    pub fn operation_success_ratio(&self) -> f64 {
        let terminal = self.operations_completed + self.operations_failed;
        if terminal == 0 {
            1.0
        } else {
            self.operations_completed as f64 / terminal as f64
        }
    }

    pub fn tool_success_ratio(&self) -> f64 {
        let terminal = self.tool_calls_completed + self.tool_calls_failed;
        if terminal == 0 {
            1.0
        } else {
            self.tool_calls_completed as f64 / terminal as f64
        }
    }

    pub fn average_model_latency_millis(&self) -> Option<f64> {
        (self.model_calls > 0)
            .then(|| self.model_latency_millis_total as f64 / self.model_calls as f64)
    }
}

#[derive(Debug, Default)]
pub struct RuntimeTelemetry {
    sessions_opened: AtomicU64,
    operations_started: AtomicU64,
    operations_completed: AtomicU64,
    operations_failed: AtomicU64,
    operations_suspended: AtomicU64,
    operations_resumed: AtomicU64,
    tool_calls_started: AtomicU64,
    tool_calls_completed: AtomicU64,
    tool_calls_failed: AtomicU64,
    approvals_requested: AtomicU64,
    approvals_approved: AtomicU64,
    approvals_rejected: AtomicU64,
    approvals_timed_out: AtomicU64,
    approvals_interrupted: AtomicU64,
    model_calls: AtomicU64,
    model_failures: AtomicU64,
    model_latency_millis_total: AtomicU64,
    closing_send_nudges: AtomicU64,
    turn_empty_deliveries: AtomicU64,
    empty_delivery_reply_nudge_attempts_total: AtomicU64,
    empty_delivery_tool_calls_total: AtomicU64,
    empty_delivery_stream_output_turns: AtomicU64,
    empty_delivery_duration_millis_total: AtomicU64,
    session_checkpoints_succeeded: AtomicU64,
    session_checkpoints_failed: AtomicU64,
    session_checkpoint_bytes_total: AtomicU64,
    session_checkpoint_duration_millis_total: AtomicU64,
    session_replays_succeeded: AtomicU64,
    session_replays_failed: AtomicU64,
    session_replay_bytes_total: AtomicU64,
    session_replay_duration_millis_total: AtomicU64,
}

impl RuntimeTelemetry {
    pub fn session_opened(&self) {
        self.sessions_opened.fetch_add(1, Ordering::Relaxed);
    }

    pub fn operation_started(&self) {
        self.operations_started.fetch_add(1, Ordering::Relaxed);
    }

    pub fn operation_completed(&self) {
        self.operations_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn operation_failed(&self) {
        self.operations_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn operation_suspended(&self) {
        self.operations_suspended.fetch_add(1, Ordering::Relaxed);
    }

    pub fn operation_resumed(&self) {
        self.operations_resumed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn tool_started(&self) {
        self.tool_calls_started.fetch_add(1, Ordering::Relaxed);
    }

    pub fn tool_completed(&self, success: bool) {
        if success {
            self.tool_calls_completed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.tool_calls_failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn approval_requested(&self) {
        self.approvals_requested.fetch_add(1, Ordering::Relaxed);
    }

    pub fn approval_approved(&self) {
        self.approvals_approved.fetch_add(1, Ordering::Relaxed);
    }

    pub fn approval_rejected(&self) {
        self.approvals_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn approval_timed_out(&self) {
        self.approvals_timed_out.fetch_add(1, Ordering::Relaxed);
    }

    pub fn approval_interrupted(&self) {
        self.approvals_interrupted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn model_finished(&self, latency: Duration, success: bool) {
        self.model_calls.fetch_add(1, Ordering::Relaxed);
        self.model_latency_millis_total.fetch_add(
            latency.as_millis().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
        if !success {
            self.model_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn closing_send_nudge(&self) {
        self.closing_send_nudges.fetch_add(1, Ordering::Relaxed);
    }

    pub fn turn_empty_delivery(
        &self,
        reply_nudge_attempts: u64,
        tool_call_count: u64,
        stream_output_produced: bool,
        duration: Duration,
    ) {
        self.turn_empty_deliveries.fetch_add(1, Ordering::Relaxed);
        self.empty_delivery_reply_nudge_attempts_total
            .fetch_add(reply_nudge_attempts, Ordering::Relaxed);
        self.empty_delivery_tool_calls_total
            .fetch_add(tool_call_count, Ordering::Relaxed);
        if stream_output_produced {
            self.empty_delivery_stream_output_turns
                .fetch_add(1, Ordering::Relaxed);
        }
        self.empty_delivery_duration_millis_total.fetch_add(
            duration.as_millis().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn session_persistence_finished(
        &self,
        operation: SessionPersistenceOperation,
        bytes: u64,
        duration: Duration,
        success: bool,
    ) {
        let duration_millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        match operation {
            SessionPersistenceOperation::Checkpoint => {
                if success {
                    self.session_checkpoints_succeeded
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    self.session_checkpoints_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.session_checkpoint_bytes_total
                    .fetch_add(bytes, Ordering::Relaxed);
                self.session_checkpoint_duration_millis_total
                    .fetch_add(duration_millis, Ordering::Relaxed);
            }
            SessionPersistenceOperation::Replay => {
                if success {
                    self.session_replays_succeeded
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    self.session_replays_failed
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.session_replay_bytes_total
                    .fetch_add(bytes, Ordering::Relaxed);
                self.session_replay_duration_millis_total
                    .fetch_add(duration_millis, Ordering::Relaxed);
            }
        }
    }

    pub fn snapshot(&self) -> RuntimeMetricsSnapshot {
        RuntimeMetricsSnapshot {
            sessions_opened: self.sessions_opened.load(Ordering::Relaxed),
            operations_started: self.operations_started.load(Ordering::Relaxed),
            operations_completed: self.operations_completed.load(Ordering::Relaxed),
            operations_failed: self.operations_failed.load(Ordering::Relaxed),
            operations_suspended: self.operations_suspended.load(Ordering::Relaxed),
            operations_resumed: self.operations_resumed.load(Ordering::Relaxed),
            tool_calls_started: self.tool_calls_started.load(Ordering::Relaxed),
            tool_calls_completed: self.tool_calls_completed.load(Ordering::Relaxed),
            tool_calls_failed: self.tool_calls_failed.load(Ordering::Relaxed),
            approvals_requested: self.approvals_requested.load(Ordering::Relaxed),
            approvals_approved: self.approvals_approved.load(Ordering::Relaxed),
            approvals_rejected: self.approvals_rejected.load(Ordering::Relaxed),
            approvals_timed_out: self.approvals_timed_out.load(Ordering::Relaxed),
            approvals_interrupted: self.approvals_interrupted.load(Ordering::Relaxed),
            model_calls: self.model_calls.load(Ordering::Relaxed),
            model_failures: self.model_failures.load(Ordering::Relaxed),
            model_latency_millis_total: self.model_latency_millis_total.load(Ordering::Relaxed),
            closing_send_nudges: self.closing_send_nudges.load(Ordering::Relaxed),
            turn_empty_deliveries: self.turn_empty_deliveries.load(Ordering::Relaxed),
            empty_delivery_reply_nudge_attempts_total: self.empty_delivery_reply_nudge_attempts_total.load(Ordering::Relaxed),
            empty_delivery_tool_calls_total: self.empty_delivery_tool_calls_total.load(Ordering::Relaxed),
            empty_delivery_stream_output_turns: self.empty_delivery_stream_output_turns.load(Ordering::Relaxed),
            empty_delivery_duration_millis_total: self.empty_delivery_duration_millis_total.load(Ordering::Relaxed),
            session_checkpoints_succeeded: self.session_checkpoints_succeeded.load(Ordering::Relaxed),
            session_checkpoints_failed: self.session_checkpoints_failed.load(Ordering::Relaxed),
            session_checkpoint_bytes_total: self.session_checkpoint_bytes_total.load(Ordering::Relaxed),
            session_checkpoint_duration_millis_total: self.session_checkpoint_duration_millis_total.load(Ordering::Relaxed),
            session_replays_succeeded: self.session_replays_succeeded.load(Ordering::Relaxed),
            session_replays_failed: self.session_replays_failed.load(Ordering::Relaxed),
            session_replay_bytes_total: self.session_replay_bytes_total.load(Ordering::Relaxed),
            session_replay_duration_millis_total: self.session_replay_duration_millis_total.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_content_free_runtime_metrics() {
        let telemetry = RuntimeTelemetry::default();
        telemetry.session_opened();
        telemetry.operation_started();
        telemetry.approval_requested();
        telemetry.approval_rejected();
        telemetry.tool_started();
        telemetry.tool_completed(false);
        telemetry.model_finished(Duration::from_millis(40), true);
        telemetry.closing_send_nudge();
        telemetry.turn_empty_delivery(3, 4, true, Duration::from_millis(1250));
        telemetry.session_persistence_finished(
            SessionPersistenceOperation::Checkpoint,
            4096,
            Duration::from_millis(12),
            true,
        );
        telemetry.session_persistence_finished(
            SessionPersistenceOperation::Checkpoint,
            1024,
            Duration::from_millis(3),
            false,
        );
        telemetry.session_persistence_finished(
            SessionPersistenceOperation::Replay,
            2048,
            Duration::from_millis(7),
            true,
        );
        telemetry.session_persistence_finished(
            SessionPersistenceOperation::Replay,
            512,
            Duration::from_millis(2),
            false,
        );
        telemetry.operation_failed();

        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.sessions_opened, 1);
        assert_eq!(snapshot.operations_started, 1);
        assert_eq!(snapshot.approvals_requested, 1);
        assert_eq!(snapshot.approvals_rejected, 1);
        assert_eq!(snapshot.tool_calls_failed, 1);
        assert_eq!(snapshot.model_calls, 1);
        assert_eq!(snapshot.closing_send_nudges, 1);
        assert_eq!(snapshot.turn_empty_deliveries, 1);
        assert_eq!(snapshot.empty_delivery_reply_nudge_attempts_total, 3);
        assert_eq!(snapshot.empty_delivery_tool_calls_total, 4);
        assert_eq!(snapshot.empty_delivery_stream_output_turns, 1);
        assert_eq!(snapshot.empty_delivery_duration_millis_total, 1250);
        assert_eq!(snapshot.session_checkpoints_succeeded, 1);
        assert_eq!(snapshot.session_checkpoints_failed, 1);
        assert_eq!(snapshot.session_checkpoint_bytes_total, 5120);
        assert_eq!(snapshot.session_checkpoint_duration_millis_total, 15);
        assert_eq!(snapshot.session_replays_succeeded, 1);
        assert_eq!(snapshot.session_replays_failed, 1);
        assert_eq!(snapshot.session_replay_bytes_total, 2560);
        assert_eq!(snapshot.session_replay_duration_millis_total, 9);
        assert_eq!(snapshot.average_model_latency_millis(), Some(40.0));
        assert_eq!(snapshot.operation_success_ratio(), 0.0);
    }
}
