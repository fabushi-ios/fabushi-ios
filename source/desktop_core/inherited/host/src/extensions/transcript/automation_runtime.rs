use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

use serde_json::{Map, Value};

use crate::automations::automation::{AutomationRecord, AutomationRun, AutomationSpec};
use crate::automations::automation_id::stable_automation_id;
use crate::automations::automation_schedule::summarize_schedule_next_7_days;
use crate::automations::automation_store::FileAutomationStore;
use crate::extensions::session::agent_session::{AgentAutomationEntry, SandAgentSessionStore};
use crate::extensions::session::production::ProductionSessionWorkers;

use super::automation_event_fires::{
    AutomationEventFires, DroppedFire, DroppedFireReporter, EventBatchExecutor, EventFireBatch,
};
use super::automation_run_path::{
    AutomationExecutionResult, AutomationRunFenceSnapshot, AutomationRunPath,
    AutomationRunStore, AutomationRunTrigger, CronSessionRetentionPolicy,
    CronSessionSweepThrottle, FireAutomationArgs, FireAutomationOutcome,
    parse_cron_session_retention,
};
use super::automation_spend_guard_runtime::AutomationSpendGuardRuntime;
use super::automation_snapshot::{
    AutomationAction as AutomationDiffAction, AutomationSnapshot, diff_automation_action,
    snapshot_automations,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutomationCommandError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Internal(String),
}

pub fn dispatch_automation_command(
    runtime: &AutomationRuntime,
    method: &str,
    args: &Value,
) -> Option<Result<Value, AutomationCommandError>> {
    if !matches!(
        method,
        "getAgentAutomations"
            | "createAgentAutomation"
            | "updateAgentAutomation"
            | "setAgentAutomationEnabled"
            | "deleteAgentAutomation"
    ) {
        return None;
    }

    let agent_id = match required_automation_string(args, &["id", "agentId"], "id") {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    let result = match method {
        "getAgentAutomations" => runtime
            .get_agent_automations(agent_id)
            .map(automation_records_value)
            .map_err(AutomationCommandError::Internal),
        "createAgentAutomation" => {
            let spec = match automation_spec(args.get("spec").unwrap_or(args)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            runtime
                .create_agent_automation(agent_id, &spec)
                .map(|(records, _events)| automation_records_value(records))
                .map_err(AutomationCommandError::Internal)
        }
        "updateAgentAutomation" => {
            let automation_id = match required_automation_string(
                args,
                &["automationId"],
                "automationId",
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let spec = match automation_spec(args.get("spec").unwrap_or(args)) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            runtime
                .update_agent_automation(agent_id, automation_id, &spec)
                .map(|(records, _events)| automation_records_value(records))
                .map_err(AutomationCommandError::Internal)
        }
        "setAgentAutomationEnabled" => {
            let automation_id = match required_automation_string(
                args,
                &["automationId"],
                "automationId",
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            let is_enabled = match args.get("isEnabled").and_then(Value::as_bool) {
                Some(value) => value,
                None => {
                    return Some(Err(AutomationCommandError::BadRequest(
                        "setAgentAutomationEnabled requires boolean isEnabled".into(),
                    )));
                }
            };
            runtime
                .set_agent_automation_enabled(agent_id, automation_id, is_enabled)
                .map(|(records, _events)| automation_records_value(records))
                .map_err(AutomationCommandError::Internal)
        }
        "deleteAgentAutomation" => {
            let automation_id = match required_automation_string(
                args,
                &["automationId"],
                "automationId",
            ) {
                Ok(value) => value,
                Err(error) => return Some(Err(error)),
            };
            runtime
                .delete_agent_automation(agent_id, automation_id)
                .map(|(records, _events)| automation_records_value(records))
                .map_err(AutomationCommandError::Internal)
        }
        _ => unreachable!(),
    };
    Some(result)
}

fn required_automation_string<'a>(
    value: &'a Value,
    keys: &[&str],
    label: &str,
) -> Result<&'a str, AutomationCommandError> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AutomationCommandError::BadRequest(format!(
                "automation command requires non-empty {label}"
            ))
        })
}

