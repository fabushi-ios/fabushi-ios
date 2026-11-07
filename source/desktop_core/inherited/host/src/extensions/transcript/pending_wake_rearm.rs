use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use serde_json::Value;

use super::sand_pending_wake_store::{
    DurablePendingWakeMarker, PendingWakeKind, QuietWakeOrigin, SandPendingWakeStore,
    coerce_marker,
};

pub const PENDING_WAKE_STALE_MAX_AGE_MS: f64 = 48.0 * 60.0 * 60.0 * 1_000.0;

pub fn is_recreate_wake_carry_disabled() -> bool {
    env::var("SAND_DISABLE_RECREATE_WAKE_CARRY").as_deref() == Ok("1")
}
#[derive(Debug, Clone, PartialEq)]
pub struct PendingWakeReport {
    pub conversation_id: String,
    pub outcome: String,
    pub kind: PendingWakeKind,
    pub work_id: String,
    pub age_ms: Option<f64>,
    pub reason: Option<String>,
    pub is_quiet_origin: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LostSubagentWake {
    pub parent_agent_id: String,
    pub subagent_agent_id: String,
    pub subagent_type: String,
    pub title: String,
    pub result: String,
    pub quiet_origin: Option<QuietWakeOrigin>,
    pub pending_wake_marker: DurablePendingWakeMarker,
}

pub trait PendingWakeRuntimePort: Send + Sync {
    fn can_execute(&self) -> bool;
    fn is_agent_gone(&self, agent_id: &str) -> bool;
    fn is_group_session(&self, agent_id: &str) -> Result<bool, String>;

