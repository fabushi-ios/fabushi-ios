use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::extensions::settings::settings_service::{
    SandLocalToolPermission, SettingsService,
};

use super::local_tool_permission_resolution::{
    LocalToolPermissionAskStore, PendingLocalToolPermissionRequest,
    SandLocalToolResolution,
};

pub const SAND_LOCAL_TOOL_SETTLED_ID_MEMORY: usize = 64;
pub const SAND_LOCAL_TOOL_REFUSAL_DIRECTION_WINDOW: u64 = 64;
pub const SAND_LOCAL_TOOL_REFUSED_ACTION_MEMORY_PER_AGENT: usize = 512;
pub const SAND_LOCAL_TOOL_FORGOTTEN_AGENT_MEMORY: usize = 256;
pub const SAND_LOCAL_TOOL_TARGET_MAX_CHARS: usize = 10_000;
pub const SAND_LOCAL_TOOL_ASK_TTL_MS: u64 = 10 * 60 * 1_000;

pub const SAND_LOCAL_TOOLS_DISABLED_MESSAGE: &str =
    "Local tools are turned off. The user has set local tool access to \"Never\", so ExternalShell, ExternalRead, AwaitExternalShell, CopyToBox, and CopyFromBox cannot run on their computer. Do not retry them while this setting remains \"Never\". Use your own computer instead (Shell, Read, AwaitShell), or ask the user to change the setting in Settings → Agent → Execution on Local Computer. If they change it away from \"Never\", you may try again.";
pub const SAND_LOCAL_TOOLS_DENIED_MESSAGE: &str =
    "The user declined this action on their computer. Do not retry it. Do something else, use your own computer instead (Shell, Read, AwaitShell), or ask them what they would prefer.";
pub const SAND_LOCAL_TOOLS_ASK_EXPIRED_MESSAGE: &str =
    "The request to run this on the user's computer went unanswered, so nothing ran. Use your own computer instead (Shell, Read, AwaitShell), or tell the user you are waiting on their approval.";
pub const SAND_LOCAL_TOOLS_ASK_UNAVAILABLE_MESSAGE: &str =
    "Using the user's computer needs their permission, and this conversation has nowhere to ask for it. Use your own computer instead (Shell, Read, AwaitShell), or do this from a direct chat with the user.";
pub const SAND_LOCAL_TOOLS_UNAPPROVED_MESSAGE: &str =
    "That action was not approved on the user's computer, so nothing ran. Ask the user to approve it (or to set Settings → Agent → Execution on Local Computer to \"Always allow\"), and use your own computer (Shell, Read, AwaitShell) in the meantime.";
pub const SAND_LOCAL_TOOLS_TARGET_TOO_LARGE_MESSAGE: &str =
    "That action is too long to show the user for approval, so it was not run. Split it into smaller steps, or use your own computer instead (Shell, Read, AwaitShell).";
pub const SAND_LOCAL_TOOLS_ABANDONED_MESSAGE: &str =
    "The user was already asked about this exact action on their computer and did not approve it, so it will not run and will not be asked again for this task — a later permission change does not authorize it. Do not retry it. If it still needs to happen, say so in chat and let the user ask for it, and use your own computer in the meantime (Shell, Read, AwaitShell).";
pub const SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE: &str =
    "This task's earlier requests to use the user's computer were not approved and the user has since moved on, so nothing from this task will run there. Do not retry. If it still needs to happen, say so in chat and let the user ask for it, and use your own computer in the meantime (Shell, Read, AwaitShell).";
pub const SAND_LOCAL_TOOLS_PREPARATORY_MESSAGE: &str =
    "That preparatory access to the user's computer was skipped: the user is asked about the action itself, not the work leading up to it. Continue without it.";
pub const SAND_LOCAL_TOOLS_ASK_CANCELLED_MESSAGE: &str =
    "The request to use the user's computer was cancelled before the user answered.";
pub const SAND_NO_LOCAL_MACHINE_MESSAGE: &str =
    "Your local machine isn't connected right now (the Fabushi desktop app must be open and online to run commands on it). Try again once it's reachable.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalToolScope {
    pub agent_id: String,
    pub session_key: Option<String>,
    pub tool_call_id: Option<String>,
    pub action: Option<String>,
    pub direction_epoch: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalToolRequest {
    pub action: String,
    pub target: String,
    pub resource_path: Option<String>,
    pub attach_to_resource_path: Option<String>,
    pub outlives_scope: bool,
    pub description: Option<String>,
}