fn automation_spec(value: &Value) -> Result<AutomationSpec, AutomationCommandError> {
    let object = value.as_object().ok_or_else(|| {
        AutomationCommandError::BadRequest("automation spec must be an object".into())
    })?;
    Ok(AutomationSpec {
        name: object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        prompt: object
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        trigger: object.get("trigger").cloned().unwrap_or(Value::Null),
        is_enabled: object.get("isEnabled").and_then(Value::as_bool),
    })
}

pub fn automation_records_value(records: Vec<AutomationRecord>) -> Value {
    Value::Array(records.into_iter().map(automation_record_value).collect())
}

pub fn automation_record_value(record: AutomationRecord) -> Value {
    let runs = record
        .runs
        .into_iter()
        .map(|run| {
            let mut value = Map::new();
            value.insert("id".into(), Value::String(run.id));
            value.insert("trigger".into(), Value::String(run.trigger));
            value.insert("startedAt".into(), number_value(run.started_at));
            if let Some(finished_at) = run.finished_at {
                value.insert("finishedAt".into(), number_value(finished_at));
            }
            value.insert("status".into(), Value::String(run.status));
            if let Some(detail) = run.detail {
                value.insert("detail".into(), Value::String(detail));
            }
            if let Some(event) = run.event {
                value.insert("event".into(), Value::String(event));
            }
            if let Some(ids) = run.coalesced_run_ids {
                value.insert(
                    "coalescedRunIds".into(),
                    Value::Array(ids.into_iter().map(Value::String).collect()),
                );
            }
            Value::Object(value)
        })
        .collect::<Vec<_>>();

    let mut value = Map::new();
    value.insert("id".into(), Value::String(record.id));
    value.insert("name".into(), Value::String(record.name));
    value.insert("prompt".into(), Value::String(record.prompt));
    value.insert("trigger".into(), record.trigger);
    value.insert(
        "isEnabled".into(),
        Value::Bool(record.is_enabled),
    );
    value.insert("createdAt".into(), number_value(record.created_at));
    if let Some(last_run_at) = record.last_run_at {
        value.insert("lastRunAt".into(), number_value(last_run_at));
    }
    value.insert(
        "raisedNotices".into(),
        Value::Array(record.raised_notices.into_iter().map(Value::String).collect()),
    );
    value.insert("schedule".into(), Value::String(record.schedule));
    value.insert(
        "triggerDescription".into(),
        Value::String(record.trigger_description),
    );
    if let Some(next_run_at) = record.next_run_at {
        value.insert("nextRunAt".into(), number_value(next_run_at));
    }
    value.insert("runs".into(), Value::Array(runs));
    value.insert(
        "filePath".into(),
        Value::String(record.file_path.to_string_lossy().into_owned()),
    );
    Value::Object(value)
}

fn number_value(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationLifecycleSource {
    Agent,
    AutomationsUi,
    WorkflowUi,
    SpendGuard,
}

impl AutomationLifecycleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::AutomationsUi => "automations_ui",
            Self::WorkflowUi => "workflow_ui",
            Self::SpendGuard => "spend_guard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationLifecycleAction {
    Created,
    Updated,
    Enabled,
    Disabled,
    Deleted,
}

impl AutomationLifecycleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationLifecycleEvent {
    pub agent_id: String,
    pub action: AutomationLifecycleAction,
    pub automation_id: String,
    pub stable_automation_id: String,
    pub automation_name: String,
    pub trigger_type: String,
    pub created_at: f64,
    pub age_ms: f64,
    pub recorded_run_count: usize,
    pub scheduled_fires_next_7_days: Option<usize>,
    pub fires_on_weekend: Option<bool>,
    pub fires_overnight: Option<bool>,
    pub source: AutomationLifecycleSource,
}

pub type AutomationLifecycleReporter =
    Arc<dyn Fn(&AutomationLifecycleEvent) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct AutomationRuntime {
    sessions: Arc<ProductionSessionWorkers>,
    run_path: Arc<AutomationRunPath>,
    event_fires: Arc<AutomationEventFires>,
    spend_guard: Arc<AutomationSpendGuardRuntime>,
    wakes_suspended: Arc<AtomicBool>,
    last_known: Arc<Mutex<HashMap<String, BTreeMap<String, AutomationSnapshot>>>>,
    mutation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    lifecycle_reporter: Arc<Mutex<Option<AutomationLifecycleReporter>>>,
    cron_sweep_throttle: Arc<Mutex<CronSessionSweepThrottle>>,
}

