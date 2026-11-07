use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::session_paths::resolve_internal_send_route;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::sand_activity::{ActivityUpdate, AgentActivity};

use super::async_task_union::{AsyncTask, merge_async_tasks};
use super::box_request_entries::{ActiveBoxRequest, BoxRequestTrackDecision};
use super::prompt_acceptance_ledger::{
    AcceptanceLookup, AcceptanceRecord, AcceptanceStatus, PromptAcceptanceError,
    PromptAcceptanceLedger, SendInput,
};
use super::run_lifecycle::RunLifecycleState;
use super::send_group_fanout::GroupMemberTurnExecutor;
use super::sand_pending_wake_store::{DurablePendingWakeMarker, PendingWakeKind, SandPendingWakeStore};
use super::sand_upgrade_resume_store::{SandUpgradeResumeStore, UpgradeResumeMarker};
use super::session_runtime::SessionRuntime;
use super::replica_writer::HostReplicaWriter;
use super::run_scheduler::{
    QueueAccepted, QueueDequeued, RUN_WATCHDOG_DEFAULT_MS, RUN_WATCHDOG_GRACE_DEFAULT_MS,
    RunLane, RunSettlement, WatchdogEvent,
};
use super::pending_wake_rearm::is_recreate_wake_carry_disabled;
use super::send_pipeline::{
    HOST_ACCOUNT_SLOT, PersistedSendContext, SendBegin, SendEchoIdentity, SendPipelineState,
};
use super::turn_runtime::{QueuedTurnRecoveryCheck, should_supersede_stale_turn};
use super::send_turn_dispatch::{ProductionTurnDispatch, UserTurnTicket};
use super::upgrade_recreate_resume::{
    UpgradeQuiesceSummary, UpgradeRecreateResume,
};

const COMPLETION_CACHE_MAX: usize = 256;

pub type AgentRunLifecycleObserver = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type SharedGroupRoomEntryPublisher =
    Arc<dyn Fn(&str, &Value) -> Result<(), String> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProductionSendError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedSendAcceptance {
    pub duplicate: bool,
    pub context: PersistedSendContext,
}