    fn cloud_watch_is_armed(&self, agent_id: &str, work_id: &str) -> bool;
    fn watch_cloud_agent(
        &self,
        agent_id: &str,
        work_id: &str,
        quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String>;

    fn watch_background_shell(
        &self,
        agent_id: &str,
        work_id: &str,
        title: Option<&str>,
        quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String>;

    fn deliver_recreate_interrupted_shell_notice(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String>;

    fn revive_lost_subagent(&self, wake: LostSubagentWake) -> Result<(), String>;

    fn revive_channel_delivery_failure(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String>;

    fn emit_async_tasks_for_agent(&self, agent_id: &str);
    fn report_pending_wake(&self, report: PendingWakeReport);
}

#[derive(Clone)]
pub struct PendingWakeRearm {
    store: Option<SandPendingWakeStore>,
    runtime: Arc<dyn PendingWakeRuntimePort>,
    now_ms: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl PendingWakeRearm {
    pub fn new(
        store: Option<SandPendingWakeStore>,
        runtime: Arc<dyn PendingWakeRuntimePort>,
    ) -> Self {
        Self {
            store,
            runtime,
            now_ms: Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
        }
    }

    pub fn with_now(mut self, now_ms: Arc<dyn Fn() -> f64 + Send + Sync>) -> Self {
        self.now_ms = now_ms;
        self
    }

    pub fn persist_pending_wake(&self, marker: DurablePendingWakeMarker) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        if self.runtime.is_agent_gone(&marker.agent_id) {
            return false;
        }
        let written = store.mark_pending(marker.clone());
        self.report(
            &marker,
            if written { "persisted" } else { "persist_failed" },
            None,
            None,
        );
        written
    }

    pub fn clear_settled_pending_wake(
        &self,
        agent_id: &str,
        kind: PendingWakeKind,
        work_id: &str,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(marker) = store.list_pending().into_iter().find(|entry| {
            entry.agent_id == agent_id && entry.kind == kind && entry.work_id == work_id
        }) else {
            return;
        };
        if !store.clear_if_matches(&marker) {
            return;
        }
        self.report(&marker, "settled", None, None);
        self.runtime.emit_async_tasks_for_agent(agent_id);
    }

    pub fn disarm_pending_wake(
        &self,
        agent_id: &str,
        kind: PendingWakeKind,
        work_id: &str,
    ) {
        let Some(store) = &self.store else {
            return;
        };
        let Some(marker) = store.list_pending().into_iter().find(|entry| {
            entry.agent_id == agent_id && entry.kind == kind && entry.work_id == work_id
        }) else {
            return;
        };
        if !store.clear_if_matches(&marker) {
            return;
        }
        self.report(&marker, "settled", Some("aborted"), None);
        self.runtime.emit_async_tasks_for_agent(agent_id);
    }

    pub fn restore_recreate_carried_pending_wakes(&self, carried: &[Value]) -> usize {
        if carried.is_empty() || is_recreate_wake_carry_disabled() || !self.runtime.can_execute() {
            return 0;
        }
        let Some(store) = &self.store else {
            return 0;
        };
        let now_ms = (self.now_ms)();
        let mut restored = 0usize;
        for mut marker in carried.iter().filter_map(coerce_marker) {
            if !matches!(marker.kind, PendingWakeKind::CloudAgent | PendingWakeKind::Shell) {
                continue;
            }
            let age_ms = Some(now_ms - marker.marked_at_ms);
            if store.has_pending(&marker.agent_id, marker.kind, &marker.work_id) {
                self.report(&marker, "rearm_skipped", Some("locally_owned"), age_ms);
                continue;
            }
            if self.runtime.is_agent_gone(&marker.agent_id) {
                self.report(&marker, "rearm_skipped", Some("agent_gone"), age_ms);
                continue;
            }
            match self.runtime.is_group_session(&marker.agent_id) {
                Ok(true) => {
                    self.report(&marker, "rearm_skipped", Some("group_session"), age_ms);
                    continue;
                }
                Err(_) => {
                    self.report(&marker, "rearm_failed", Some("session_unavailable"), age_ms);
                    continue;
                }
                Ok(false) => {}
            }
            if marker.kind == PendingWakeKind::Shell {
                marker.interrupted_by_recreate = true;
            }
            if !store.mark_if_absent_or_matches(&marker) {
                self.report(&marker, "persist_failed", None, age_ms);
                continue;
            }
            self.report(&marker, "carried", None, age_ms);
            restored = restored.saturating_add(1);
            match marker.kind {
                PendingWakeKind::CloudAgent => {
                    self.rearm_pending_wake(marker, now_ms, Some("recreate_carry"));
                }
                PendingWakeKind::Shell => {
                    self.report(&marker, "dropped_with_notice", None, age_ms);
                    if self
                        .runtime
                        .deliver_recreate_interrupted_shell_notice(&marker)
                        .is_err()
                    {
                        self.report(&marker, "rearm_failed", Some("error"), age_ms);
                    }
                }
                PendingWakeKind::Subagent => unreachable!("filtered above"),
                PendingWakeKind::ChannelDelivery => unreachable!("filtered above"),
            }
        }
        restored
    }

    pub fn rearm_pending_wakes(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if !self.runtime.can_execute() {
            return;
        }
        let now = (self.now_ms)();
        for marker in store.prune_stale(PENDING_WAKE_STALE_MAX_AGE_MS, now) {
            self.report(
                &marker,
                "pruned",
                Some("stale"),
                Some(now - marker.marked_at_ms),
            );
        }

        for marker in store.list_pending() {
            if self.runtime.is_agent_gone(&marker.agent_id) {
                self.report(
                    &marker,
                    "rearm_skipped",
                    Some("agent_gone"),
                    Some(now - marker.marked_at_ms),
                );
                continue;
            }

            self.rearm_pending_wake(marker, now, None);
        }
    }

    pub fn rearm_pending_wake(
        &self,
        marker: DurablePendingWakeMarker,
        now_ms: f64,
        success_reason: Option<&str>,
    ) {
        let report = |service: &Self, outcome: &str, reason: Option<&str>| {
            if outcome == "rearm_failed" {
                if let Some(store) = &service.store {
                    store.mark_if_absent_or_matches(&marker);
                }
            }
            let effective_reason = reason.or_else(|| {
                (outcome == "rearmed")
                    .then_some(success_reason)
                    .flatten()
            });
            service.report(
                &marker,
                outcome,
                effective_reason,
                Some(now_ms - marker.marked_at_ms),
            );
        };

        let group = match self.runtime.is_group_session(&marker.agent_id) {
            Ok(value) => value,
            Err(_) => {
                report(self, "rearm_failed", Some("session_unavailable"));
                return;
            }
        };

        if group {
            if marker.kind == PendingWakeKind::Shell && marker.interrupted_by_recreate {
                if let Some(store) = &self.store {
                    store.clear_if_matches(&marker);
                }
            } else if let Some(store) = &self.store {
                store.clear_if_matches(&marker);
            }
            report(self, "rearm_skipped", Some("group_session"));
            return;
        }

        let result = match marker.kind {
            PendingWakeKind::CloudAgent => self.rearm_cloud_agent_wake(&marker),
            PendingWakeKind::Shell if marker.interrupted_by_recreate => self
                .runtime
                .deliver_recreate_interrupted_shell_notice(&marker)
                .map(|_| ()),
            PendingWakeKind::Shell => self.runtime.watch_background_shell(
                &marker.agent_id,
                &marker.work_id,
                marker.title.as_deref(),
                marker.quiet_origin.as_ref(),
            ),
            PendingWakeKind::Subagent => self.revive_parent_for_lost_subagent_wake(&marker),
            PendingWakeKind::ChannelDelivery => {
                self.runtime.revive_channel_delivery_failure(&marker)
            }
        };

        match result {
            Ok(()) => {
                let clear_after_success = !matches!(
                    marker.kind,
                    PendingWakeKind::CloudAgent | PendingWakeKind::Subagent
                )
                    && !(marker.kind == PendingWakeKind::Shell
                        && marker.interrupted_by_recreate);
                if clear_after_success {
                    if let Some(store) = &self.store {
                        store.clear_if_matches(&marker);
                    }
                }
                report(
                    self,
                    "rearmed",
                    (marker.kind == PendingWakeKind::Subagent)
                        .then_some("interrupted_completion"),
                );
            }
            Err(_) => report(self, "rearm_failed", Some("error")),
        }
    }

    fn rearm_cloud_agent_wake(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        if self
            .runtime
            .cloud_watch_is_armed(&marker.agent_id, &marker.work_id)
        {
            if let Some(store) = &self.store {
                if !store.mark_if_absent_or_matches(marker) {
                    return Err("pending cloud wake was replaced during rearm".into());
                }
            }
            return Ok(());
        }

        self.runtime.watch_cloud_agent(
            &marker.agent_id,
            &marker.work_id,
            marker.quiet_origin.as_ref(),
        )?;
        if self
            .runtime
            .cloud_watch_is_armed(&marker.agent_id, &marker.work_id)
        {
            Ok(())
        } else {
            Err("watch_not_armed".into())
        }
    }

    fn revive_parent_for_lost_subagent_wake(
        &self,
        marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        if let Some(store) = &self.store {
            store.mark_if_absent_or_matches(marker);
        }
        self.runtime.revive_lost_subagent(LostSubagentWake {
            parent_agent_id: marker.agent_id.clone(),
            subagent_agent_id: marker.work_id.clone(),
            subagent_type: marker
                .subagent_type
                .clone()
                .unwrap_or_else(|| "task".into()),
            title: marker
                .title
                .clone()
                .unwrap_or_else(|| "Background task".into()),
            result: "A host restart interrupted this background task before its result could be delivered; its in-process run did not survive, so its final state is unknown. Check its transcript (Await with this task id) if you need what it got through, and dispatch a fresh background task if the work still matters.".into(),
            quiet_origin: marker.quiet_origin.clone(),
            pending_wake_marker: marker.clone(),
        })
    }

    fn report(
        &self,
        marker: &DurablePendingWakeMarker,
        outcome: &str,
        reason: Option<&str>,
        age_ms: Option<f64>,
    ) {
        self.runtime.report_pending_wake(PendingWakeReport {
            conversation_id: marker.agent_id.clone(),
            outcome: outcome.into(),
            kind: marker.kind,
            work_id: marker.work_id.clone(),
            age_ms,
            reason: reason.map(str::to_string),
            is_quiet_origin: marker.quiet_origin.is_some(),
        });
    }

    pub fn enqueue_pending_wake<T>(
        &self,
        queue: &mut HashMap<String, Vec<T>>,
        agent_id: &str,
        items: Vec<T>,
    ) -> bool {
        if self.runtime.is_agent_gone(agent_id) {
            return false;
        }
        queue.entry(agent_id.to_string()).or_default().extend(items);
        true
    }
}