struct AdmittedAutomationRunStore<'a> {
    session: SandAgentSessionStore,
    agent_id: &'a str,
    store: &'a FileAutomationStore,
}

impl AutomationRunStore for AdmittedAutomationRunStore<'_> {
    fn resolved_user_time_zone(&self) -> Option<String> {
        self.store.resolved_user_time_zone()
    }

    fn mark_notice_raised(
        &self,
        automation_id: &str,
        notice: &str,
    ) -> Result<Option<AutomationRecord>, String> {
        self.session
            .with_admitted_filesystem_mutation(self.agent_id, || {
                self.store
                    .mark_notice_raised(automation_id, notice)
                    .map_err(|error| error.to_string())
            })
    }

    fn record_run(
        &self,
        automation_id: &str,
        fired_at_ms: f64,
    ) -> Result<Option<AutomationRecord>, String> {
        self.session
            .with_admitted_filesystem_mutation(self.agent_id, || {
                self.store
                    .record_run(automation_id, fired_at_ms)
                    .map_err(|error| error.to_string())
            })
    }

    fn begin_run(
        &self,
        automation_id: &str,
        trigger: &str,
        fired_at_ms: f64,
        event: Option<&str>,
        run_id: Option<&str>,
        coalesced_run_ids: Option<&[String]>,
    ) -> Result<Option<AutomationRun>, String> {
        self.session
            .with_admitted_filesystem_mutation(self.agent_id, || {
                self.store
                    .begin_run(
                        automation_id,
                        trigger,
                        fired_at_ms,
                        event,
                        run_id,
                        coalesced_run_ids,
                    )
                    .map_err(|error| error.to_string())
            })
    }

    fn finish_run(
        &self,
        automation_id: &str,
        run_id: &str,
        status: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
    ) -> Result<Option<AutomationRecord>, String> {
        self.session
            .with_admitted_filesystem_mutation(self.agent_id, || {
                self.store
                    .finish_run(automation_id, run_id, status, finished_at_ms, detail)
                    .map_err(|error| error.to_string())
            })
    }

    fn begin_cron_session_run(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        started_at_ms: f64,
    ) -> Result<(), String> {
        if agent_id != self.agent_id {
            return Err("cron session registration agent identity mismatch".into());
        }
        let changed = self.session.begin_cron_session_run(
            agent_id,
            automation_id,
            run_id,
            started_at_ms,
        )?;
        if changed {
            Ok(())
        } else {
            Err("cron session run registration was not committed".into())
        }
    }

    fn finish_cron_session_run(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
    ) -> Result<(), String> {
        if agent_id != self.agent_id {
            return Err("cron session settlement agent identity mismatch".into());
        }
        let changed = self.session.finish_cron_session_run(
            agent_id,
            automation_id,
            run_id,
            outcome,
            finished_at_ms,
            detail,
        )?;
        if changed {
            Ok(())
        } else {
            Err("cron session run settlement was stale or not committed".into())
        }
    }

    fn publish_cron_session_fences(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        fences: AutomationRunFenceSnapshot,
    ) -> Result<(), String> {
        if agent_id != self.agent_id {
            return Err("cron fence publication agent identity mismatch".into());
        }
        let changed = self.session.publish_cron_session_run_fences(
            agent_id,
            automation_id,
            run_id,
            fences.active_controller,
            fences.pending_outgoing_media,
            fences.unsettled_subagent,
        )?;
        if changed {
            Ok(())
        } else {
            Err("cron session fence publication was stale or not committed".into())
        }
    }

    fn finish_cron_session_run_with_fences(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
        fences: AutomationRunFenceSnapshot,
    ) -> Result<(), String> {
        if agent_id != self.agent_id {
            return Err("cron session settlement agent identity mismatch".into());
        }
        let changed = self.session.finish_cron_session_run_with_fences(
            agent_id,
            automation_id,
            run_id,
            outcome,
            finished_at_ms,
            detail,
            fences.active_controller,
            fences.pending_outgoing_media,
            fences.unsettled_subagent,
        )?;
        if changed {
            Ok(())
        } else {
            Err("cron session fenced settlement was stale or not committed".into())
        }
    }
}