pub fn classify_send_dispatch(
    args: &Value,
) -> Result<(RunLane, &'static str), ProductionSendError> {
    let request_source = optional_non_empty(args, "requestSource");
    let is_handoff_resume = request_source == Some("handoff-resume");
    let is_automation = request_source == Some("automation");
    let is_upgrade_resume = request_source == Some("upgrade-resume");
    let is_event = request_source == Some("event");
    let is_group_member = request_source == Some("group-member");
    let is_agent_inbound = request_source == Some("agent-inbound");
    let is_human_handoff = request_source == Some("human-handoff");
    let is_kickstart = request_source == Some("kickstart");
    let is_ack_redrive =
        optional_bool(args, "ackRedrive")?.unwrap_or(false) && is_handoff_resume;
    Ok((
        if is_handoff_resume || is_automation || is_upgrade_resume || is_event {
            RunLane::Background
        } else if is_agent_inbound {
            RunLane::Agent
        } else {
            RunLane::User
        },
        if is_ack_redrive {
            "ack-redrive"
        } else if is_handoff_resume {
            "handoff-resume"
        } else if is_automation {
            "automation"
        } else if is_upgrade_resume {
            "upgrade-resume"
        } else if is_event {
            "event"
        } else if is_agent_inbound {
            "agent-inbound"
        } else if is_human_handoff {
            "human-handoff"
        } else if is_group_member {
            "group-member"
        } else if is_kickstart {
            "kickstart"
        } else {
            "turn"
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutedTurnLease {
    agent_id: String,
    ticket: UserTurnTicket,
    generation: u64,
    is_group_member_turn: bool,
    human_handoff_conversation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedTurnSettlement {
    pub watchdog: Option<WatchdogEvent>,
    pub next: Option<QueueDequeued>,
}

struct RuntimeState {
    pipeline: SendPipelineState,
    ledger: PromptAcceptanceLedger,
    lifecycle: RunLifecycleState,
    turn_dispatch: ProductionTurnDispatch,
    routed_turn_leases: HashMap<String, RoutedTurnLease>,
    completions: HashMap<String, Result<Value, ProductionSendError>>,
    completion_order: VecDeque<String>,
}

pub struct ProductionTranscriptRuntime {
    state: Mutex<RuntimeState>,
    send_settled: Condvar,
    turn_ready: Condvar,
    pending_wake_store: Option<SandPendingWakeStore>,
    upgrade_resume_store: Option<SandUpgradeResumeStore>,
    upgrade_recreate_resume: UpgradeRecreateResume,
    session_runtime: SessionRuntime,
    replica_writer: HostReplicaWriter,
    shared_group_remote_executor: Mutex<Option<GroupMemberTurnExecutor>>,
    shared_group_room_entry_publisher: Mutex<Option<SharedGroupRoomEntryPublisher>>,
    roster_snapshot_seq: AtomicU64,
    agent_run_lifecycle_observer: Mutex<Option<AgentRunLifecycleObserver>>,
}

impl ProductionTranscriptRuntime {
    pub fn new(root_dir: Option<&Path>) -> Self {
        Self::with_watchdog(
            root_dir,
            RUN_WATCHDOG_DEFAULT_MS,
            RUN_WATCHDOG_GRACE_DEFAULT_MS,
        )
    }

    pub fn with_watchdog(
        root_dir: Option<&Path>,
        watchdog_ms: u64,
        watchdog_grace_ms: u64,
    ) -> Self {
        let pending_wake_store = root_dir.map(SandPendingWakeStore::new);
        let upgrade_resume_store = root_dir.map(SandUpgradeResumeStore::new);
        Self {
            state: Mutex::new(RuntimeState {
                pipeline: SendPipelineState::default(),
                ledger: PromptAcceptanceLedger::new(root_dir),
                lifecycle: RunLifecycleState::default(),
                turn_dispatch: ProductionTurnDispatch::with_watchdog(
                    watchdog_ms,
                    watchdog_grace_ms,
                ),
                routed_turn_leases: HashMap::new(),
                completions: HashMap::new(),
                completion_order: VecDeque::new(),
            }),
            send_settled: Condvar::new(),
            turn_ready: Condvar::new(),
            pending_wake_store,
            upgrade_resume_store,
            upgrade_recreate_resume: UpgradeRecreateResume::default(),
            session_runtime: SessionRuntime::new(),
            replica_writer: HostReplicaWriter::new(),
            shared_group_remote_executor: Mutex::new(None),
            shared_group_room_entry_publisher: Mutex::new(None),
            roster_snapshot_seq: AtomicU64::new(0),
            agent_run_lifecycle_observer: Mutex::new(None),
        }
    }

    pub fn set_agent_run_lifecycle_observer(
        &self,
        observer: Option<AgentRunLifecycleObserver>,
    ) {
        *self
            .agent_run_lifecycle_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observer;
    }

    fn emit_agent_run_lifecycle(&self, event_type: &str, request_id: &str) {
        let observer = self
            .agent_run_lifecycle_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer(event_type, request_id);
        }
    }

    pub fn session_runtime(&self) -> &SessionRuntime {
        &self.session_runtime
    }

    pub fn bind_shared_group_remote_executor(
        &self,
        executor: Option<GroupMemberTurnExecutor>,
    ) {
        *self
            .shared_group_remote_executor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = executor;
    }

    pub fn shared_group_remote_executor(&self) -> Option<GroupMemberTurnExecutor> {
        self.shared_group_remote_executor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn bind_shared_group_room_entry_publisher(
        &self,
        publisher: Option<SharedGroupRoomEntryPublisher>,
    ) {
        *self
            .shared_group_room_entry_publisher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = publisher;
    }

    pub fn publish_shared_group_room_entry(
        &self,
        room_id: &str,
        entry: &Value,
    ) -> Result<bool, String> {
        let publisher = self
            .shared_group_room_entry_publisher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(publisher) = publisher else {
            return Ok(false);
        };
        publisher(room_id, entry)?;
        Ok(true)
    }

    pub fn switch_agent(
        &self,
        sessions: &std::sync::Arc<ProductionSessionWorkers>,
        agent_id: &str,
        now_ms: f64,
    ) -> Result<Vec<Value>, String> {
        let store = SandAgentSessionStore::new(std::sync::Arc::clone(sessions));
        let previous = store.read_active_agent_id();
        let entries = self
            .session_runtime
            .switch_agent(sessions, agent_id, now_ms)?;
        if let Some(previous) = previous
            .as_deref()
            .filter(|previous| *previous != agent_id)
        {
            let should_retire = {
                let state = self.lock_state();
                !state.lifecycle.is_running(previous)
            };
            if should_retire {
                let _ = self
                    .session_runtime
                    .retire_inactive_session(sessions, previous)?;
            }
        }
        Ok(entries)
    }

    pub fn retire_idle_live_session(
        &self,
        sessions: &Arc<ProductionSessionWorkers>,
        agent_id: &str,
    ) -> Result<bool, String> {
        if self.lock_state().lifecycle.is_running(agent_id) {
            return Ok(false);
        }
        self.session_runtime
            .retire_inactive_session(sessions, agent_id)
    }

    pub fn pending_wake_store(&self) -> Option<&SandPendingWakeStore> {
        self.pending_wake_store.as_ref()
    }

    pub fn upgrade_resume_store(&self) -> Option<&SandUpgradeResumeStore> {
        self.upgrade_resume_store.as_ref()
    }

    pub fn begin_live_subagent(&self, parent_agent_id: &str) {
        self.lock_state()
            .lifecycle
            .begin_live_subagent(parent_agent_id);
    }

    pub fn end_live_subagent(&self, parent_agent_id: &str) {
        self.lock_state()
            .lifecycle
            .end_live_subagent(parent_agent_id);
    }

    pub fn has_live_subagent(&self, parent_agent_id: &str) -> bool {
        self.lock_state()
            .lifecycle
            .has_live_subagent(parent_agent_id)
    }

    /// Frozen SandHost health reads this live set instead of projecting a
    /// permanently-idle default through the gateway.
    pub fn live_running_agent_ids(&self) -> Vec<String> {
        let mut ids = self
            .lock_state()
            .lifecycle
            .running_agent_ids()
            .into_iter()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    /// Pending durable wakes count as background work for Host health.
    pub fn has_carryable_pending_wake(&self) -> bool {
        self.pending_wake_store
            .as_ref()
            .is_some_and(|store| !store.list_pending().is_empty())
    }

    pub fn recreate_carry_pending_wakes(&self) -> Vec<DurablePendingWakeMarker> {
        if is_recreate_wake_carry_disabled() {
            return Vec::new();
        }
        self.pending_wake_store
            .as_ref()
            .map(|store| {
                store
                    .list_pending()
                    .into_iter()
                    .filter(|marker| {
                        matches!(marker.kind, PendingWakeKind::CloudAgent | PendingWakeKind::Shell)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn active_agent_id(
        &self,
        sessions: &Arc<ProductionSessionWorkers>,
    ) -> Option<String> {
        self.session_runtime.active_agent_id(sessions)
    }

    pub fn quiesce_for_upgrade(&self) -> UpgradeQuiesceSummary {
        let running = {
            let state = self.lock_state();
            state
                .lifecycle
                .running_agent_ids()
                .into_iter()
                .map(|agent_id| {
                    let source = state
                        .turn_dispatch
                        .active_source(&agent_id)
                        .unwrap_or("turn")
                        .to_string();
                    (agent_id, source)
                })
                .collect::<Vec<_>>()
        };
        if let Some(store) = self.upgrade_resume_store.as_ref() {
            let marked_at_ms = system_now_ms() as f64;
            for (agent_id, source) in &running {
                store.mark_pending(UpgradeResumeMarker {
                    agent_id: agent_id.clone(),
                    marked_at_ms,
                    source: Some(source.clone()),
                    automation_id: None,
                    automation_run_id: None,
                });
            }
        }
        self.upgrade_recreate_resume
            .quiesce_for_upgrade(running.len())
    }

    pub fn upgrade_resume_agent_ids(&self) -> Vec<String> {
        let mut agent_ids = self
            .upgrade_resume_store
            .as_ref()
            .map(|store| {
                store
                    .list_pending()
                    .into_iter()
                    .map(|marker| marker.agent_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        agent_ids.sort();
        agent_ids
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.upgrade_recreate_resume.is_quiescing_for_upgrade()
    }

    pub fn resume_after_recreate(&self) {
        self.upgrade_recreate_resume.resume_after_recreate();
    }

    pub fn clear_agent_durable_recovery(&self, agent_id: &str) {
        if let Some(store) = self.pending_wake_store.as_ref() {
            store.clear_agent(agent_id);
        }
        if let Some(store) = self.upgrade_resume_store.as_ref() {
            store.clear(agent_id);
        }
    }

    pub fn get_async_tasks(
        &self,
        agent_id: &str,
        live_tasks: &[AsyncTask],
    ) -> Vec<AsyncTask> {
        let markers = self
            .pending_wake_store
            .as_ref()
            .map(|store| {
                store
                    .list_pending()
                    .into_iter()
                    .filter(|marker| marker.agent_id == agent_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        merge_async_tasks(live_tasks, &markers)
    }

    /// Settle process-local Transcript runtime state before Session owners close.
    pub fn dispose(&self) {
        self.session_runtime.invalidate_deferred_activation();
        self.set_agent_run_lifecycle_observer(None);
        if let Ok(mut executor) = self.shared_group_remote_executor.lock() {
            *executor = None;
        }
        if let Ok(mut publisher) = self.shared_group_room_entry_publisher.lock() {
            *publisher = None;
        }

        {
            let mut state = self.lock_state();
            state.ledger.dispose();
            state.turn_dispatch.dispose();
            state.lifecycle = RunLifecycleState::default();
            state.routed_turn_leases.clear();
            state.completions.clear();
            state.completion_order.clear();
        }
        self.turn_ready.notify_all();
        self.send_settled.notify_all();
    }

    pub fn prompt_acceptance_status(
        &self,
        args: &Value,
    ) -> Result<Value, ProductionSendError> {
        let client_nonce = args
            .get("clientNonce")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProductionSendError::BadRequest(
                    "promptAcceptanceStatus requires clientNonce".into(),
                )
            })?;
        let account_slot = args
            .get("accountSlot")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(HOST_ACCOUNT_SLOT);
        let mut state = self.lock_state();
        Ok(match state.ledger.lookup(account_slot, client_nonce) {
            AcceptanceLookup::Found(record) => json!({
                "outcome": "found",
                "record": record,
            }),
            AcceptanceLookup::UnknownDurability => json!({
                "outcome": "unknown-durability",
            }),
            AcceptanceLookup::NotFound => json!({
                "outcome": "not-found",
            }),
        })
    }

    /// Admit a non-Cursor Coordinator-routed prompt into the authoritative
    /// Host/Session send pipeline without pretending provider execution happened
    /// on the Host lane. The Coordinator starts the independent Runner only
    /// after this durable admission returns.
    pub fn accept_routed_send<Persist, Persisted>(
        &self,
        args: &Value,
        persist_accepted: Persist,
    ) -> Result<RoutedSendAcceptance, ProductionSendError>
    where
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
    {
        self.accept_routed_send_with_queue_observers(
            args,
            persist_accepted,
            |_| false,
            |_| {},
            |_| {},
        )
    }

    pub fn accept_routed_send_with_queue_observers<
        Persist,
        Persisted,
        Watchdog,
        QueueAcceptedObserver,
        QueueDequeuedObserver,
    >(
        &self,
        args: &Value,
        persist_accepted: Persist,
        on_watchdog: Watchdog,
        on_queue_accepted: QueueAcceptedObserver,
        on_queue_dequeued: QueueDequeuedObserver,
    ) -> Result<RoutedSendAcceptance, ProductionSendError>
    where
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
        Watchdog: Fn(&WatchdogEvent) -> bool,
        QueueAcceptedObserver: Fn(&QueueAccepted),
        QueueDequeuedObserver: Fn(&QueueDequeued),
    {
        let input = parse_send_input(args)?;
        let request_source = optional_non_empty(args, "requestSource");
        let session_key = args
            .get("sessionKey")
            .or_else(|| args.get("sessionId"))
            .and_then(Value::as_str);
        let requires_selector = matches!(
            request_source,
            Some("automation" | "group-member" | "upgrade-resume" | "ack-redrive" | "subagent")
        );
        if (requires_selector || session_key.is_some()) && input.agent_id.is_some() {
            resolve_internal_send_route(
                input.agent_id.as_deref().unwrap_or_default(),
                session_key,
                args.get("chatType").and_then(Value::as_str),
                args.get("spawnedBy").and_then(Value::as_str),
                args.get("directMainFallback")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
            .map_err(|error| {
                ProductionSendError::BadRequest(format!(
                    "invalid trusted session route: {error:?}"
                ))
            })?;
        }
        if input.prompt.trim().is_empty() && input.attachment_paths.is_empty() {
            return Err(ProductionSendError::BadRequest(
                "routed send requires prompt or attachments".into(),
            ));
        }
        let stream_id = optional_non_empty(args, "streamId")
            .ok_or_else(|| {
                ProductionSendError::BadRequest(
                    "routed send requires streamId for Host queue ownership".into(),
                )
            })?
            .to_string();
        let nonce = optional_non_empty(args, "clientNonce").map(ToOwned::to_owned);
        let agent_id = input.agent_id.clone();
        let human_handoff_conversation_id =
            optional_non_empty(args, "humanHandoffConversationId").map(ToOwned::to_owned);
        let is_human_handoff =
            optional_non_empty(args, "requestSource") == Some("human-handoff");
        match (is_human_handoff, human_handoff_conversation_id.as_deref()) {
            (true, None) => {
                return Err(ProductionSendError::BadRequest(
                    "human-handoff routed send requires humanHandoffConversationId".into(),
                ));
            }
            (false, Some(_)) => {
                return Err(ProductionSendError::BadRequest(
                    "humanHandoffConversationId is only valid for requestSource=human-handoff".into(),
                ));
            }
            _ => {}
        }
        let (dispatch_lane, dispatch_source) = classify_send_dispatch(args)?;
        let dispatch_ack_token = optional_non_empty(args, "ackToken");
        let is_fork = optional_bool(args, "isFork")?.unwrap_or(false);
        let is_group_member_turn =
            optional_bool(args, "groupMemberTurn")?.unwrap_or(false);
        let accepted = json!({ "accepted": true, "routed": true });

        let mut state = self.lock_state();
        loop {
            let begin = {
                let RuntimeState {
                    pipeline, ledger, ..
                } = &mut *state;
                pipeline
                    .begin_send(ledger, &input, nonce.as_deref())
                    .map_err(map_acceptance_error)?
            };
            match begin {
                SendBegin::EmptyNoop => {
                    return Err(ProductionSendError::BadRequest(
                        "routed send requires prompt or attachments".into(),
                    ));
                }
                SendBegin::Coalesced { client_nonce } => {
                    state = self
                        .send_settled
                        .wait_while(state, |state| state.pipeline.is_in_flight(&client_nonce))
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    drop(state);
                    let context = persist_accepted(&accepted)?.into();
                    return Ok(RoutedSendAcceptance {
                        duplicate: true,
                        context,
                    });
                }
                SendBegin::DuplicateNoop { .. } => {
                    drop(state);
                    let context = persist_accepted(&accepted)?.into();
                    return Ok(RoutedSendAcceptance {
                        duplicate: true,
                        context,
                    });
                }
                SendBegin::Dispatch { .. } => {
                    let context = match persist_accepted(&accepted) {
                        Ok(persisted) => persisted.into(),
                        Err(error) => {
                            {
                                let RuntimeState {
                                    pipeline, ledger, ..
                                } = &mut *state;
                                pipeline.finish_send(ledger, nonce.as_deref(), false);
                            }
                            drop(state);
                            self.send_settled.notify_all();
                            return Err(error);
                        }
                    };

                    if let Some(client_nonce) = nonce.as_deref() {
                        let pending = {
                            let RuntimeState {
                                pipeline, ledger, ..
                            } = &mut *state;
                            pipeline.record_pending_acceptance(
                                ledger,
                                client_nonce,
                                SendEchoIdentity {
                                    agent_id: agent_id.clone().unwrap_or_default(),
                                    echo_entry_id: context.echo_entry_id.clone(),
                                },
                            )
                        };
                        if let Err(error) = pending {
                            let failure = map_acceptance_error(error);
                            {
                                let RuntimeState {
                                    pipeline, ledger, ..
                                } = &mut *state;
                                pipeline.finish_send(ledger, nonce.as_deref(), false);
                            }
                            drop(state);
                            self.send_settled.notify_all();
                            return Err(failure);
                        }
                    }

                    if let Some(agent_id) = agent_id.as_deref() {
                        let accepted_at_ms = system_now_ms();
                        state
                            .lifecycle
                            .begin_session_run(
                                agent_id,
                                accepted_at_ms,
                                is_group_member_turn,
                            );
                        let epoch = state.pipeline.next_turn_epoch(agent_id);
                        state.pipeline.register_recovery_turn(
                            agent_id,
                            epoch,
                            &context,
                            is_fork,
                        );
                        let (ticket, queue_accepted, queue_dequeued) =
                            match state.turn_dispatch.enqueue_turn_with_start(
                                agent_id,
                                nonce.as_deref(),
                                accepted_at_ms,
                                accepted_at_ms,
                                dispatch_lane,
                                dispatch_source,
                                dispatch_ack_token,
                            ) {
                                Ok(value) => value,
                                Err(error) => {
                                    let _ = state.lifecycle.end_session_run_with_kind(
                                        agent_id,
                                        system_now_ms(),
                                        is_group_member_turn,
                                    );
                                    let RuntimeState {
                                        pipeline, ledger, ..
                                    } = &mut *state;
                                    pipeline.finish_send(ledger, nonce.as_deref(), false);
                                    drop(state);
                                    self.turn_ready.notify_all();
                                    self.send_settled.notify_all();
                                    return Err(ProductionSendError::Internal(
                                        error.to_string(),
                                    ));
                                }
                            };

                        drop(state);
                        on_queue_accepted(&queue_accepted);
                        if let Some(event) = queue_dequeued.as_ref() {
                            on_queue_dequeued(event);
                        }
                        state = self.lock_state();

                        let generation = loop {
                            if let Some(generation) =
                                state.turn_dispatch.active_generation_for(&ticket)
                            {
                                break generation;
                            }
                            let now_ms = system_now_ms();
                            let wait_ms = state
                                .turn_dispatch
                                .watchdog_wait_ms(agent_id, now_ms)
                                .unwrap_or(1_000)
                                .max(1);
                            let (next_state, _) = self
                                .turn_ready
                                .wait_timeout(state, Duration::from_millis(wait_ms))
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state = next_state;
                            if let Some(tick) =
                                state.turn_dispatch.watchdog_tick(agent_id, system_now_ms())
                            {
                                let event = tick.event;
                                let started_next = tick.started_next;
                                let escaped = started_next.is_some();
                                drop(state);
                                let _ = on_watchdog(&event);
                                if let Some(event) = started_next.as_ref() {
                                    on_queue_dequeued(event);
                                }
                                if escaped {
                                    self.turn_ready.notify_all();
                                }
                                state = self.lock_state();
                            }
                        };

                        if state.routed_turn_leases.contains_key(&stream_id) {
                            let _ = state
                                .turn_dispatch
                                .settle_and_start_next(&ticket, generation, system_now_ms());
                            let _ = state.lifecycle.end_session_run_with_kind(
                                agent_id,
                                system_now_ms(),
                                is_group_member_turn,
                            );
                            let RuntimeState {
                                pipeline, ledger, ..
                            } = &mut *state;
                            pipeline.finish_send(ledger, nonce.as_deref(), false);
                            drop(state);
                            self.turn_ready.notify_all();
                            self.send_settled.notify_all();
                            return Err(ProductionSendError::Conflict(format!(
                                "routed stream already owns a Host turn lease: {stream_id}"
                            )));
                        }
                        state.routed_turn_leases.insert(
                            stream_id.clone(),
                            RoutedTurnLease {
                                agent_id: agent_id.to_string(),
                                ticket,
                                generation,
                                is_group_member_turn,
                                human_handoff_conversation_id: human_handoff_conversation_id.clone(),
                            },
                        );
                    }

                    {
                        let RuntimeState {
                            pipeline, ledger, ..
                        } = &mut *state;
                        pipeline.mark_send_accepted(ledger, nonce.as_deref());
                        pipeline.finish_send(ledger, nonce.as_deref(), true);
                    }
                    drop(state);
                    self.emit_agent_run_lifecycle("started", &stream_id);
                    self.send_settled.notify_all();
                    return Ok(RoutedSendAcceptance {
                        duplicate: false,
                        context,
                    });
                }
            }
        }
    }

    pub fn require_routed_turn_lease(
        &self,
        agent_id: &str,
        stream_id: &str,
    ) -> Result<(), ProductionSendError> {
        let state = self.lock_state();
        match state.routed_turn_leases.get(stream_id) {
            Some(lease) if lease.agent_id == agent_id => Ok(()),
            Some(_) => Err(ProductionSendError::Conflict(format!(
                "routed stream lease agent mismatch: {stream_id}"
            ))),
            None => Err(ProductionSendError::Rejected(format!(
                "routed provider stream has no admitted Host turn lease: {stream_id}"
            ))),
        }
    }

    pub fn settle_routed_turn(
        &self,
        agent_id: &str,
        stream_id: &str,
        now_ms: u64,
    ) -> Result<Option<RoutedTurnSettlement>, ProductionSendError> {
        let mut state = self.lock_state();
        let Some(lease) = state.routed_turn_leases.remove(stream_id) else {
            return Ok(None);
        };
        if lease.agent_id != agent_id {
            state
                .routed_turn_leases
                .insert(stream_id.to_string(), lease);
            return Err(ProductionSendError::Conflict(format!(
                "routed stream lease agent mismatch: {stream_id}"
            )));
        }
        let (settlement, next) = state
            .turn_dispatch
            .settle_and_start_next(&lease.ticket, lease.generation, now_ms);
        let watchdog = match settlement {
            RunSettlement::ZombieSettled { watchdog, .. } => Some(watchdog),
            RunSettlement::ActiveSettled { .. } | RunSettlement::Unknown => None,
        };
        let _ = state.lifecycle.end_session_run_with_kind(
            agent_id,
            now_ms,
            lease.is_group_member_turn,
        );
        drop(state);
        self.emit_agent_run_lifecycle("ended", stream_id);
        self.turn_ready.notify_all();
        Ok(Some(RoutedTurnSettlement { watchdog, next }))
    }

    pub fn has_routed_turn_lease(&self, agent_id: &str, stream_id: &str) -> bool {
        self.lock_state()
            .routed_turn_leases
            .get(stream_id)
            .is_some_and(|lease| lease.agent_id == agent_id)
    }

    pub fn routed_human_handoff_conversation_id(
        &self,
        agent_id: &str,
        stream_id: &str,
    ) -> Option<String> {
        self.lock_state()
            .routed_turn_leases
            .get(stream_id)
            .filter(|lease| lease.agent_id == agent_id)
            .and_then(|lease| lease.human_handoff_conversation_id.clone())
    }

    pub fn execute_send<Dispatch, Persist, Persisted>(
        &self,
        args: &Value,
        dispatch: Dispatch,
        persist_accepted: Persist,
    ) -> Result<Value, ProductionSendError>
    where
        Dispatch: FnOnce() -> Result<Value, ProductionSendError>,
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
    {
        self.execute_send_with_watchdog(args, dispatch, persist_accepted, |_| false)
    }

    pub fn execute_send_with_watchdog<Dispatch, Persist, Persisted, Watchdog>(
        &self,
        args: &Value,
        dispatch: Dispatch,
        persist_accepted: Persist,
        on_watchdog: Watchdog,
    ) -> Result<Value, ProductionSendError>
    where
        Dispatch: FnOnce() -> Result<Value, ProductionSendError>,
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
        Watchdog: Fn(&WatchdogEvent) -> bool,
    {
        self.execute_send_with_queue_observers(args, dispatch, persist_accepted, on_watchdog, |_| {}, |_| {})
    }

    pub fn execute_send_with_queue_observers<Dispatch, Persist, Persisted, Watchdog, QueueAcceptedObserver, QueueDequeuedObserver>(
        &self,
        args: &Value,
        dispatch: Dispatch,
        persist_accepted: Persist,
        on_watchdog: Watchdog,
        on_queue_accepted: QueueAcceptedObserver,
        on_queue_dequeued: QueueDequeuedObserver,
    ) -> Result<Value, ProductionSendError>
    where
        Dispatch: FnOnce() -> Result<Value, ProductionSendError>,
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
        Watchdog: Fn(&WatchdogEvent) -> bool,
        QueueAcceptedObserver: Fn(&QueueAccepted),
        QueueDequeuedObserver: Fn(&QueueDequeued),
    {
        self.execute_send_with_acceptance_observer(
            args,
            dispatch,
            persist_accepted,
            |_| {},
            on_watchdog,
            on_queue_accepted,
            on_queue_dequeued,
        )
    }

    pub fn execute_send_with_acceptance_observer<
        Dispatch,
        Persist,
        Persisted,
        AcceptanceObserver,
        Watchdog,
        QueueAcceptedObserver,
        QueueDequeuedObserver,
    >(
        &self,
        args: &Value,
        dispatch: Dispatch,
        persist_accepted: Persist,
        on_accepted: AcceptanceObserver,
        on_watchdog: Watchdog,
        on_queue_accepted: QueueAcceptedObserver,
        on_queue_dequeued: QueueDequeuedObserver,
    ) -> Result<Value, ProductionSendError>
    where
        Dispatch: FnOnce() -> Result<Value, ProductionSendError>,
        Persist: Fn(&Value) -> Result<Persisted, ProductionSendError>,
        Persisted: Into<PersistedSendContext>,
        AcceptanceObserver: Fn(&PersistedSendContext),
        Watchdog: Fn(&WatchdogEvent) -> bool,
        QueueAcceptedObserver: Fn(&QueueAccepted),
        QueueDequeuedObserver: Fn(&QueueDequeued),
    {
        let input = parse_send_input(args)?;
        let nonce = optional_non_empty(args, "clientNonce").map(ToOwned::to_owned);
        let agent_id = input.agent_id.clone();
        let (dispatch_lane, dispatch_source) = classify_send_dispatch(args)?;
        let dispatch_ack_token = optional_non_empty(args, "ackToken");
        let is_fork = optional_bool(args, "isFork")?.unwrap_or(false);
        let has_reply_context = optional_non_empty(args, "replyToId").is_some()
            || args.get("replyContext").is_some_and(|value| !value.is_null());
        let attachment_count = input.attachment_paths.len();
        let image_count = args
            .get("selectedImages")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let video_count = args
            .get("selectedVideos")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        let mut turn_ticket: Option<UserTurnTicket> = None;
        let mut turn_generation: Option<u64> = None;
        let mut turn_epoch: Option<u64> = None;
        let mut queue_accepted_event: Option<QueueAccepted> = None;
        let mut queue_dequeued_event: Option<QueueDequeued> = None;
        let mut persisted_echo_entry_id: Option<String> = None;
        let mut persisted_send_context = PersistedSendContext::default();

        let mut state = self.lock_state();
        loop {
            let begin = {
                let RuntimeState {
                    pipeline, ledger, ..
                } = &mut *state;
                pipeline
                    .begin_send(ledger, &input, nonce.as_deref())
                    .map_err(map_acceptance_error)?
            };
            match begin {
                SendBegin::EmptyNoop => return Ok(Value::Null),
                SendBegin::Coalesced { client_nonce } => {
                    state = self
                        .send_settled
                        .wait_while(state, |state| {
                            state.pipeline.is_in_flight(&client_nonce)
                        })
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(result) = state.completions.get(&client_nonce) {
                        return result.clone();
                    }
                }
                SendBegin::DuplicateNoop { record } => {
                    if let Some(result) = state.completions.get(&record.client_nonce) {
                        return result.clone();
                    }
                    return Ok(replay_record(&record));
                }
                SendBegin::Dispatch { .. } => {
                    // Frozen Grok durably appends the addressed user echo before
                    // the run enters the per-agent execution queue. This makes
                    // accepted user intent recoverable even if Host/Runner
                    // admission or provider execution fails afterwards.
                    let synthetic_acceptance = json!({ "accepted": true });
                    match persist_accepted(&synthetic_acceptance) {
                        Ok(persisted) => {
                            persisted_send_context = persisted.into();
                            persisted_echo_entry_id =
                                persisted_send_context.echo_entry_id.clone();
                        }
                        Err(error) => {
                            let failure = Err(error.clone());
                            {
                                let RuntimeState {
                                    pipeline, ledger, ..
                                } = &mut *state;
                                pipeline.finish_send(ledger, nonce.as_deref(), false);
                            }
                            if let Some(client_nonce) = nonce.as_deref() {
                                cache_completion(&mut state, client_nonce, failure.clone());
                            }
                            drop(state);
                            self.send_settled.notify_all();
                            return failure;
                        }
                    }

                    if let Some(client_nonce) = nonce.as_deref() {
                        let pending = {
                            let RuntimeState {
                                pipeline, ledger, ..
                            } = &mut *state;
                            pipeline.record_pending_acceptance(
                                ledger,
                                client_nonce,
                                SendEchoIdentity {
                                    agent_id: agent_id.clone().unwrap_or_default(),
                                    echo_entry_id: persisted_echo_entry_id.clone(),
                                },
                            )
                        };
                        if let Err(error) = pending {
                            let error = map_acceptance_error(error);
                            let failure = Err(error.clone());
                            {
                                let RuntimeState {
                                    pipeline, ledger, ..
                                } = &mut *state;
                                pipeline.finish_send(ledger, nonce.as_deref(), false);
                            }
                            cache_completion(&mut state, client_nonce, failure.clone());
                            drop(state);
                            self.send_settled.notify_all();
                            return failure;
                        }
                    }

                    if let Some(agent_id) = agent_id.as_deref() {
                        let accepted_at_ms = system_now_ms();
                        state
                            .lifecycle
                            .begin_session_run(agent_id, accepted_at_ms, false);
                        let epoch = state.pipeline.next_turn_epoch(agent_id);
                        state.pipeline.register_recovery_turn(
                            agent_id,
                            epoch,
                            &persisted_send_context,
                            is_fork,
                        );
                        turn_epoch = Some(epoch);
                        let (ticket, accepted, started) = state
                            .turn_dispatch
                            .enqueue_turn_with_start(
                                agent_id,
                                nonce.as_deref(),
                                accepted_at_ms,
                                accepted_at_ms,
                                dispatch_lane,
                                dispatch_source,
                                dispatch_ack_token,
                            )
                            .map_err(|error| ProductionSendError::Internal(error.to_string()))?;
                        queue_accepted_event = Some(accepted);
                        queue_dequeued_event = started;
                        turn_ticket = Some(ticket);
                    }
                    break;
                }
            }
        }
        // Durable acceptance side effects must run after the admission lock is
        // released and before provider dispatch. Roster projection reaches back
        // into Transcript runtime, so invoking it from persist_accepted would
        // self-deadlock this owner.
        drop(state);
        on_accepted(&persisted_send_context);
        if let Some(event) = queue_accepted_event.as_ref() {
            on_queue_accepted(event);
        }
        if let Some(event) = queue_dequeued_event.as_ref() {
            on_queue_dequeued(event);
        }
        state = self.lock_state();

        if let Some(ticket) = turn_ticket.as_ref() {
            loop {
                if let Some(generation) = state.turn_dispatch.active_generation_for(ticket) {
                    turn_generation = Some(generation);
                    break;
                }
                let now_ms = system_now_ms();
                let wait_ms = state
                    .turn_dispatch
                    .watchdog_wait_ms(&ticket.agent_id, now_ms)
                    .unwrap_or(1_000)
                    .max(1);
                let (next_state, _) = self
                    .turn_ready
                    .wait_timeout(state, Duration::from_millis(wait_ms))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state = next_state;

                if let Some(tick) = state
                    .turn_dispatch
                    .watchdog_tick(&ticket.agent_id, system_now_ms())
                {
                    let event = tick.event;
                    let started_next = tick.started_next;
                    let escaped = started_next.is_some();
                    drop(state);
                    let _ = on_watchdog(&event);
                    if let Some(event) = started_next.as_ref() {
                        on_queue_dequeued(event);
                    }
                    if escaped {
                        self.turn_ready.notify_all();
                    }
                    state = self.lock_state();
                }
            }
        }
        let suppress_stale_turn = match (agent_id.as_deref(), turn_epoch) {
            (Some(agent_id), Some(epoch)) => should_supersede_stale_turn(
                QueuedTurnRecoveryCheck {
                    epoch,
                    current_epoch: state.pipeline.current_turn_epoch(agent_id),
                    recovery_break_epoch: state.pipeline.recovery_break_epoch(agent_id),
                    prompt: &input.prompt,
                    context: &persisted_send_context,
                    latest_recovery: state.pipeline.latest_recovery_send(agent_id),
                    is_fork,
                    has_reply_context,
                    attachment_count,
                    image_count,
                    video_count,
                },
            ),
            _ => false,
        };
        drop(state);

        let mut result = if suppress_stale_turn {
            Ok(json!({
                "accepted": true,
                "superseded": true,
                "echoEntryId": persisted_send_context.echo_entry_id,
            }))
        } else {
            dispatch()
        };

        let mut state = self.lock_state();
        let accepted = result
            .as_ref()
            .ok()
            .is_some_and(|value| value.get("accepted").and_then(Value::as_bool) == Some(true));

        if accepted {
            if let Some(client_nonce) = nonce.as_deref() {
                let RuntimeState {
                    pipeline, ledger, ..
                } = &mut *state;
                pipeline.mark_send_accepted(ledger, Some(client_nonce));
            }
            if let Some(value) = result.as_ref().ok() {
                let operation_id = value
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                if let (Some(agent_id), Some(operation_id)) =
                    (agent_id.as_deref(), operation_id.as_deref())
                {
                    state.lifecycle.record_request_id(agent_id, operation_id);
                    state.lifecycle.track_turn_request_id(agent_id, operation_id);
                }
            }
        }

        let succeeded = result
            .as_ref()
            .ok()
            .is_some_and(|value| value.get("accepted").and_then(Value::as_bool) == Some(true));
        {
            let RuntimeState {
                pipeline, ledger, ..
            } = &mut *state;
            pipeline.finish_send(ledger, nonce.as_deref(), succeeded);
        }
        let settled_at_ms = system_now_ms();
        let mut terminal_watchdog_event = None;
        let mut settled_dequeued_event = None;
        if let (Some(ticket), Some(generation)) =
            (turn_ticket.as_ref(), turn_generation)
        {
            let (settlement, next) = state
                .turn_dispatch
                .settle_and_start_next(ticket, generation, settled_at_ms);
            settled_dequeued_event = next;
            if let RunSettlement::ZombieSettled { watchdog, .. } = settlement {
                terminal_watchdog_event = Some(watchdog);
            }
        }
        if let Some(agent_id) = agent_id.as_deref() {
            let _ = state.lifecycle.end_session_run(agent_id, settled_at_ms);
        }
        if let Some(client_nonce) = nonce.as_deref() {
            cache_completion(&mut state, client_nonce, result.clone());
        }
        drop(state);
        if let Some(event) = terminal_watchdog_event.as_ref() {
            let _ = on_watchdog(event);
        }
        if let Some(event) = settled_dequeued_event.as_ref() {
            on_queue_dequeued(event);
        }
        self.turn_ready.notify_all();
        self.send_settled.notify_all();
        result
    }

    pub fn begin_provider_run(&self, agent_id: &str) {
        self.begin_provider_run_with_kind(agent_id, false);
    }

    pub fn begin_provider_run_with_kind(&self, agent_id: &str, is_group_member_turn: bool) {
        self.lock_state()
            .lifecycle
            .begin_provider_run_with_kind(agent_id, is_group_member_turn);
    }

    pub fn end_provider_run(&self, agent_id: &str) {
        self.end_provider_run_with_kind(agent_id, false);
    }

    pub fn end_provider_run_with_kind(&self, agent_id: &str, is_group_member_turn: bool) {
        self.lock_state()
            .lifecycle
            .end_provider_run_with_kind(agent_id, is_group_member_turn);
    }

    pub fn track_runner_activity_update(
        &self,
        agent_id: &str,
        update: &ActivityUpdate,
        now_ms: u64,
    ) {
        let mut state = self.lock_state();
        state.lifecycle.track_activity_from_update(agent_id, update, now_ms);
        state.lifecycle.track_composing_from_update(agent_id, update);
        state.lifecycle.track_retrying_from_update(agent_id, update);
    }

    pub fn set_active_remote_member(
        &self,
        agent_id: &str,
        remote_member_id: Option<&str>,
    ) {
        self.lock_state()
            .lifecycle
            .set_active_remote_member(agent_id, remote_member_id);
    }

    pub fn track_box_request_entry(
        &self,
        agent_id: &str,
        entry: &Value,
    ) -> BoxRequestTrackDecision {
        self.lock_state()
            .pipeline
            .track_box_request_entry(agent_id, entry)
    }

    pub fn append_generated_send_message_entry(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
        session_key: &str,
        message: &Value,
        timestamp_ms: u64,
        reply_thread_target: Option<&str>,
        is_fork: bool,
    ) -> Result<Value, String> {
        let entries = sessions.read_agent_transcript_entries(agent_id)?;
        let mut entry = self
            .lock_state()
            .pipeline
            .prepare_generated_send_message_entry(
                agent_id,
                &entries,
                message,
                reply_thread_target,
                is_fork,
                timestamp_ms,
            );
        entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "generated send pipeline did not produce an entry id".to_string())?;
        entry["sessionKey"] = Value::String(session_key.trim().to_string());
        sessions.append_agent_transcript_entries_if_session_admitted(
            agent_id,
            session_key,
            std::slice::from_ref(&entry),
        )?;
        Ok(entry)
    }

    /// Append a generated send for an explicitly unbound compatibility
    /// producer. The generic session owner still applies its deletion and
    /// archive fence; no session identity is invented for the entry.
    pub fn append_generated_send_message_entry_compatibility(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
        message: &Value,
        timestamp_ms: u64,
        reply_thread_target: Option<&str>,
        is_fork: bool,
    ) -> Result<Value, String> {
        let entries = sessions.read_agent_transcript_entries(agent_id)?;
        let entry = self
            .lock_state()
            .pipeline
            .prepare_generated_send_message_entry(
                agent_id,
                &entries,
                message,
                reply_thread_target,
                is_fork,
                timestamp_ms,
            );
        entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "generated send pipeline did not produce an entry id".to_string())?;
        sessions.append_agent_transcript_entries(
            agent_id,
            std::slice::from_ref(&entry),
        )?;
        Ok(entry)
    }

    /// Append a generated send and release its durable media fence in the same
    /// session-owner transaction.
    pub fn append_generated_send_message_entry_with_media_custody(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
        session_key: &str,
        message: &Value,
        timestamp_ms: u64,
        reply_thread_target: Option<&str>,
        is_fork: bool,
        expected_lifecycle_revision: &str,
        media_work_id: &str,
    ) -> Result<Value, String> {
        let entries = sessions.read_agent_transcript_entries(agent_id)?;
        let mut entry = self
            .lock_state()
            .pipeline
            .prepare_generated_send_message_entry(
                agent_id,
                &entries,
                message,
                reply_thread_target,
                is_fork,
                timestamp_ms,
            );
        entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "generated send pipeline did not produce an entry id".to_string())?;
        entry["sessionKey"] = Value::String(session_key.trim().to_string());
        sessions.append_agent_transcript_entries_and_release_media(
            agent_id,
            session_key,
            std::slice::from_ref(&entry),
            expected_lifecycle_revision,
            media_work_id,
        )?;
        Ok(entry)
    }

    pub fn append_generated_send_message(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
        session_key: &str,
        message: &Value,
        timestamp_ms: u64,
        reply_thread_target: Option<&str>,
        is_fork: bool,
    ) -> Result<String, String> {
        self.append_generated_send_message_entry(
            sessions,
            agent_id,
            session_key,
            message,
            timestamp_ms,
            reply_thread_target,
            is_fork,
        )?
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "generated send pipeline did not produce an entry id".to_string())
    }

    pub fn resolve_box_request_tracking(&self, request_id: &str) -> bool {
        self.lock_state()
            .pipeline
            .resolve_box_request_tracking(request_id)
    }

    pub fn active_box_request(&self) -> Option<ActiveBoxRequest> {
        self.lock_state()
            .pipeline
            .active_box_request()
            .cloned()
    }

    fn durable_subagent_parent_ids(&self) -> HashSet<String> {
        self.pending_wake_store
            .as_ref()
            .map(|store| {
                store
                    .list_pending()
                    .into_iter()
                    .filter(|marker| marker.kind == PendingWakeKind::Subagent)
                    .map(|marker| marker.agent_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn decorate_agent_summaries(&self, value: &mut Value) {
        let durable_subagent_parents = self.durable_subagent_parent_ids();
        let snapshot_epoch = self.replica_writer.process_epoch().to_string();
        let snapshot_seq = self
            .roster_snapshot_seq
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1);
        let state = self.lock_state();
        let Some(rows) = value.as_array_mut() else {
            return;
        };
        for row in rows {
            let Some(object) = row.as_object_mut() else {
                continue;
            };
            let Some(agent_id) = object.get("id").and_then(Value::as_str).map(str::to_string) else {
                continue;
            };
            let active_remote_member_id = state
                .lifecycle
                .active_remote_member_id(&agent_id)
                .map(ToOwned::to_owned);
            let projected = state.lifecycle.project_run_state(
                &agent_id,
                durable_subagent_parents.contains(&agent_id),
                active_remote_member_id.as_deref(),
            );
            object.insert("isRunning".into(), Value::Bool(projected.is_running));
            object.insert(
                "isRunningTurn".into(),
                Value::Bool(projected.is_running_turn),
            );
            object.insert(
                "isComposingMessage".into(),
                Value::Bool(projected.is_composing_message),
            );
            object.insert(
                "isRetrying".into(),
                Value::Bool(projected.is_retrying),
            );
            object.insert(
                "currentActivity".into(),
                projected
                    .current_activity
                    .as_ref()
                    .and_then(|activity| serde_json::to_value(activity).ok())
                    .unwrap_or(Value::Null),
            );
            if let Some(active_remote_member_id) = projected.active_remote_member_id {
                object.insert(
                    "activeRemoteMemberId".into(),
                    Value::String(active_remote_member_id),
                );
            } else {
                object.insert("activeRemoteMemberId".into(), Value::Null);
            }
            object.insert(
                "snapshotEpoch".into(),
                Value::String(snapshot_epoch.clone()),
            );
            object.insert(
                "snapshotSeq".into(),
                Value::Number(snapshot_seq.into()),
            );
        }
    }

    pub fn roster_process_epoch(&self) -> &str {
        self.replica_writer.process_epoch()
    }

    pub fn next_replica_stamp(
        &self,
        replica_key: &str,
    ) -> super::replica_writer::ReplicaStamp {
        self.replica_writer.next_stamp(replica_key)
    }

    pub fn current_turn_epoch(&self, agent_id: &str) -> u64 {
        self.lock_state().pipeline.current_turn_epoch(agent_id)
    }

    pub fn next_turn_epoch(&self, agent_id: &str) -> u64 {
        self.lock_state().pipeline.next_turn_epoch(agent_id)
    }

    pub fn in_flight_run_count(&self, agent_id: &str) -> u64 {
        self.lock_state().lifecycle.in_flight_count(agent_id)
    }

    pub fn queued_turn_count(&self, agent_id: &str) -> usize {
        self.lock_state()
            .turn_dispatch
            .queued_task_ids(agent_id)
            .len()
    }

    pub fn active_turn_lane(&self, agent_id: &str) -> Option<RunLane> {
        self.lock_state().turn_dispatch.active_lane(agent_id)
    }

    pub fn active_turn_source(&self, agent_id: &str) -> Option<String> {
        self.lock_state()
            .turn_dispatch
            .active_source(agent_id)
            .map(ToOwned::to_owned)
    }

    pub fn is_turn_dispatch_idle(&self, agent_id: &str) -> bool {
        self.lock_state().turn_dispatch.is_idle(agent_id)
    }

    pub fn is_agent_running(&self, agent_id: &str) -> bool {
        self.lock_state().lifecycle.is_running(agent_id)
    }

    pub fn settle_turn_usage(
        &self,
        agent_id: &str,
        source: &str,
    ) -> super::run_lifecycle::TurnUsageReport {
        self.lock_state().lifecycle.settle_turn_usage(agent_id, source)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn cache_completion(
    state: &mut RuntimeState,
    client_nonce: &str,
    result: Result<Value, ProductionSendError>,
) {
    if !state.completions.contains_key(client_nonce) {
        state.completion_order.push_back(client_nonce.to_string());
    }
    state.completions.insert(client_nonce.to_string(), result);
    while state.completion_order.len() > COMPLETION_CACHE_MAX {
        if let Some(oldest) = state.completion_order.pop_front() {
            state.completions.remove(&oldest);
        }
    }
}

fn replay_record(record: &AcceptanceRecord) -> Value {
    let operation_id = record
        .echo_entry_id
        .as_deref()
        .and_then(|entry_id| entry_id.strip_suffix(":user"));
    json!({
        "accepted": record.status == AcceptanceStatus::Accepted,
        "duplicate": true,
        "acceptanceStatus": match record.status {
            AcceptanceStatus::Accepted => "accepted",
            AcceptanceStatus::Rejected => "rejected",
            AcceptanceStatus::Pending => "pending",
        },
        "agentId": record.agent_id,
        "clientNonce": record.client_nonce,
        "operationId": operation_id,
    })
}

fn parse_send_input(args: &Value) -> Result<SendInput, ProductionSendError> {
    let agent_id = optional_non_empty(args, "agentId")
        .or_else(|| optional_non_empty(args, "id"))
        .map(ToOwned::to_owned);
    let prompt = args
        .get("prompt")
        .or_else(|| args.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let attachment_paths = string_array(args, "attachmentPaths")?;
    let attachment_names = string_array(args, "attachmentNames")?;
    if prompt.trim().is_empty() && attachment_paths.is_empty() {
        return Ok(SendInput {
            agent_id,
            prompt,
            rich_text: optional_string(args, "richText")?,
            reply_to_id: optional_string(args, "replyToId")?,
            is_fork: optional_bool(args, "isFork")?.unwrap_or(false),
            attachment_paths,
            attachment_names,
        });
    }
    Ok(SendInput {
        agent_id,
        prompt,
        rich_text: optional_string(args, "richText")?,
        reply_to_id: optional_string(args, "replyToId")?,
        is_fork: optional_bool(args, "isFork")?.unwrap_or(false),
        attachment_paths,
        attachment_names,
    })
}

fn string_array(args: &Value, field: &str) -> Result<Vec<String>, ProductionSendError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| {
                        ProductionSendError::BadRequest(format!(
                            "{field} must contain only strings"
                        ))
                    })
            })
            .collect(),
        Some(_) => Err(ProductionSendError::BadRequest(format!(
            "{field} must be an array"
        ))),
    }
}

fn optional_string(args: &Value, field: &str) -> Result<Option<String>, ProductionSendError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ProductionSendError::BadRequest(format!("invalid {field}"))),
    }
}

fn optional_bool(args: &Value, field: &str) -> Result<Option<bool>, ProductionSendError> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ProductionSendError::BadRequest(format!("invalid {field}"))),
    }
}

fn optional_non_empty<'a>(args: &'a Value, field: &str) -> Option<&'a str> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn map_acceptance_error(error: PromptAcceptanceError) -> ProductionSendError {
    match error {
        PromptAcceptanceError::DigestMismatch { .. } => {
            ProductionSendError::Conflict(error.to_string())
        }
        PromptAcceptanceError::Rejected { .. } => {
            ProductionSendError::Rejected(error.to_string())
        }
        PromptAcceptanceError::InvalidPendingIdentity => {
            ProductionSendError::Internal(error.to_string())
        }
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