impl SandLocalToolRequest {
    pub fn simple(action: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            target: target.into(),
            resource_path: None,
            attach_to_resource_path: None,
            outlives_scope: false,
            description: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalToolDecision {
    pub allowed: bool,
    pub reason: Option<String>,
    pub approval_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandLocalToolRequestStatus {
    Pending,
    Allowed,
    Denied,
    Always,
    Never,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalToolAskRequest {
    pub id: String,
    pub agent_id: String,
    pub session_key: Option<String>,
    pub action: String,
    pub target: String,
    pub status: SandLocalToolRequestStatus,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
struct StoredApproval {
    id: String,
    agent_id: String,
    tool_call_id: String,
    action: String,
    target: String,
    resource_path: Option<String>,
    outlives_scope: bool,
}

struct PendingAsk {
    request: Mutex<SandLocalToolAskRequest>,
    decision: Mutex<Option<SandLocalToolDecision>>,
    wake: Condvar,
    tool_call_id: String,
    resource_path: Option<String>,
    outlives_scope: bool,
    direction_epoch: u64,
    waiter_count: Mutex<usize>,
}

#[derive(Debug, Clone)]
struct RefusedAction {
    agent_id: String,
    direction_epoch: u64,
    sequence: u64,
}

#[derive(Default)]
struct ControllerState {
    pending_by_key: HashMap<String, Arc<PendingAsk>>,
    pending_by_id: HashMap<String, String>,
    approvals_by_id: HashMap<String, StoredApproval>,
    direction_epochs: HashMap<String, u64>,
    refused_actions: HashMap<String, RefusedAction>,
    saturated_directions: HashMap<String, u64>,
    always_granted_at_epoch: HashMap<String, u64>,
    forgotten_agents: HashSet<String>,
    forgotten_order: VecDeque<String>,
    noted_permission: Option<SandLocalToolPermission>,
    next_refusal_sequence: u64,
    settled_ids: HashSet<String>,
    settled_order: VecDeque<String>,
}

type AgentPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type EventSink = Arc<dyn Fn(SandLocalToolAskRequest) + Send + Sync>;
type ControllerEventListener = Arc<dyn Fn(SandLocalToolControllerEvent) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandLocalToolControllerEventKind {
    Created,
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalToolControllerEvent {
    pub kind: SandLocalToolControllerEventKind,
    pub request: SandLocalToolAskRequest,
}

pub struct SandLocalToolControllerSubscription {
    id: u64,
    listeners: Arc<Mutex<HashMap<u64, ControllerEventListener>>>,
    active: bool,
}

impl SandLocalToolControllerSubscription {
    pub fn unsubscribe(mut self) {
        if self.active {
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.remove(&self.id);
            }
            self.active = false;
        }
    }
}

impl Drop for SandLocalToolControllerSubscription {
    fn drop(&mut self) {
        if self.active {
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.remove(&self.id);
            }
            self.active = false;
        }
    }
}

pub struct SandLocalToolPermissionController {
    settings: Arc<SettingsService>,
    state: Mutex<ControllerState>,
    can_ask: Mutex<AgentPredicate>,
    has_live_computer: Mutex<AgentPredicate>,
    event_sink: Mutex<Option<EventSink>>,
    event_listeners: Arc<Mutex<HashMap<u64, ControllerEventListener>>>,
    next_listener_id: AtomicU64,
    approval_retired_sink: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
    ask_ttl_ms: u64,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    random_id: Arc<dyn Fn() -> String + Send + Sync>,
}

impl SandLocalToolPermissionController {
    pub fn production(settings: Arc<SettingsService>) -> Self {
        Self::with_options(
            settings,
            SAND_LOCAL_TOOL_ASK_TTL_MS,
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            }),
            Arc::new(|| {
                format!(
                    "{}{}",
                    Uuid::new_v4().simple(),
                    Uuid::new_v4().simple()
                )
            }),
        )
    }

    pub fn with_options(
        settings: Arc<SettingsService>,
        ask_ttl_ms: u64,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
        random_id: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            settings,
            state: Mutex::new(ControllerState::default()),
            can_ask: Mutex::new(Arc::new(|_| false)),
            has_live_computer: Mutex::new(Arc::new(|_| false)),
            event_sink: Mutex::new(None),
            event_listeners: Arc::new(Mutex::new(HashMap::new())),
            next_listener_id: AtomicU64::new(1),
            approval_retired_sink: Mutex::new(None),
            ask_ttl_ms,
            now_ms,
            random_id,
        }
    }

    pub fn bind_ask_surfaces(&self, provider: AgentPredicate) {
        *self.can_ask.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }

    pub fn bind_live_computer_check(&self, provider: AgentPredicate) {
        *self
            .has_live_computer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }

    pub fn bind_event_sink(&self, sink: Option<EventSink>) {
        *self.event_sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = sink;
    }

    pub fn bind_approval_retired_sink(
        &self,
        sink: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    ) {
        *self
            .approval_retired_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = sink;
    }

    pub fn subscribe(
        &self,
        listener: ControllerEventListener,
    ) -> SandLocalToolControllerSubscription {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.event_listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, listener);
        SandLocalToolControllerSubscription {
            id,
            listeners: Arc::clone(&self.event_listeners),
            active: true,
        }
    }