impl AutomationRuntime {
    pub fn new(sessions: Arc<ProductionSessionWorkers>) -> Self {
        Self {
            sessions: Arc::clone(&sessions),
            run_path: Arc::new(AutomationRunPath::default()),
            event_fires: Arc::new(AutomationEventFires::default()),
            spend_guard: Arc::new(AutomationSpendGuardRuntime::new(sessions)),
            wakes_suspended: Arc::new(AtomicBool::new(false)),
            last_known: Arc::new(Mutex::new(HashMap::new())),
            mutation_locks: Arc::new(Mutex::new(HashMap::new())),
            lifecycle_reporter: Arc::new(Mutex::new(None)),
            cron_sweep_throttle: Arc::new(Mutex::new(CronSessionSweepThrottle::default())),
        }
    }

    pub fn run_path(&self) -> Arc<AutomationRunPath> {
        Arc::clone(&self.run_path)
    }

    /// Request a throttled, read-only cron candidate projection through the
    /// existing automation and session owners. A scheduler may consume the
    /// result later; this bridge never archives or deletes a row.
    pub fn list_expired_cron_session_candidates(
        &self,
        agent_id: &str,
        retention_value: Option<&Value>,
        now_ms: f64,
    ) -> Result<Vec<crate::extensions::session::agent_db::CronSessionRunRecord>, String> {
        let retention_ms = match parse_cron_session_retention(retention_value) {
            CronSessionRetentionPolicy::Disabled => return Ok(Vec::new()),
            CronSessionRetentionPolicy::DurationMs(value) => value,
        };
        if !now_ms.is_finite() || now_ms < 0.0 {
            return Ok(Vec::new());
        }
        let now_ms_u64 = if now_ms >= u64::MAX as f64 {
            u64::MAX
        } else {
            now_ms.floor() as u64
        };
        let admitted = self
            .cron_sweep_throttle
            .lock()
            .map_err(|_| "cron sweep throttle lock poisoned".to_string())?
            .admit(agent_id, now_ms_u64);
        if !admitted {
            return Ok(Vec::new());
        }
        match self
            .sessions
            .list_expired_cron_session_runs(agent_id, now_ms, Some(retention_ms))
        {
            Ok(rows) => Ok(rows),
            Err(error) => {
                if let Ok(mut throttle) = self.cron_sweep_throttle.lock() {
                    throttle.forget_owner(agent_id);
                }
                Err(error)
            }
        }
    }

    pub fn event_fires(&self) -> Arc<AutomationEventFires> {
        Arc::clone(&self.event_fires)
    }

    pub fn set_dropped_fire_reporter(&self, reporter: Option<DroppedFireReporter>) {
        self.event_fires.set_dropped_fire_reporter(reporter);
    }

    pub fn set_lifecycle_reporter(&self, reporter: Option<AutomationLifecycleReporter>) {
        *self.lifecycle_reporter.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = reporter;
    }

    /// Freeze only background automation wakes while a Host upgrade is being
    /// prepared. Manual user-triggered runs remain available, matching Grok's
    /// automations.suspendWakes() boundary rather than globally disabling CRUD.
    pub fn suspend_wakes(&self) {
        self.wakes_suspended.store(true, Ordering::Release);
    }

    pub fn resume_wakes(&self) {
        self.wakes_suspended.store(false, Ordering::Release);
    }

    pub fn wakes_suspended(&self) -> bool {
        self.wakes_suspended.load(Ordering::Acquire)
    }

