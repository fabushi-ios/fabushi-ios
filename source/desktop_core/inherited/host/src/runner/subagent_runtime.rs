use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::background_work::{derive_background_subagent_title, format_steer_prompt};
use super::TurnUsage;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentLineage {
    pub parent_request_id: Option<String>,
    pub root_parent_request_id: Option<String>,
    pub parent_agent_tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentDispatchMeta {
    pub tool_call_id: String,
    pub lineage: Option<SubagentLineage>,
}

pub fn compute_subagent_request_id(tool_call_id: &str) -> String {
    if tool_call_id.is_empty() {
        "subagent".to_string()
    } else {
        format!("subagent:{tool_call_id}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRunOptions {
    pub inference_request_id: String,
    pub lineage: Option<SubagentLineage>,
}

pub fn subagent_steer_run_options(meta: &SubagentDispatchMeta) -> SubagentRunOptions {
    let lineage = meta.lineage.as_ref().map(|lineage| {
        let mut lineage = lineage.clone();
        if !meta.tool_call_id.is_empty() {
            lineage.parent_agent_tool_call_id = Some(meta.tool_call_id.clone());
        }
        lineage
    });
    SubagentRunOptions {
        inference_request_id: compute_subagent_request_id(&meta.tool_call_id),
        lineage,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerUseUsageSnapshot {
    pub model_id: Option<String>,
    pub turn_ended_count: u64,
    pub usage: Option<TurnUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStatus {
    Pending,
    Running,
    Done,
    Error,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRecord {
    pub subagent_type: String,
    pub title: String,
    pub started_at_ms: u64,
    pub status: SubagentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRunRecord {
    pub run_id: String,
    pub child_session_key: String,
    pub controller_session_key: String,
    pub requester_session_key: String,
    pub subagent_type: String,
    pub title: String,
    pub created_at_ms: u64,
    pub status: String,
}

impl From<SubagentRecord> for SubagentRunRecord {
    fn from(rec: SubagentRecord) -> Self {
        Self {
            run_id: uuid::Uuid::new_v4().to_string(),
            child_session_key: String::new(),
            controller_session_key: String::new(),
            requester_session_key: String::new(),
            subagent_type: rec.subagent_type,
            title: rec.title,
            created_at_ms: rec.started_at_ms,
            status: match rec.status {
                SubagentStatus::Pending => "pending".into(),
                SubagentStatus::Running => "running".into(),
                SubagentStatus::Done => "done".into(),
                SubagentStatus::Error => "error".into(),
                SubagentStatus::Aborted => "aborted".into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundSubagentCompletion {
    pub parent_agent_id: String,
    pub subagent_agent_id: String,
    pub lifecycle_revision: String,
    pub marked_at_ms: u64,
    pub subagent_type: String,
    pub tool_call_id: String,
    pub title: String,
    pub status: String,
    pub result: String,
    pub quiet_origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningSubagentInfo {
    pub subagent_id: String,
    pub subagent_type: String,
    pub title: String,
    pub elapsed_ms: u64,
    pub tool_call_count: usize,
    pub recent_activity: Vec<String>,
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentSessionSnapshot {
    pub resolved_outline: Vec<Value>,
    pub observed_tool_call_count: usize,
    pub recent_activity: Vec<String>,
    pub transcript_path: Option<String>,
    pub computer_use_usage: Option<ComputerUseUsageSnapshot>,
    pub computer_use_action_counts: HashMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWake {
    pub parent_agent_id: String,
    pub work_id: String,
    pub title: String,
    pub subagent_type: String,
    pub marked_at_ms: u64,
    pub quiet_origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerUseUsageEvent {
    pub parent_agent_id: String,
    pub subagent_agent_id: String,
    pub subagent_type: String,
    pub subagent_request_id: String,
    pub model_id: Option<String>,
    pub outcome: String,
    pub duration_ms: u64,
    pub tool_call_count: usize,
    pub turn_ended_count: u64,
    pub usage: Option<TurnUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerUseAuditRecord {
    pub agent_id: String,
    pub turn_id: Option<String>,
    pub box_id: String,
    pub occurred_at_ms: u64,
    pub tool_call_id: String,
    pub action_count: u64,
    pub action_counts: HashMap<String, u64>,
    pub duration_ms: u64,
    pub screenshot_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed(String),
    Aborted,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerContinuation {
    pub prompt: String,
    pub options: SubagentRunOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettleResult {
    pub continuation: Option<SteerContinuation>,
    pub completion: Option<BackgroundSubagentCompletion>,
    pub pending_wake_disarmed: Option<(String, String)>,
    pub computer_use_usage: Option<ComputerUseUsageEvent>,
    pub computer_use_audit: Option<ComputerUseAuditRecord>,
    pub free_computer_window: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMeta {
    parent_agent_id: String,
    subagent_type: String,
    tool_call_id: String,
    title: String,
    started_at_ms: u64,
    lineage: Option<SubagentLineage>,
    quiet_origin: Option<String>,
    box_id: String,
    lifecycle_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlResult {
    Ok { interrupt_reason: &'static str },
    NotRunning,
}

pub type SubagentAbortObserver = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Default)]
pub struct SubagentRuntime {
    sessions: HashMap<String, SubagentSessionSnapshot>,
    running: HashSet<String>,
    meta: HashMap<String, RuntimeMeta>,
    registry: HashMap<String, SubagentRecord>,
    outlines: HashMap<String, Vec<Value>>,
    pending_steers: HashMap<String, String>,
    aborting: HashSet<String>,
    abort_observer: Option<SubagentAbortObserver>,
}

impl SubagentRuntime {
    pub fn set_abort_observer(&mut self, observer: Option<SubagentAbortObserver>) {
        self.abort_observer = observer;
    }

    fn notify_abort(&self, id: &str) {
        let Some(meta) = self.meta.get(id) else {
            return;
        };
        if let Some(observer) = self.abort_observer.as_ref() {
            observer(&meta.parent_agent_id, id);
        }
    }

    pub fn register_session(&mut self, id: impl Into<String>, snapshot: SubagentSessionSnapshot) {
        self.sessions.insert(id.into(), snapshot);
    }

    pub fn reset(&mut self) {
        self.sessions.clear();
        self.running.clear();
        self.meta.clear();
        self.registry.clear();
        self.outlines.clear();
        self.pending_steers.clear();
        self.aborting.clear();
    }

    pub fn dispatch_background_subagent(
        &mut self,
        parent_agent_id: &str,
        box_id: &str,
        subagent_agent_id: &str,
        subagent_type: &str,
        tool_call_id: &str,
        prompt: &str,
        lineage: Option<SubagentLineage>,
        quiet_origin: Option<&str>,
        now_ms: u64,
    ) -> Option<PendingWake> {
        self.dispatch_background_subagent_with_lifecycle(
            parent_agent_id,
            box_id,
            subagent_agent_id,
            subagent_type,
            tool_call_id,
            prompt,
            lineage,
            quiet_origin,
            "",
            now_ms,
        )
    }

    pub fn dispatch_background_subagent_with_lifecycle(
        &mut self,
        parent_agent_id: &str,
        box_id: &str,
        subagent_agent_id: &str,
        subagent_type: &str,
        tool_call_id: &str,
        prompt: &str,
        lineage: Option<SubagentLineage>,
        quiet_origin: Option<&str>,
        lifecycle_revision: &str,
        now_ms: u64,
    ) -> Option<PendingWake> {
        if self.running.contains(subagent_agent_id) {
            return None;
        }
        let title = derive_background_subagent_title(prompt);
        self.meta.insert(
            subagent_agent_id.to_string(),
            RuntimeMeta {
                parent_agent_id: parent_agent_id.to_string(),
                subagent_type: subagent_type.to_string(),
                tool_call_id: tool_call_id.to_string(),
                title: title.clone(),
                started_at_ms: now_ms,
                lineage,
                quiet_origin: quiet_origin.map(ToOwned::to_owned),
                box_id: box_id.to_string(),
                lifecycle_revision: lifecycle_revision.to_string(),
            },
        );
        self.registry.insert(
            subagent_agent_id.to_string(),
            SubagentRecord {
                subagent_type: subagent_type.to_string(),
                title: title.clone(),
                started_at_ms: now_ms,
                status: SubagentStatus::Running,
            },
        );
        self.running.insert(subagent_agent_id.to_string());
        Some(PendingWake {
            parent_agent_id: parent_agent_id.to_string(),
            work_id: subagent_agent_id.to_string(),
            title,
            subagent_type: subagent_type.to_string(),
            marked_at_ms: now_ms,
            quiet_origin: quiet_origin.map(ToOwned::to_owned),
        })
    }

    pub fn steer_subagent(&mut self, id: &str, message: &str) -> ControlResult {
        if !self.sessions.contains_key(id) || !self.running.contains(id) || self.aborting.contains(id) {
            return ControlResult::NotRunning;
        }
        self.pending_steers.insert(id.to_string(), message.to_string());
        ControlResult::Ok {
            interrupt_reason: "Steering message from the parent agent.",
        }
    }

    pub fn abort_subagent(&mut self, id: &str) -> ControlResult {
        if !self.sessions.contains_key(id) || !self.running.contains(id) {
            return ControlResult::NotRunning;
        }
        let newly_aborting = self.aborting.insert(id.to_string());
        self.pending_steers.remove(id);
        if newly_aborting {
            self.notify_abort(id);
        }
        ControlResult::Ok {
            interrupt_reason: "Stopped by the parent agent.",
        }
    }

    pub fn settle_background_subagent_turn(
        &mut self,
        id: &str,
        outcome: RunOutcome,
        now_ms: u64,
    ) -> SettleResult {
        let pending_steer = self.pending_steers.get(id).cloned();
        if pending_steer.is_some() && self.sessions.contains_key(id) && !self.aborting.contains(id) {
            let pending_steer = self.pending_steers.remove(id).unwrap_or_default();
            if let Some(meta) = self.meta.get(id) {
                return SettleResult {
                    continuation: Some(SteerContinuation {
                        prompt: format_steer_prompt(&pending_steer),
                        options: subagent_steer_run_options(&SubagentDispatchMeta {
                            tool_call_id: meta.tool_call_id.clone(),
                            lineage: meta.lineage.clone(),
                        }),
                    }),
                    ..SettleResult::default()
                };
            }
        }

        self.running.remove(id);
        self.pending_steers.remove(id);
        let abort_requested = self.aborting.remove(id);
        let meta = self.meta.remove(id);
        let snapshot = self.sessions.remove(id);

        if let Some(snapshot) = snapshot.as_ref() {
            self.outlines.insert(id.to_string(), snapshot.resolved_outline.clone());
        }

        let status = if abort_requested || matches!(outcome, RunOutcome::Aborted) {
            SubagentStatus::Aborted
        } else if matches!(outcome, RunOutcome::Completed(_)) {
            SubagentStatus::Done
        } else {
            SubagentStatus::Error
        };
        if let Some(record) = self.registry.get_mut(id) {
            record.status = status;
        }

        let Some(meta) = meta else {
            return SettleResult {
                free_computer_window: true,
                ..SettleResult::default()
            };
        };
        let duration_ms = now_ms.saturating_sub(meta.started_at_ms);
        let is_computer_use = is_computer_use_subagent_type(&meta.subagent_type);
        let mut result = SettleResult {
            free_computer_window: true,
            ..SettleResult::default()
        };

        if is_computer_use {
            let usage = snapshot.as_ref().and_then(|snapshot| snapshot.computer_use_usage.clone());
            result.computer_use_usage = Some(ComputerUseUsageEvent {
                parent_agent_id: meta.parent_agent_id.clone(),
                subagent_agent_id: id.to_string(),
                subagent_type: meta.subagent_type.clone(),
                subagent_request_id: compute_subagent_request_id(&meta.tool_call_id),
                model_id: usage.as_ref().and_then(|usage| usage.model_id.clone()),
                outcome: if abort_requested || matches!(outcome, RunOutcome::Aborted) {
                    "aborted".to_string()
                } else if matches!(outcome, RunOutcome::Completed(_)) {
                    "completed".to_string()
                } else {
                    "error".to_string()
                },
                duration_ms,
                tool_call_count: snapshot.as_ref().map_or(0, |snapshot| snapshot.observed_tool_call_count),
                turn_ended_count: usage.as_ref().map_or(0, |usage| usage.turn_ended_count),
                usage: usage.and_then(|usage| usage.usage),
            });
            if let Some(snapshot) = snapshot.as_ref() {
                let action_count = snapshot.computer_use_action_counts.values().copied().sum();
                result.computer_use_audit = Some(ComputerUseAuditRecord {
                    agent_id: meta.parent_agent_id.clone(),
                    turn_id: (!meta.tool_call_id.is_empty())
                        .then(|| compute_subagent_request_id(&meta.tool_call_id)),
                    box_id: meta.box_id.clone(),
                    occurred_at_ms: now_ms,
                    tool_call_id: meta.tool_call_id.clone(),
                    action_count,
                    action_counts: snapshot.computer_use_action_counts.clone(),
                    duration_ms,
                    screenshot_count: snapshot
                        .computer_use_action_counts
                        .get("screenshot")
                        .copied()
                        .unwrap_or(0),
                });
            }
        }

        if abort_requested {
            result.pending_wake_disarmed =
                Some((meta.parent_agent_id.clone(), id.to_string()));
            return result;
        }

        let completion_status = if matches!(outcome, RunOutcome::Completed(_)) {
            "completed"
        } else {
            "error"
        };
        let completion_text = match outcome {
            RunOutcome::Completed(text) if !text.trim().is_empty() => text.trim().to_string(),
            RunOutcome::Completed(_) => "(the task finished without producing any text output)".to_string(),
            RunOutcome::Aborted => "The background task was interrupted before it finished.".to_string(),
            RunOutcome::Error(error) => error,
        };
        result.completion = Some(BackgroundSubagentCompletion {
            parent_agent_id: meta.parent_agent_id,
            subagent_agent_id: id.to_string(),
            lifecycle_revision: meta.lifecycle_revision,
            marked_at_ms: meta.started_at_ms,
            subagent_type: meta.subagent_type,
            tool_call_id: meta.tool_call_id,
            title: meta.title,
            status: completion_status.to_string(),
            result: completion_text,
            quiet_origin: meta.quiet_origin,
        });
        result
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.contains(id)
    }

    pub fn has_subagent(&self, id: &str) -> bool {
        self.registry.contains_key(id)
    }

    pub fn has_running_subagents(&self) -> bool {
        self.registry
            .values()
            .any(|record| record.status == SubagentStatus::Running)
    }

    pub fn running_subagent_ids_for_parent(&self, parent_agent_id: &str) -> Vec<String> {
        let mut ids = self
            .running
            .iter()
            .filter(|id| {
                self.meta
                    .get(*id)
                    .is_some_and(|meta| meta.parent_agent_id == parent_agent_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn running_subagent_records_for_parent(
        &self,
        parent_agent_id: &str,
    ) -> Vec<(String, SubagentRecord)> {
        let mut records = self
            .running
            .iter()
            .filter(|id| !self.aborting.contains(*id))
            .filter_map(|id| {
                let meta = self.meta.get(id)?;
                if meta.parent_agent_id != parent_agent_id {
                    return None;
                }
                self.registry
                    .get(id)
                    .cloned()
                    .map(|record| (id.clone(), record))
            })
            .collect::<Vec<_>>();
        records.sort_by(|(left_id, left), (right_id, right)| {
            left.started_at_ms
                .cmp(&right.started_at_ms)
                .then_with(|| left_id.cmp(right_id))
        });
        records
    }

    pub fn abort_running_subagents_for_parent(&mut self, parent_agent_id: &str) -> Vec<String> {
        let ids = self.running_subagent_ids_for_parent(parent_agent_id);
        for id in &ids {
            let newly_aborting = self.aborting.insert(id.clone());
            self.pending_steers.remove(id);
            if newly_aborting {
                self.notify_abort(id);
            }
        }
        ids
    }

    pub fn list_subagents(&self) -> Vec<(String, SubagentRecord)> {
        let mut entries = self
            .registry
            .iter()
            .map(|(id, record)| (id.clone(), record.clone()))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(_, record)| record.started_at_ms);
        entries
    }

    pub fn list_running_subagents(&self, now_ms: u64) -> Vec<RunningSubagentInfo> {
        let mut infos = self
            .running
            .iter()
            .filter_map(|id| self.running_info(id, now_ms))
            .collect::<Vec<_>>();
        infos.sort_by_key(|info| {
            self.registry
                .get(&info.subagent_id)
                .map_or(u64::MAX, |record| record.started_at_ms)
        });
        infos
    }

    pub fn get_running_subagent(&self, id: &str, now_ms: u64) -> Option<RunningSubagentInfo> {
        self.running_info(id, now_ms)
    }

    pub fn get_subagent_outline(&self, id: &str) -> Vec<Value> {
        if let Some(session) = self.sessions.get(id) {
            return session.resolved_outline.clone();
        }
        self.outlines.get(id).cloned().unwrap_or_default()
    }

    fn running_info(&self, id: &str, now_ms: u64) -> Option<RunningSubagentInfo> {
        if !self.running.contains(id) {
            return None;
        }
        let meta = self.meta.get(id)?;
        let session = self.sessions.get(id);
        Some(RunningSubagentInfo {
            subagent_id: id.to_string(),
            subagent_type: meta.subagent_type.clone(),
            title: meta.title.clone(),
            elapsed_ms: now_ms.saturating_sub(meta.started_at_ms),
            tool_call_count: session.map_or(0, |session| session.observed_tool_call_count),
            recent_activity: session
                .map(|session| session.recent_activity.clone())
                .unwrap_or_default(),
            transcript_path: session.and_then(|session| session.transcript_path.clone()),
        })
    }
}

pub fn is_computer_use_subagent_type(value: &str) -> bool {
    value
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_' | ' '))
        .collect::<String>()
        .eq_ignore_ascii_case("computeruse")
}