    pub fn permission(&self) -> SandLocalToolPermission {
        self.settings.get_local_tool_permission()
    }

    pub fn blocked_reason(&self) -> Option<String> {
        (self.permission() == SandLocalToolPermission::Never)
            .then(|| SAND_LOCAL_TOOLS_DISABLED_MESSAGE.to_string())
    }

    pub fn requires_approval(&self) -> bool {
        self.permission() == SandLocalToolPermission::Ask
    }

    pub fn direction_epoch(&self, agent_id: &str) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .direction_epochs
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn begin_turn(&self, agent_id: &str) {
        let (pending, retired) = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.forgotten_agents.remove(agent_id);
            state.forgotten_order.retain(|value| value != agent_id);
            let epoch = state.direction_epochs.entry(agent_id.to_string()).or_insert(0);
            *epoch = epoch.saturating_add(1);
            let retired = state
                .approvals_by_id
                .iter()
                .filter_map(|(id, approval)| (approval.agent_id == agent_id).then_some(id.clone()))
                .collect::<Vec<_>>();
            for id in &retired {
                state.approvals_by_id.remove(id);
            }
            let pending = state
                .pending_by_key
                .values()
                .filter(|pending| {
                    pending
                        .request
                        .lock()
                        .map(|request| request.agent_id == agent_id)
                        .unwrap_or(false)
                })
                .cloned()
                .collect::<Vec<_>>();
            (pending, retired)
        };
        for id in retired {
            self.emit_approval_retired(&id);
        }
        for pending in pending {
            let id = pending
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .id
                .clone();
            self.settle_pending(
                &id,
                SandLocalToolRequestStatus::Expired,
                SandLocalToolDecision {
                    allowed: false,
                    reason: Some(SAND_LOCAL_TOOLS_ASK_EXPIRED_MESSAGE.to_string()),
                    approval_id: None,
                },
                true,
            );
        }
    }

    pub fn forget_agent(&self, agent_id: &str) {
        self.begin_turn(agent_id);
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.direction_epochs.remove(agent_id);
        state.saturated_directions.remove(agent_id);
        state.always_granted_at_epoch.remove(agent_id);
        state.refused_actions.retain(|_, refused| refused.agent_id != agent_id);
        if state.forgotten_agents.insert(agent_id.to_string()) {
            state.forgotten_order.push_back(agent_id.to_string());
        }
        while state.forgotten_order.len() > SAND_LOCAL_TOOL_FORGOTTEN_AGENT_MEMORY {
            if let Some(oldest) = state.forgotten_order.pop_front() {
                state.forgotten_agents.remove(&oldest);
            }
        }
    }

    pub fn complete_scope(&self, scope: Option<&SandLocalToolScope>) {
        let Some(scope) = scope else { return };
        let Some(tool_call_id) = scope.tool_call_id.as_deref() else { return };
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let ids = state
                .approvals_by_id
                .iter()
                .filter_map(|(id, approval)| {
                    (approval.agent_id == scope.agent_id
                        && approval.tool_call_id == tool_call_id
                        && !approval.outlives_scope)
                        .then_some(id.clone())
                })
                .collect::<Vec<_>>();
            for id in &ids {
                state.approvals_by_id.remove(id);
            }
            ids
        };
        for id in retired {
            self.emit_approval_retired(&id);
        }
    }

    pub fn authorize(
        &self,
        scope: Option<&SandLocalToolScope>,
        request: &SandLocalToolRequest,
    ) -> SandLocalToolDecision {
        self.authorize_with_cancel(scope, request, None)
    }

    pub fn authorize_with_cancel(
        &self,
        scope: Option<&SandLocalToolScope>,
        request: &SandLocalToolRequest,
        cancel: Option<&AtomicBool>,
    ) -> SandLocalToolDecision {
        let permission = self.permission();
        if permission == SandLocalToolPermission::Never {
            return denied(SAND_LOCAL_TOOLS_DISABLED_MESSAGE);
        }

        if let Some(scope) = scope {
            if let Some(reason) = self.refusal_for(scope, request) {
                return denied(reason);
            }
        }

        if permission == SandLocalToolPermission::Always
            && !scope.is_some_and(|scope| self.predates_standing_grant(scope))
        {
            return allowed(None);
        }

        let Some(scope) = scope else {
            return denied(SAND_LOCAL_TOOLS_UNAPPROVED_MESSAGE);
        };

        if let Some(decision) = self.covering_approval(scope, request) {
            return decision;
        }

        let Some(tool_call_id) = scope.tool_call_id.as_deref() else {
            return denied(SAND_LOCAL_TOOLS_UNAPPROVED_MESSAGE);
        };

        let can_ask = self
            .can_ask
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if !can_ask(&scope.agent_id) {
            return denied(SAND_LOCAL_TOOLS_ASK_UNAVAILABLE_MESSAGE);
        }
        let has_live = self
            .has_live_computer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if !has_live(&scope.agent_id) {
            return denied(SAND_NO_LOCAL_MACHINE_MESSAGE);
        }

        let scope_approved = scope.action.as_deref().is_some_and(|scope_action| {
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .approvals_by_id
                .values()
                .any(|approval| {
                    approval.agent_id == scope.agent_id
                        && approval.tool_call_id == tool_call_id
                        && approval.action == scope_action
                })
        });
        if let Some(scope_action) = scope.action.as_deref() {
            if scope_action != request.action && !scope_approved {
                return denied(SAND_LOCAL_TOOLS_PREPARATORY_MESSAGE);
            }
        }

        if cancel.is_some_and(|signal| signal.load(Ordering::Acquire)) {
            return denied(SAND_LOCAL_TOOLS_ASK_CANCELLED_MESSAGE);
        }

        if request.target.encode_utf16().count() > SAND_LOCAL_TOOL_TARGET_MAX_CHARS {
            return denied(SAND_LOCAL_TOOLS_TARGET_TOO_LARGE_MESSAGE);
        }

        let key = format!(
            "{}\0{}\0{}\0{}",
            scope.agent_id, tool_call_id, request.action, request.target
        );
        // Compute the epoch before taking the controller state lock below. Calling
        // scope_epoch() while that lock is held would try to acquire the same
        // non-reentrant mutex again and deadlock the first permission request.
        let direction_epoch = self.scope_epoch(scope);
        let (pending, created) = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(existing) = state.pending_by_key.get(&key) {
                (Arc::clone(existing), None)
            } else {
                let now = (self.now_ms)();
                let id = (self.random_id)();
                let pending = Arc::new(PendingAsk {
                    request: Mutex::new(SandLocalToolAskRequest {
                        id: id.clone(),
                        agent_id: scope.agent_id.clone(),
                        session_key: scope.session_key.clone(),
                        action: request.action.clone(),
                        target: request.target.clone(),
                        status: SandLocalToolRequestStatus::Pending,
                        created_at_ms: now,
                        expires_at_ms: now.saturating_add(self.ask_ttl_ms),
                        description: request
                            .description
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string),
                    }),
                    decision: Mutex::new(None),
                    wake: Condvar::new(),
                    tool_call_id: tool_call_id.to_string(),
                    resource_path: normalize_resource_path(request.resource_path.as_deref()),
                    outlives_scope: request.outlives_scope,
                    direction_epoch,
                    waiter_count: Mutex::new(0),
                });
                let created = pending
                    .request
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                state.pending_by_id.insert(id, key.clone());
                state.pending_by_key.insert(key.clone(), Arc::clone(&pending));
                (pending, Some(created))
            }
        };

        if let Some(request) = created {
            self.emit_event(SandLocalToolControllerEventKind::Created, request);
        }
        self.wait_for_pending(&pending, cancel)
    }

    pub fn await_desktop_standing_decision(
        &self,
        agent_id: &str,
        tool_call_id: &str,
        command: Option<&str>,
        description: Option<&str>,
        cancel: Option<&AtomicBool>,
    ) -> SandLocalToolDecision {
        let scope = SandLocalToolScope {
            agent_id: agent_id.to_string(),
            session_key: None,
            tool_call_id: Some(tool_call_id.to_string()),
            action: Some("run-command".into()),
            direction_epoch: Some(self.direction_epoch(agent_id)),
        };
        let mut request = SandLocalToolRequest::simple("run-command", command.unwrap_or_default());
        request.description = description
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        self.authorize_with_cancel(Some(&scope), &request, cancel)
    }

    pub fn remembered_refusal_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .refused_actions
            .len()
    }

    pub fn retained_agent_ids(&self) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut ids = HashSet::new();
        ids.extend(state.direction_epochs.keys().cloned());
        ids.extend(state.saturated_directions.keys().cloned());
        ids.extend(state.always_granted_at_epoch.keys().cloned());
        ids.extend(state.refused_actions.values().map(|value| value.agent_id.clone()));
        ids.extend(state.approvals_by_id.values().map(|value| value.agent_id.clone()));
        for pending in state.pending_by_key.values() {
            if let Ok(request) = pending.request.lock() {
                ids.insert(request.agent_id.clone());
            }
        }
        let mut ids = ids.into_iter().collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn note_permission_changed(&self) {
        let permission = self.permission();
        let (pending, retired) = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = state.noted_permission.replace(permission);

            if permission == SandLocalToolPermission::Always
                && previous != Some(SandLocalToolPermission::Always)
            {
                state.always_granted_at_epoch.clear();
                let epochs = state.direction_epochs.clone();
                state.always_granted_at_epoch.extend(epochs);
            }

            if permission == SandLocalToolPermission::Ask
                && previous == Some(SandLocalToolPermission::Never)
            {
                state.refused_actions.clear();
            }

            let retired = if permission == SandLocalToolPermission::Always {
                Vec::new()
            } else {
                let ids = state.approvals_by_id.keys().cloned().collect::<Vec<_>>();
                state.approvals_by_id.clear();
                ids
            };
            let pending = if permission == SandLocalToolPermission::Ask {
                Vec::new()
            } else {
                state.pending_by_key.values().cloned().collect::<Vec<_>>()
            };
            (pending, retired)
        };

        for id in retired {
            self.emit_approval_retired(&id);
        }
        if permission == SandLocalToolPermission::Ask {
            return;
        }
        for pending in pending {
            let request = pending
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let stale = pending.direction_epoch < self.direction_epoch(&request.agent_id);
            if permission == SandLocalToolPermission::Always && stale {
                self.settle_pending(
                    &request.id,
                    SandLocalToolRequestStatus::Expired,
                    denied(SAND_LOCAL_TOOLS_ABANDONED_MESSAGE),
                    true,
                );
            } else if permission == SandLocalToolPermission::Always {
                self.settle_pending(
                    &request.id,
                    SandLocalToolRequestStatus::Always,
                    allowed(None),
                    false,
                );
            } else {
                self.settle_pending(
                    &request.id,
                    SandLocalToolRequestStatus::Never,
                    denied(SAND_LOCAL_TOOLS_DISABLED_MESSAGE),
                    false,
                );
            }
        }
    }

    pub fn find_approval_for_resource(
        &self,
        agent_id: &str,
        resource_path: Option<&str>,
    ) -> Option<String> {
        let wanted = normalize_resource_path(resource_path);
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .approvals_by_id
            .values()
            .find(|approval| {
                approval.agent_id == agent_id
                    && wanted.is_some()
                    && approval.resource_path == wanted
            })
            .map(|approval| approval.id.clone())
    }

    pub fn get_pending_request_for_agent(&self, agent_id: &str) -> Option<SandLocalToolAskRequest> {
        let pending = {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state
                .pending_by_key
                .values()
                .find(|pending| {
                    pending
                        .request
                        .lock()
                        .map(|request| request.agent_id == agent_id)
                        .unwrap_or(false)
                })
                .cloned()
        }?;
        let request = pending
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Some(request)
    }

    pub fn get_pending_request_by_id_full(&self, id: &str) -> Option<SandLocalToolAskRequest> {
        let pending = {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let key = state.pending_by_id.get(id)?;
            state.pending_by_key.get(key).cloned()
        }?;
        let request = pending
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Some(request)
    }

    pub fn was_settled_id(&self, id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .settled_ids
            .contains(id)
    }

    pub fn resolve_request_full(
        &self,
        request_id: &str,
        initial_resolution: SandLocalToolResolution,
    ) -> Option<SandLocalToolAskRequest> {
        let pending = {
            let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let key = state.pending_by_id.get(request_id)?;
            state.pending_by_key.get(key).cloned()
        }?;

        let mut resolution = initial_resolution;
        if matches!(resolution, SandLocalToolResolution::Always) {
            let _ = self
                .settings
                .set_local_tool_permission(SandLocalToolPermission::Always);
            if self.permission() != SandLocalToolPermission::Always {
                resolution = SandLocalToolResolution::AllowOnce;
            }
        } else if matches!(resolution, SandLocalToolResolution::Never) {
            let _ = self
                .settings
                .set_local_tool_permission(SandLocalToolPermission::Never);
        }

        let (status, decision) = match resolution {
            SandLocalToolResolution::AllowOnce => (
                SandLocalToolRequestStatus::Allowed,
                allowed(Some(request_id.to_string())),
            ),
            SandLocalToolResolution::Deny => (
                SandLocalToolRequestStatus::Denied,
                denied(SAND_LOCAL_TOOLS_DENIED_MESSAGE),
            ),
            SandLocalToolResolution::Always => (
                SandLocalToolRequestStatus::Always,
                allowed(Some(request_id.to_string())),
            ),
            SandLocalToolResolution::Never => (
                SandLocalToolRequestStatus::Never,
                denied(SAND_LOCAL_TOOLS_DISABLED_MESSAGE),
            ),
        };

        if decision.allowed {
            let request = pending
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .approvals_by_id
                .insert(
                    request_id.to_string(),
                    StoredApproval {
                        id: request_id.to_string(),
                        agent_id: request.agent_id,
                        tool_call_id: pending.tool_call_id.clone(),
                        action: request.action,
                        target: request.target,
                        resource_path: pending.resource_path.clone(),
                        outlives_scope: pending.outlives_scope,
                    },
                );
        }

        let settled = self.settle_pending(
            request_id,
            status,
            decision,
            matches!(resolution, SandLocalToolResolution::Deny),
        );
        if matches!(resolution, SandLocalToolResolution::Always | SandLocalToolResolution::Never) {
            self.note_permission_changed();
        }
        settled
    }

    pub fn retire_approval(&self, id: &str) -> bool {
        let retired = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .approvals_by_id
            .remove(id)
            .is_some();
        if retired {
            self.emit_approval_retired(id);
        }
        retired
    }

    pub fn live_approval_ids(&self) -> Vec<String> {
        let mut ids = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .approvals_by_id
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn wait_for_pending(
        &self,
        pending: &Arc<PendingAsk>,
        cancel: Option<&AtomicBool>,
    ) -> SandLocalToolDecision {
        {
            let mut waiters = pending
                .waiter_count
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *waiters = waiters.saturating_add(1);
        }
        let started = Instant::now();

        loop {
            if cancel.is_some_and(|signal| signal.load(Ordering::Acquire)) {
                let last = self.leave_waiter(pending);
                let decision = denied(SAND_LOCAL_TOOLS_ASK_CANCELLED_MESSAGE);
                if last {
                    let id = pending
                        .request
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .id
                        .clone();
                    self.settle_pending(
                        &id,
                        SandLocalToolRequestStatus::Expired,
                        decision.clone(),
                        true,
                    );
                }
                return decision;
            }

            let decision_guard = pending
                .decision
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(settled_decision) = decision_guard.clone() {
                drop(decision_guard);
                self.leave_waiter(pending);
                return settled_decision;
            }

            let request = pending
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let logical_remaining = request.expires_at_ms.saturating_sub((self.now_ms)());
            let wall_remaining = self
                .ask_ttl_ms
                .saturating_sub(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
            let remaining = logical_remaining.min(wall_remaining);
            if remaining == 0 {
                drop(decision_guard);
                self.leave_waiter(pending);
                let expired = denied(SAND_LOCAL_TOOLS_ASK_EXPIRED_MESSAGE);
                self.settle_pending(
                    &request.id,
                    SandLocalToolRequestStatus::Expired,
                    expired.clone(),
                    true,
                );
                return expired;
            }

            let wait_ms = if cancel.is_some() {
                remaining.min(25)
            } else {
                remaining
            };
            let (decision_guard, _) = pending
                .wake
                .wait_timeout(decision_guard, Duration::from_millis(wait_ms))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(settled_decision) = decision_guard.clone() {
                drop(decision_guard);
                self.leave_waiter(pending);
                return settled_decision;
            }
        }
    }

    fn leave_waiter(&self, pending: &PendingAsk) -> bool {
        let mut waiters = pending
            .waiter_count
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *waiters = waiters.saturating_sub(1);
        *waiters == 0
    }

    fn emit_event(
        &self,
        kind: SandLocalToolControllerEventKind,
        request: SandLocalToolAskRequest,
    ) {
        if let Some(sink) = self
            .event_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            sink(request.clone());
        }
        let listeners = self
            .event_listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let event = SandLocalToolControllerEvent { kind, request };
        for listener in listeners {
            listener(event.clone());
        }
    }

    fn scope_epoch(&self, scope: &SandLocalToolScope) -> u64 {
        scope
            .direction_epoch
            .unwrap_or_else(|| self.direction_epoch(&scope.agent_id))
    }

    fn predates_standing_grant(&self, scope: &SandLocalToolScope) -> bool {
        let epoch = self.scope_epoch(scope);
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .always_granted_at_epoch
            .get(&scope.agent_id)
            .is_some_and(|granted_at| epoch < *granted_at)
    }

    fn refusal_for(
        &self,
        scope: &SandLocalToolScope,
        request: &SandLocalToolRequest,
    ) -> Option<&'static str> {
        let epoch = self.scope_epoch(scope);
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.forgotten_agents.contains(&scope.agent_id) {
            return Some(SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE);
        }
        if state
            .saturated_directions
            .get(&scope.agent_id)
            .is_some_and(|marker| epoch <= *marker)
        {
            return Some(SAND_LOCAL_TOOLS_STALE_TASK_MESSAGE);
        }
        let key = refusal_key(&scope.agent_id, &request.action, &request.target);
        if state
            .refused_actions
            .get(&key)
            .is_some_and(|refused| refused.direction_epoch >= epoch)
        {
            return Some(SAND_LOCAL_TOOLS_ABANDONED_MESSAGE);
        }
        None
    }

    fn remember_refused_action(&self, pending: &PendingAsk) {
        let request = pending
            .request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let key = refusal_key(&request.agent_id, &request.action, &request.target);
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let sequence = state.next_refusal_sequence;
        state.next_refusal_sequence = state.next_refusal_sequence.saturating_add(1);
        match state.refused_actions.get_mut(&key) {
            Some(existing) => {
                existing.direction_epoch = existing.direction_epoch.max(pending.direction_epoch);
            }
            None => {
                state.refused_actions.insert(
                    key,
                    RefusedAction {
                        agent_id: request.agent_id.clone(),
                        direction_epoch: pending.direction_epoch,
                        sequence,
                    },
                );
            }
        }
        reclaim_refused_actions(&mut state, &request.agent_id);
    }

    fn emit_approval_retired(&self, id: &str) {
        if let Some(sink) = self
            .approval_retired_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            sink(id);
        }
    }

    fn covering_approval(
        &self,
        scope: &SandLocalToolScope,
        request: &SandLocalToolRequest,
    ) -> Option<SandLocalToolDecision> {
        let wanted_resource = normalize_resource_path(request.attach_to_resource_path.as_deref());
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let approval = state.approvals_by_id.values_mut().find(|approval| {
            approval.agent_id == scope.agent_id
                && (scope.tool_call_id.as_deref() == Some(approval.tool_call_id.as_str())
                    || approval.resource_path.is_some())
                && ((approval.action == request.action && approval.target == request.target)
                    || (approval.resource_path.is_some()
                        && approval.resource_path == wanted_resource))
        })?;
        if request.outlives_scope {
            approval.outlives_scope = true;
        }
        Some(allowed(Some(approval.id.clone())))
    }

    fn settle_pending(
        &self,
        request_id: &str,
        status: SandLocalToolRequestStatus,
        decision: SandLocalToolDecision,
        remember_refusal: bool,
    ) -> Option<SandLocalToolAskRequest> {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let key = state.pending_by_id.remove(request_id)?;
            let pending = state.pending_by_key.remove(&key)?;
            state.settled_ids.insert(request_id.to_string());
            state.settled_order.push_back(request_id.to_string());
            while state.settled_order.len() > SAND_LOCAL_TOOL_SETTLED_ID_MEMORY {
                if let Some(oldest) = state.settled_order.pop_front() {
                    state.settled_ids.remove(&oldest);
                }
            }
            pending
        };
        let settled = {
            let mut request = pending
                .request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            request.status = status;
            request.clone()
        };
        if remember_refusal {
            self.remember_refused_action(&pending);
        }
        *pending
            .decision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(decision);
        pending.wake.notify_all();
        self.emit_event(SandLocalToolControllerEventKind::Settled, settled.clone());
        Some(settled)
    }
}