    pub fn dispose(&self) {
        self.suspend_wakes();
        self.event_fires.dispose();
        self.event_fires.set_dropped_fire_reporter(None);
        self.run_path.set_run_reporter(None);
        self.set_lifecycle_reporter(None);
        self.last_known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.mutation_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    fn report_fire_dropped(&self, args: &FireAutomationArgs, reason: &str) {
        self.event_fires.report_fire_dropped(DroppedFire {
            agent_id: args.agent_id.clone(),
            trigger: args.trigger,
            reason: reason.to_string(),
            scheduled_for_ms: None,
            run_uuid: args.run_uuid.clone(),
        });
    }

    pub fn spend_guard(&self) -> Arc<AutomationSpendGuardRuntime> {
        Arc::clone(&self.spend_guard)
    }

    pub fn get_agent_automations(&self, agent_id: &str) -> Result<Vec<AutomationRecord>, String> {
        SandAgentSessionStore::new(Arc::clone(&self.sessions)).list_agent_automations(agent_id)
    }

    pub fn list_all_automations(&self) -> Result<Vec<AgentAutomationEntry>, String> {
        SandAgentSessionStore::new(Arc::clone(&self.sessions)).list_all_automations()
    }

    pub fn list_all_automation_definitions(&self) -> Result<Vec<AgentAutomationEntry>, String> {
        SandAgentSessionStore::new(Arc::clone(&self.sessions)).list_all_automation_definitions()
    }

    pub fn seed_known_automations(&self, agent_id: &str) -> Result<(), String> {
        self.with_agent_mutation_lock(agent_id, || {
            let definitions = self.automation_store(agent_id)?.list_definitions();
            self.last_known
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(agent_id.to_string())
                .or_insert_with(|| snapshot_automations(&definitions));
            Ok(())
        })
    }

    /// Frozen Grok watches the active session's automation directory. The
    /// manager retains the returned store for exactly the active-session
    /// lifetime; the callback re-opens the store so it does not self-own its
    /// WatchedDirectory.
    pub fn watch_agent_automations(
        &self,
        agent_id: &str,
    ) -> Result<FileAutomationStore, String> {
        let store = self.automation_store(agent_id)?;
        let definitions = store.list_definitions();
        self.last_known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_insert_with(|| snapshot_automations(&definitions));
        let runtime = self.clone();
        let watched_agent_id = agent_id.to_string();
        store.set_on_change(Some(Arc::new(move || {
            let _ = runtime.record_agent_store_change(&watched_agent_id);
        })));
        Ok(store)
    }

    fn record_agent_store_change(&self, agent_id: &str) -> Result<(), String> {
        self.with_agent_mutation_lock(agent_id, || {
            let current = self.automation_store(agent_id)?.list_definitions();
            let _ = self.record_changes(
                agent_id,
                &current,
                &current,
                AutomationLifecycleSource::Agent,
            );
            Ok(())
        })
    }

    pub fn create_agent_automation(
        &self,
        agent_id: &str,
        spec: &AutomationSpec,
    ) -> Result<(Vec<AutomationRecord>, Vec<AutomationLifecycleEvent>), String> {
        self.mutate_agent(
            agent_id,
            AutomationLifecycleSource::AutomationsUi,
            |store| {
                store
                    .upsert(spec, now_ms())
                    .map_err(|error| error.to_string())?;
                Ok(store.list())
            },
        )
    }

    pub fn update_agent_automation(
        &self,
        agent_id: &str,
        automation_id: &str,
        spec: &AutomationSpec,
    ) -> Result<(Vec<AutomationRecord>, Vec<AutomationLifecycleEvent>), String> {
        self.mutate_agent(
            agent_id,
            AutomationLifecycleSource::AutomationsUi,
            |store| {
                store
                    .update(automation_id, spec)
                    .map_err(|error| error.to_string())?;
                Ok(store.list())
            },
        )
    }

    pub fn set_agent_automation_enabled(
        &self,
        agent_id: &str,
        automation_id: &str,
        enabled: bool,
    ) -> Result<(Vec<AutomationRecord>, Vec<AutomationLifecycleEvent>), String> {
        self.mutate_agent(
            agent_id,
            AutomationLifecycleSource::AutomationsUi,
            |store| {
                store
                    .set_enabled(automation_id, enabled)
                    .map_err(|error| error.to_string())?;
                Ok(store.list())
            },
        )
    }

    pub fn delete_agent_automation(
        &self,
        agent_id: &str,
        automation_id: &str,
    ) -> Result<(Vec<AutomationRecord>, Vec<AutomationLifecycleEvent>), String> {
        self.mutate_agent(
            agent_id,
            AutomationLifecycleSource::AutomationsUi,
            |store| {
                store
                    .remove(automation_id)
                    .map_err(|error| error.to_string())?;
                Ok(store.list())
            },
        )
    }

    pub fn run_agent_automation_now_with<Execute>(
        &self,
        agent_id: &str,
        automation_id: &str,
        execute: Execute,
    ) -> Result<Option<FireAutomationOutcome>, String>
    where
        Execute: FnOnce(&str) -> Result<AutomationExecutionResult, String>,
    {
        let Some((store, automation, before)) = self.with_agent_mutation_lock(agent_id, || {
            let store = self.automation_store(agent_id)?;
            let Some(automation) = store.get(automation_id) else {
                return Ok(None);
            };
            let before = store.list_definitions();
            let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);
            Ok(Some((store, automation, before)))
        })? else {
            return Ok(None);
        };

        let mut args = FireAutomationArgs::manual(agent_id, automation, now_ms());
        args.is_group = Some(false);
        let runtime = self.clone();
        let admitted_store = AdmittedAutomationRunStore {
            session: SandAgentSessionStore::new(Arc::clone(&self.sessions)),
            agent_id,
            store: &store,
        };
        let outcome = self.run_path.fire_automation_with_on_duplicate(
            &admitted_store,
            args,
            move |args| runtime.report_fire_dropped(args, "duplicate_in_flight"),
            execute,
        )?;
        self.with_agent_mutation_lock(agent_id, || {
            let after = store.list_definitions();
            let _ = self.record_changes(
                agent_id,
                &before,
                &after,
                AutomationLifecycleSource::Agent,
            );
            Ok(())
        })?;
        Ok(outcome)
    }

    pub fn run_background_automation_with<Execute>(
        &self,
        agent_id: &str,
        automation_id: &str,
        trigger: AutomationRunTrigger,
        events: Vec<Value>,
        run_uuid: Option<String>,
        coalesced_run_uuids: Vec<String>,
        fired_at_ms: f64,
        scheduled_for_ms: Option<f64>,
        execute: Execute,
    ) -> Result<Option<FireAutomationOutcome>, String>
    where
        Execute: FnOnce(&str) -> Result<AutomationExecutionResult, String>,
    {
        if !trigger.is_background() {
            return Err("background automation run requires schedule or event trigger".into());
        }
        if self.wakes_suspended() {
            return Ok(None);
        }
        // Spend Guard owns the Session filesystem/admission boundary. Keep
        // only this Runtime's mutation mutex here so the Session lock is not
        // acquired recursively.
        let Some((store, automation, reminder)) = self.with_agent_mutation_lock(agent_id, || {
            let store = self.automation_store(agent_id)?;
            let Some(automation) = store.get(automation_id) else {
                return Ok(None);
            };
            let before = store.list_definitions();
            let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);
            let guard = self.spend_guard.apply_with_store(
                agent_id,
                &store,
                automation_id,
                fired_at_ms,
            )?;
            let after_guard = store.list_definitions();
            let _ = self.record_changes(
                agent_id,
                &before,
                &after_guard,
                AutomationLifecycleSource::SpendGuard,
            );
            if guard.paused {
                self.event_fires.report_fire_dropped(DroppedFire {
                    agent_id: agent_id.to_string(),
                    trigger,
                    reason: "user_away_paused".into(),
                    scheduled_for_ms: None,
                    run_uuid: run_uuid.clone(),
                });
                return Ok(None);
            }
            Ok(Some((store, automation, guard.reminder)))
        })? else {
            return Ok(None);
        };