impl LocalToolPermissionAskStore for SandLocalToolPermissionController {
    fn get_pending_request_by_id(
        &self,
        request_id: &str,
    ) -> Option<PendingLocalToolPermissionRequest> {
        self.get_pending_request_by_id_full(request_id)
            .map(|request| PendingLocalToolPermissionRequest {
                agent_id: request.agent_id,
            })
    }

    fn was_settled(&self, request_id: &str) -> bool {
        self.was_settled_id(request_id)
    }

    fn resolve_request(
        &self,
        request_id: &str,
        resolution: SandLocalToolResolution,
    ) -> bool {
        self.resolve_request_full(request_id, resolution).is_some()
    }
}

fn refusal_key(agent_id: &str, action: &str, target: &str) -> String {
    let digest = Sha256::digest(target.as_bytes());
    format!("{agent_id}\0{action}\0{digest:x}")
}

fn reclaim_refused_actions(state: &mut ControllerState, agent_id: &str) {
    let current = state.direction_epochs.get(agent_id).copied().unwrap_or(0);
    let mut mine = state
        .refused_actions
        .iter()
        .filter(|(_, refused)| refused.agent_id == agent_id)
        .map(|(key, refused)| (key.clone(), refused.clone()))
        .collect::<Vec<_>>();
    if mine.len() <= SAND_LOCAL_TOOL_REFUSED_ACTION_MEMORY_PER_AGENT {
        return;
    }
    mine.sort_by_key(|(_, refused)| refused.sequence);

    for (key, refused) in mine.clone() {
        if state
            .refused_actions
            .values()
            .filter(|value| value.agent_id == agent_id)
            .count()
            <= SAND_LOCAL_TOOL_REFUSED_ACTION_MEMORY_PER_AGENT
        {
            break;
        }
        if current.saturating_sub(refused.direction_epoch) > SAND_LOCAL_TOOL_REFUSAL_DIRECTION_WINDOW {
            state.refused_actions.remove(&key);
            let marker = state.saturated_directions.entry(agent_id.to_string()).or_insert(0);
            *marker = (*marker).max(refused.direction_epoch);
        }
    }

    for (key, refused) in mine {
        if state
            .refused_actions
            .values()
            .filter(|value| value.agent_id == agent_id)
            .count()
            <= SAND_LOCAL_TOOL_REFUSED_ACTION_MEMORY_PER_AGENT
        {
            break;
        }
        if state.refused_actions.remove(&key).is_some() {
            let marker = state.saturated_directions.entry(agent_id.to_string()).or_insert(0);
            *marker = (*marker).max(refused.direction_epoch);
        }
    }
}

fn normalize_resource_path(path: Option<&str>) -> Option<String> {
    path.map(|path| path.replace('\\', "/")).filter(|path| !path.is_empty())
}

fn allowed(approval_id: Option<String>) -> SandLocalToolDecision {
    SandLocalToolDecision {
        allowed: true,
        reason: None,
        approval_id,
    }
}

fn denied(reason: &str) -> SandLocalToolDecision {
    SandLocalToolDecision {
        allowed: false,
        reason: Some(reason.to_string()),
        approval_id: None,
    }
}