        let args = FireAutomationArgs {
            agent_id: agent_id.to_string(),
            automation,
            trigger,
            events,
            run_uuid,
            coalesced_run_uuids,
            fired_at_ms,
            scheduled_for_ms,
            is_group: Some(false),
            sent_message_count: None,
            spend_guard_reminder: reminder,
        };
        let runtime = self.clone();
        let admitted_store = AdmittedAutomationRunStore {
            session: SandAgentSessionStore::new(Arc::clone(&self.sessions)),
            agent_id,
            store: &store,
        };
        let outcome = self.run_path.fire_automation_with_on_duplicate(
            &admitted_store,
            args,
            move |args| runtime.report_fire_dropped(args, "duplicate_in_flight"),
            execute,
        )?;
        self.with_agent_mutation_lock(agent_id, || {
            let current = store.list_definitions();
            let _ = self.record_changes(
                agent_id,
                &current,
                &current,
                AutomationLifecycleSource::Agent,
            );
            Ok(())
        })?;
        Ok(outcome)
    }

    pub fn enqueue_event_automation_fire_with(
        &self,
        agent_id: &str,
        automation_id: &str,
        event: Value,
        run_uuid: Option<String>,
        execute: Arc<
            dyn Fn(&str, &str, &str, &str) -> Result<AutomationExecutionResult, String>
                + Send
                + Sync
                + 'static,
        >,
    ) -> Result<Option<FireAutomationOutcome>, String> {
        if self.wakes_suspended() {
            return Ok(None);
        }
        let store = self.automation_store(agent_id)?;
        let Some(automation) = store.get(automation_id) else {
            return Ok(None);
        };
        let runtime = self.clone();
        let executor: EventBatchExecutor = Arc::new(move |batch: EventFireBatch| {
            let execute = Arc::clone(&execute);
            let agent_id = batch.agent_id.clone();
            let automation_id = batch.automation.id.clone();
            let automation_name = batch.automation.name.clone();
            runtime.run_background_automation_with(
                &batch.agent_id,
                &batch.automation.id,
                AutomationRunTrigger::Event,
                batch.events,
                batch.run_uuid,
                batch.coalesced_run_uuids,
                now_ms(),
                None,
                move |prompt| {
                    execute(
                        &agent_id,
                        &automation_id,
                        &automation_name,
                        prompt,
                    )
                },
            )
        });
        let receiver = self.event_fires.enqueue_event_automation_fire(
            agent_id,
            automation,
            event,
            run_uuid,
            executor,
        );
        receiver
            .recv()
            .map_err(|_| "automation event batch settled without a result".to_string())
    }

    pub fn run_server_scheduled_automation_with<Execute>(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_uuid: String,
        scheduled_for_ms: Option<f64>,
        execute: Execute,
    ) -> Result<Option<FireAutomationOutcome>, String>
    where
        Execute: FnOnce(&str) -> Result<AutomationExecutionResult, String>,
    {
        self.run_background_automation_with(
            agent_id,
            automation_id,
            AutomationRunTrigger::Schedule,
            Vec::new(),
            Some(run_uuid),
            Vec::new(),
            now_ms(),
            scheduled_for_ms,
            execute,
        )
    }

    pub fn handle_spend_guard_answer(
        &self,
        agent_id: &str,
        entry_id: &str,
        value: &str,
        now_ms: f64,
    ) -> Result<Option<String>, String> {
        // The guard answer acquires the Session admission boundary once.
        self.with_agent_mutation_lock(agent_id, || {
            let store = self.automation_store(agent_id)?;
            let before = store.list_definitions();
            let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);
            let ack = self.spend_guard.handle_widget_answer_with_store(
                agent_id,
                &store,
                entry_id,
                value,
                now_ms,
            )?;
            if ack.is_some() {
                let after = store.list_definitions();
                let _ = self.record_changes(
                    agent_id,
                    &before,
                    &after,
                    AutomationLifecycleSource::SpendGuard,
                );
            }
            Ok(ack)
        })
    }

    pub fn with_workflow_ui_mutation<T, Mutation>(
        &self,
        agent_id: &str,
        mutation: Mutation,
    ) -> Result<(T, Vec<AutomationLifecycleEvent>), String>
    where
        Mutation: FnOnce(&SandAgentSessionStore) -> Result<T, String>,
    {
        self.with_agent_mutation_lock(agent_id, || {
            let session = SandAgentSessionStore::new(Arc::clone(&self.sessions));
            let before = session.automation_store_for(agent_id)?.list_definitions();
            let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);
            let result = mutation(&session)?;
            let after = session.automation_store_for(agent_id)?.list_definitions();
            let events = self.record_changes(
                agent_id,
                &before,
                &after,
                AutomationLifecycleSource::WorkflowUi,
            );
            Ok((result, events))
        })
    }

    pub fn record_external_changes(
        &self,
        agent_id: &str,
        before: &[AutomationRecord],
        after: &[AutomationRecord],
        source: AutomationLifecycleSource,
    ) -> Vec<AutomationLifecycleEvent> {
        self.record_changes(agent_id, before, after, source)
    }

    fn mutate_agent<T, Mutation>(
        &self,
        agent_id: &str,
        source: AutomationLifecycleSource,
        mutation: Mutation,
    ) -> Result<(T, Vec<AutomationLifecycleEvent>), String>
    where
        Mutation: FnOnce(&FileAutomationStore) -> Result<T, String>,
    {
        self.with_admitted_agent_mutation(agent_id, || {
            let store = self.automation_store(agent_id)?;
            let before = store.list_definitions();
            let _ = self.record_changes(agent_id, &before, &before, AutomationLifecycleSource::Agent);
            let result = mutation(&store)?;
            let after = store.list_definitions();
            let events = self.record_changes(agent_id, &before, &after, source);
            Ok((result, events))
        })
    }

    fn with_admitted_agent_mutation<T>(
        &self,
        agent_id: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_agent_mutation_lock(agent_id, || {
            SandAgentSessionStore::new(Arc::clone(&self.sessions))
                .with_admitted_filesystem_mutation(agent_id, operation)
        })
    }

    fn record_changes(
        &self,
        agent_id: &str,
        before: &[AutomationRecord],
        after: &[AutomationRecord],
        source: AutomationLifecycleSource,
    ) -> Vec<AutomationLifecycleEvent> {
        let mut known = self
            .last_known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = known
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| snapshot_automations(before));
        let current = snapshot_automations(after);
        known.insert(agent_id.to_string(), current.clone());
        drop(known);

        let mut events = Vec::new();
        for (id, after) in &current {
            let action = match previous.get(id) {
                None => Some(AutomationLifecycleAction::Created),
                Some(before) => diff_automation_action(before, after).map(map_diff_action),
            };
            if let Some(action) = action {
                events.push(self.lifecycle_event(agent_id, after, action, source));
            }
        }
        for (id, before) in &previous {
            if !current.contains_key(id) {
                events.push(self.lifecycle_event(
                    agent_id,
                    before,
                    AutomationLifecycleAction::Deleted,
                    source,
                ));
            }
        }
        let reporter = self.lifecycle_reporter.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(reporter) = reporter {
            for event in &events {
                reporter(event);
            }
        }
        events
    }

    fn lifecycle_event(
        &self,
        agent_id: &str,
        snapshot: &AutomationSnapshot,
        action: AutomationLifecycleAction,
        source: AutomationLifecycleSource,
    ) -> AutomationLifecycleEvent {
        let observed_at_ms = now_ms();
        let summary = (snapshot.trigger_type == "cron").then(|| {
            let time_zone = self.sessions.resolve_user_time_zone();
            summarize_schedule_next_7_days(&snapshot.schedule, time_zone.as_deref(), observed_at_ms)
        });
        AutomationLifecycleEvent {
            agent_id: agent_id.to_string(),
            action,
            automation_id: snapshot.id.clone(),
            stable_automation_id: stable_automation_id(agent_id, &snapshot.id),
            automation_name: snapshot.name.clone(),
            trigger_type: snapshot.trigger_type.clone(),
            created_at: snapshot.created_at,
            age_ms: (observed_at_ms - snapshot.created_at).max(0.0),
            recorded_run_count: snapshot.recorded_run_count,
            scheduled_fires_next_7_days: summary.map(|value| value.scheduled_fires_next_7_days),
            fires_on_weekend: summary.map(|value| value.fires_on_weekend),
            fires_overnight: summary.map(|value| value.fires_overnight),
            source,
        }
    }

    fn sync_baseline(&self, agent_id: &str, definitions: &[AutomationRecord]) {
        self.last_known
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_insert_with(|| snapshot_automations(definitions));
    }

    fn automation_store(&self, agent_id: &str) -> Result<FileAutomationStore, String> {
        SandAgentSessionStore::new(Arc::clone(&self.sessions)).automation_store_for(agent_id)
    }

    fn with_agent_mutation_lock<T>(
        &self,
        agent_id: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let lock = {
            let mut locks = self
                .mutation_locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Arc::clone(
                locks
                    .entry(agent_id.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _guard = lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation()
    }
}

fn map_diff_action(action: AutomationDiffAction) -> AutomationLifecycleAction {
    match action {
        AutomationDiffAction::Updated => AutomationLifecycleAction::Updated,
        AutomationDiffAction::Enabled => AutomationLifecycleAction::Enabled,
        AutomationDiffAction::Disabled => AutomationLifecycleAction::Disabled,
    }
}


fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}
