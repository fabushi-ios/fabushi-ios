use crate::codebase_telemetry::cursor_backend::{
    configured_backend_url, sand_client_version, sand_namespace,
};
use crate::notify_bus::{MobileNotifyBusOwner, NotifyBusSubscription, SandNotifyTopic};
use reqwest::blocking::Client;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

pub const AUTOMATIONS_POLL_INTERVAL_MS: u64 = 4_000;
pub const LISTENER_WATCH_INTERVAL_MS: u64 = 5_000;
const REGISTER_INTERVAL_MS: u64 = 5 * 60_000;
const ERROR_BACKOFF_MS: u64 = 30_000;
const NOTIFY_DRAIN_FLOOR_MS: u64 = 250;
const MAX_NEXT_POLL_DELAY_MS: u64 = 60_000;
const SAND_CLIENT_TYPE: &str = "sand";

pub trait AutomationsHostBridge: Send + Sync {
    fn request(&self, method: &str, params: Value) -> Result<Value, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountScope {
    identity: String,
    session_id: Option<String>,
    device_id: Option<String>,
    generation: Option<String>,
}

impl AccountScope {
    fn from_session(session: &Value) -> Result<Self, String> {
        let text = |name: &str| {
            session
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        let identity = text("userId")
            .or_else(|| text("sessionId"))
            .or_else(|| text("deviceId"))
            .ok_or_else(|| "automations auth session missing account identity".to_string())?;
        let generation = session
            .get("accountGeneration")
            .or_else(|| session.get("generation"))
            .and_then(|value| match value {
                Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            });
        Ok(Self {
            identity,
            session_id: text("sessionId"),
            device_id: text("deviceId"),
            generation,
        })
    }
}

#[derive(Debug)]
enum BackendError {
    Transport(String),
    Status { path: String, status: u16 },
    Decode(String),
}

impl BackendError {
    fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(message) | Self::Decode(message) => formatter.write_str(message),
            Self::Status { path, status } => write!(formatter, "{path} returned HTTP {status}"),
        }
    }
}

struct AuthenticatedBackendTransport {
    bridge: Arc<dyn AutomationsHostBridge>,
    client: Client,
}

impl AuthenticatedBackendTransport {
    fn new(bridge: Arc<dyn AutomationsHostBridge>) -> Result<Self, String> {
        Ok(Self {
            bridge,
            client: Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|error| format!("build automations backend client: {error}"))?,
        })
    }

    fn session(&self) -> Result<Value, String> {
        self.bridge
            .request("feature.auth.deviceAgentSession", Value::Null)
    }

    fn scope(&self) -> Result<AccountScope, String> {
        AccountScope::from_session(&self.session()?)
    }

    fn post_json(&self, path: &str, body: &Value) -> Result<Value, BackendError> {
        let session = self
            .session()
            .map_err(BackendError::Transport)?;
        let token = session
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| BackendError::Transport("automations auth session missing accessToken".into()))?;
        let base = Url::parse(&configured_backend_url())
            .map_err(|error| BackendError::Transport(format!("automations backend URL invalid: {error}")))?;
        let url = base
            .join(path)
            .map_err(|error| BackendError::Transport(format!("automations backend path invalid: {error}")))?;
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_namespace())
            .json(body)
            .send()
            .map_err(|error| BackendError::Transport(format!("automations backend transport failed: {error}")))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(BackendError::Status {
                path: path.to_string(),
                status,
            });
        }
        response
            .json::<Value>()
            .map_err(|error| BackendError::Decode(format!("automations backend decode failed: {error}")))
    }
}

#[derive(Debug, Clone)]
enum FireEntry {
    Running,
    Reported,
}

#[derive(Debug, Default)]
struct RelayState {
    registered_key: String,
    registered_at_ms: u64,
    pending_ack_ids: Vec<String>,
    notify_pending: bool,
    backoff_until_ms: u64,
}

impl RelayState {
    fn reset(&mut self) {
        *self = Self::default();
        self.notify_pending = true;
    }
}

#[derive(Debug, Default)]
struct FireState {
    states: BTreeMap<String, FireEntry>,
    notify_pending: bool,
    notify_seq: u64,
    drained_notify_seq: u64,
    drained_while_unschedulable: bool,
    last_poll_at_ms: Option<u64>,
    backoff_until_ms: u64,
    poll_not_before_ms: u64,
}

impl FireState {
    fn reset(&mut self) {
        *self = Self::default();
        self.notify_pending = true;
        self.notify_seq = 1;
    }

    fn request_drain(&mut self) {
        self.notify_pending = true;
        self.notify_seq = self.notify_seq.saturating_add(1);
        self.poll_not_before_ms = 0;
        self.drained_while_unschedulable = false;
    }
}

struct Wake {
    requested: Mutex<bool>,
    cv: Condvar,
    stopped: AtomicBool,
    suspended: AtomicBool,
}

impl Wake {
    fn new() -> Self {
        Self {
            requested: Mutex::new(true),
            cv: Condvar::new(),
            stopped: AtomicBool::new(false),
            suspended: AtomicBool::new(false),
        }
    }

    fn signal(&self) {
        *self.requested.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.cv.notify_all();
    }

    fn wait(&self, duration: Duration) {
        let guard = self.requested.lock().unwrap_or_else(|p| p.into_inner());
        let (mut guard, _) = self
            .cv
            .wait_timeout_while(guard, duration, |pending| !*pending)
            .unwrap_or_else(|p| p.into_inner());
        *guard = false;
    }
}

pub struct MobileAutomationsBackendOwner {
    bridge: Arc<dyn AutomationsHostBridge>,
    notify_bus: Arc<MobileNotifyBusOwner>,
    transport: Arc<AuthenticatedBackendTransport>,
    scope: Mutex<Option<AccountScope>>,
    relay: Mutex<RelayState>,
    fire: Mutex<FireState>,
    wake: Arc<Wake>,
    worker: Mutex<Option<JoinHandle<()>>>,
    notify_subscriptions: Mutex<Vec<NotifyBusSubscription>>,
}

impl MobileAutomationsBackendOwner {
    pub fn start(
        bridge: Arc<dyn AutomationsHostBridge>,
        notify_bus: Arc<MobileNotifyBusOwner>,
    ) -> Result<Arc<Self>, String> {
        let owner = Arc::new(Self {
            transport: Arc::new(AuthenticatedBackendTransport::new(Arc::clone(&bridge))?),
            bridge,
            notify_bus,
            scope: Mutex::new(None),
            relay: Mutex::new(RelayState::default()),
            fire: Mutex::new(FireState::default()),
            wake: Arc::new(Wake::new()),
            worker: Mutex::new(None),
            notify_subscriptions: Mutex::new(Vec::new()),
        });

        let weak = Arc::downgrade(&owner);
        let listener = owner.notify_bus.on_notify(
            SandNotifyTopic::ListenerEvents,
            Arc::new(move || {
                if let Some(owner) = weak.upgrade() {
                    owner.relay.lock().unwrap_or_else(|p| p.into_inner()).notify_pending = true;
                    owner.wake.signal();
                }
            }),
        );
        let weak = Arc::downgrade(&owner);
        let fire = owner.notify_bus.on_notify(
            SandNotifyTopic::AutomationFires,
            Arc::new(move || {
                if let Some(owner) = weak.upgrade() {
                    owner.fire.lock().unwrap_or_else(|p| p.into_inner()).request_drain();
                    owner.wake.signal();
                }
            }),
        );
        owner
            .notify_subscriptions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend([listener, fire]);

        let weak = Arc::downgrade(&owner);
        let worker = thread::Builder::new()
            .name("ios-production-automations-backend".into())
            .spawn(move || run_loop(weak))
            .map_err(|error| format!("spawn mobile automations backend runtime: {error}"))?;
        *owner.worker.lock().unwrap_or_else(|p| p.into_inner()) = Some(worker);
        Ok(owner)
    }

    pub fn observe_host_dispatch(&self, input: &str, output: &str) {
        if self.wake.stopped.load(Ordering::Acquire) {
            return;
        }
        let Ok(response) = serde_json::from_str::<Value>(output) else { return };
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return;
        }
        let Ok(request) = serde_json::from_str::<Value>(input) else { return };
        match request.get("method").and_then(Value::as_str) {
            Some("feature.sessionActivity")
                if request
                    .get("params")
                    .and_then(|params| params.get("active"))
                    .and_then(Value::as_bool)
                    == Some(false) =>
            {
                self.suspend();
            }
            Some("feature.sessionActivity") => self.resume(),
            Some(
                "feature.auth.logout"
                | "feature.auth.passwordLogin"
                | "feature.auth.browserPoll"
                | "feature.execute",
            ) => self.request_reconcile(),
            _ => {}
        }
    }

    pub fn request_reconcile(&self) {
        self.relay.lock().unwrap_or_else(|p| p.into_inner()).notify_pending = true;
        self.fire.lock().unwrap_or_else(|p| p.into_inner()).request_drain();
        self.wake.signal();
    }

    pub fn suspend(&self) {
        if !self.wake.stopped.load(Ordering::Acquire) {
            self.wake.suspended.store(true, Ordering::Release);
            self.wake.signal();
        }
    }

    pub fn resume(&self) {
        if !self.wake.stopped.load(Ordering::Acquire) {
            self.wake.suspended.store(false, Ordering::Release);
            self.request_reconcile();
        }
    }

    pub fn stop(&self) {
        if self.wake.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.notify_subscriptions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.wake.signal();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
        self.reset_scope();
    }

    fn reset_scope(&self) {
        *self.scope.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.relay.lock().unwrap_or_else(|p| p.into_inner()).reset();
        self.fire.lock().unwrap_or_else(|p| p.into_inner()).reset();
    }

    fn refresh_scope(&self) -> bool {
        match self.transport.scope() {
            Ok(next) => {
                let changed = {
                    let mut current = self.scope.lock().unwrap_or_else(|p| p.into_inner());
                    if current.as_ref() == Some(&next) {
                        false
                    } else {
                        *current = Some(next);
                        true
                    }
                };
                if changed {
                    self.relay.lock().unwrap_or_else(|p| p.into_inner()).reset();
                    self.fire.lock().unwrap_or_else(|p| p.into_inner()).reset();
                }
                true
            }
            Err(_) => {
                self.reset_scope();
                false
            }
        }
    }

    fn scope_is_current(&self) -> bool {
        let expected = self.scope.lock().unwrap_or_else(|p| p.into_inner()).clone();
        expected.is_some() && self.transport.scope().ok() == expected
    }
}

impl Drop for MobileAutomationsBackendOwner {
    fn drop(&mut self) {
        if !self.wake.stopped.swap(true, Ordering::AcqRel) {
            self.wake.signal();
        }
    }
}

fn run_loop(weak: Weak<MobileAutomationsBackendOwner>) {
    let mut last_listener_watch_ms = 0;
    while let Some(owner) = weak.upgrade() {
        if owner.wake.stopped.load(Ordering::Acquire) {
            break;
        }
        if owner.wake.suspended.load(Ordering::Acquire) {
            owner.wake.wait(Duration::from_secs(3600));
            continue;
        }
        let now = now_ms();
        if owner.refresh_scope() {
            let _ = tick_relay(&owner, now);
            let _ = tick_fire(&owner, now);
            if now.saturating_sub(last_listener_watch_ms) >= LISTENER_WATCH_INTERVAL_MS {
                let _ = owner
                    .bridge
                    .request("feature.automation.pollListenerReconnects", Value::Null);
                last_listener_watch_ms = now;
            }
        }
        owner
            .wake
            .wait(Duration::from_millis(AUTOMATIONS_POLL_INTERVAL_MS));
    }
}

fn tick_relay(owner: &MobileAutomationsBackendOwner, now: u64) -> Result<(), String> {
    let mut state = owner.relay.lock().unwrap_or_else(|p| p.into_inner());
    if now < state.backoff_until_ms {
        return Ok(());
    }

    let subscription = owner
        .bridge
        .request("feature.automation.relaySubscription", Value::Null)?;
    let key = serde_json::to_string(&subscription)
        .map_err(|error| format!("encode relay subscription: {error}"))?;
    if key != state.registered_key
        || now.saturating_sub(state.registered_at_ms) >= REGISTER_INTERVAL_MS
    {
        owner
            .transport
            .post_json("/sand/listener-subscriptions", &subscription)
            .map_err(|error| {
                state.backoff_until_ms = now.saturating_add(ERROR_BACKOFF_MS);
                error.to_string()
            })?;
        if !owner.scope_is_current() {
            state.reset();
            return Err("account changed during listener registration".into());
        }
        state.registered_key = key;
        state.registered_at_ms = now;
        state.notify_pending = true;
    }

    if !state.notify_pending
        && !owner.notify_bus.is_connected()
        && !owner.notify_bus.is_safety_poll_enabled()
        && state.pending_ack_ids.is_empty()
    {
        return Ok(());
    }

    let previous = state.pending_ack_ids.iter().cloned().collect::<BTreeSet<_>>();
    let response = owner
        .transport
        .post_json(
            "/sand/listener-events/poll",
            &json!({"ackIds": state.pending_ack_ids.clone()}),
        )
        .map_err(|error| {
            state.backoff_until_ms = now.saturating_add(ERROR_BACKOFF_MS);
            error.to_string()
        })?;
    if !owner.scope_is_current() {
        state.reset();
        return Err("account changed during listener poll".into());
    }

    let mut next_ack_ids = Vec::new();
    for raw in response
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = raw
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        if previous.contains(&id) {
            next_ack_ids.push(id);
            continue;
        }
        let Some(event) = map_relay_event(raw, now) else {
            next_ack_ids.push(id);
            continue;
        };
        if owner
            .bridge
            .request("feature.automation.ingestListenerEvent", json!({"event": event}))
            .is_ok()
            && owner.scope_is_current()
        {
            next_ack_ids.push(id);
        }
    }
    state.pending_ack_ids = next_ack_ids;
    state.notify_pending = false;
    state.backoff_until_ms = 0;
    Ok(())
}

fn tick_fire(owner: &MobileAutomationsBackendOwner, now: u64) -> Result<(), String> {
    let poll_state = owner
        .bridge
        .request("feature.automation.backendFirePollState", Value::Null)?;
    let is_ready = poll_state.get("isReady").and_then(Value::as_bool) == Some(true);
    let has_server_schedulable = poll_state
        .get("hasServerSchedulable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !is_ready {
        return Ok(());
    }

    let mut state = owner.fire.lock().unwrap_or_else(|p| p.into_inner());
    if now < state.backoff_until_ms || now < state.poll_not_before_ms {
        return Ok(());
    }

    settle_running(owner, &mut state)?;

    let has_owed_work = !state.states.is_empty();
    state.drained_notify_seq = state.notify_seq;
    if !has_owed_work {
        if owner.notify_bus.is_connected() {
            let should_drain = match state.last_poll_at_ms {
                None => true,
                Some(last) => {
                    let elapsed = now.saturating_sub(last);
                    (state.notify_pending && elapsed >= NOTIFY_DRAIN_FLOOR_MS)
                        || (owner.notify_bus.is_safety_poll_enabled()
                            && elapsed >= AUTOMATIONS_POLL_INTERVAL_MS)
                }
            };
            if !should_drain {
                return Ok(());
            }
        }
        if !has_server_schedulable && state.drained_while_unschedulable {
            return Ok(());
        }
    }

    let ack_run_uuids = state
        .states
        .iter()
        .filter_map(|(id, entry)| matches!(entry, FireEntry::Reported).then_some(id.clone()))
        .collect::<Vec<_>>();
    let response = owner
        .transport
        .post_json(
            "/sand/automation-events/poll",
            &json!({"ackRunUuids": ack_run_uuids}),
        )
        .map_err(|error| {
            state.backoff_until_ms = now.saturating_add(ERROR_BACKOFF_MS);
            error.to_string()
        })?;
    if !owner.scope_is_current() {
        state.reset();
        return Err("account changed during automation fire poll".into());
    }

    if state.notify_seq == state.drained_notify_seq {
        state.notify_pending = false;
    }
    state.last_poll_at_ms = Some(now);
    let events = response
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let returned = events
        .iter()
        .filter_map(|raw| raw.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect::<BTreeSet<_>>();
    state
        .states
        .retain(|id, entry| !matches!(entry, FireEntry::Reported) || returned.contains(id));

    let mut delivered = 0usize;
    for raw in events {
        let Some(id) = raw
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= 200)
            .map(str::to_owned)
        else {
            continue;
        };
        if state.states.contains_key(&id) {
            continue;
        }
        let admission = owner
            .bridge
            .request("feature.automation.admitBackendFire", json!({"fire": raw}))?;
        if !owner.scope_is_current() {
            state.reset();
            return Err("account changed during automation fire admission".into());
        }
        match admission.get("disposition").and_then(Value::as_str) {
            Some("admitted" | "existingRunning") => {
                state.states.insert(id, FireEntry::Running);
                delivered += 1;
            }
            Some("complete") => {
                state.states.insert(id.clone(), FireEntry::Running);
                let status = admission
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                let error_message = admission.get("errorMessage").cloned().unwrap_or(Value::Null);
                report_completion(owner, &id, status, error_message)?;
                state.states.insert(id, FireEntry::Reported);
            }
            Some("abandon") | None | Some(_) => {
                state.states.remove(&id);
            }
        }
    }

    settle_running(owner, &mut state)?;
    if !has_owed_work {
        state.drained_while_unschedulable = !has_server_schedulable;
    }
    let next = response
        .get("nextPollAfterMs")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(MAX_NEXT_POLL_DELAY_MS);
    if delivered == 0 && state.states.is_empty() && next > 0 {
        state.poll_not_before_ms = now.saturating_add(next);
    } else {
        state.poll_not_before_ms = 0;
    }
    state.backoff_until_ms = 0;
    Ok(())
}

fn settle_running(
    owner: &MobileAutomationsBackendOwner,
    state: &mut FireState,
) -> Result<(), String> {
    let running = state
        .states
        .iter()
        .filter_map(|(id, entry)| matches!(entry, FireEntry::Running).then_some(id.clone()))
        .collect::<Vec<_>>();
    for id in running {
        let status = owner
            .bridge
            .request("feature.automation.backendFireStatus", json!({"runUuid": id.clone()}))?;
        if !owner.scope_is_current() {
            state.reset();
            return Err("account changed while settling automation fire".into());
        }
        match status.get("state").and_then(Value::as_str) {
            Some("completed") => {
                let completion = status.get("status").and_then(Value::as_str).unwrap_or("failed");
                let error_message = status.get("errorMessage").cloned().unwrap_or(Value::Null);
                report_completion(owner, &id, completion, error_message)?;
                state.states.insert(id, FireEntry::Reported);
            }
            Some("missing") => {
                state.states.remove(&id);
            }
            _ => {}
        }
    }
    Ok(())
}

fn report_completion(
    owner: &MobileAutomationsBackendOwner,
    id: &str,
    status: &str,
    error_message: Value,
) -> Result<(), String> {
    match owner.transport.post_json(
        "/sand/automation-runs/complete",
        &json!({
            "runUuid": id,
            "status": status,
            "errorMessage": error_message,
        }),
    ) {
        Ok(_) => {
            if owner.scope_is_current() {
                Ok(())
            } else {
                Err("account changed during automation completion".into())
            }
        }
        Err(error) if matches!(error.status(), Some(404 | 409)) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn map_relay_event(raw: &Value, now: u64) -> Option<Value> {
    let object = raw.as_object()?;
    let source = object.get("source")?.as_str()?;
    let timestamp = object
        .get("timestampMs")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| now.min(i64::MAX as u64) as i64);
    match source {
        "slack" => map_slack_event(object, timestamp),
        "github" => map_github_event(object, timestamp),
        _ => None,
    }
}

fn event_field(label: &str, value: impl Into<String>) -> Value {
    json!({"label": label, "value": value.into()})
}

fn map_slack_event(object: &Map<String, Value>, timestamp: i64) -> Option<Value> {
    let channel_name = object
        .get("channelName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| object.get("channelId").and_then(Value::as_str))?;
    let channel_id = object.get("channelId").and_then(Value::as_str);
    let channel = if Some(channel_name) == channel_id || channel_name.starts_with('#') {
        channel_name.to_string()
    } else {
        format!("#{channel_name}")
    };
    let sender = object
        .get("senderSlackUserId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(|value| format!("@{value}"))
        .unwrap_or_else(|| "someone".into());
    let is_reaction = object.get("kind").and_then(Value::as_str) == Some("reaction");
    let event = if is_reaction {
        "reaction"
    } else if object.get("isMention").and_then(Value::as_bool) == Some(true) {
        "mention"
    } else {
        "message"
    };
    let mut fields = vec![
        event_field("channel", channel.clone()),
        event_field("sender", sender.clone()),
    ];
    if let Some(value) = channel_id {
        fields.push(event_field("channelId", value));
    }
    for field in ["ts", "threadTs"] {
        if let Some(value) = object.get(field).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            fields.push(event_field(field, value));
        }
    }
    fields.push(event_field(
        "isMention",
        object
            .get("isMention")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .to_string(),
    ));
    if is_reaction {
        let emoji = object
            .get("reactionEmoji")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())?;
        fields.push(event_field("reactionEmoji", format!(":{}:", emoji.replace(':', ""))));
        fields.push(event_field(
            "isSelf",
            object.get("isSelf").and_then(Value::as_bool).unwrap_or(false).to_string(),
        ));
    }
    Some(json!({
        "source": "slack",
        "event": event,
        "title": channel,
        "summary": object.get("text").and_then(Value::as_str).unwrap_or_default(),
        "actor": sender,
        "fields": fields,
        "occurredAtMs": timestamp,
    }))
}

fn map_github_event(object: &Map<String, Value>, timestamp: i64) -> Option<Value> {
    let repo = object.get("repo").and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let kind = object.get("kind").and_then(Value::as_str).filter(|value| !value.is_empty())?;
    let mut fields = vec![
        event_field("repo", repo),
        event_field("kind", kind),
    ];
    for field in ["prOwner", "branch"] {
        if let Some(value) = object.get(field).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            fields.push(event_field(field, value));
        }
    }
    Some(json!({
        "source": "github",
        "event": kind,
        "title": object.get("title").and_then(Value::as_str).unwrap_or(repo),
        "summary": object.get("detail").and_then(Value::as_str)
            .or_else(|| object.get("title").and_then(Value::as_str))
            .unwrap_or_default(),
        "url": object.get("url").and_then(Value::as_str),
        "actor": object.get("actor").and_then(Value::as_str).unwrap_or("someone"),
        "fields": fields,
        "occurredAtMs": timestamp,
    }))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_mapping_preserves_trigger_fields() {
        let slack = map_relay_event(
            &json!({
                "source":"slack",
                "channelName":"alerts",
                "channelId":"C1",
                "text":"incident",
                "senderSlackUserId":"U1",
                "isMention":true,
                "timestampMs":42
            }),
            99,
        )
        .expect("slack event");
        assert_eq!(slack["source"], "slack");
        assert_eq!(slack["event"], "mention");
        assert_eq!(slack["occurredAtMs"], 42);
        assert!(slack["fields"].as_array().unwrap().iter().any(|field| {
            field["label"] == "channel" && field["value"] == "#alerts"
        }));

        let github = map_relay_event(
            &json!({
                "source":"github",
                "repo":"openai/repo",
                "kind":"pr-opened",
                "title":"PR",
                "prOwner":"octocat",
                "branch":"feature",
                "timestampMs":43
            }),
            99,
        )
        .expect("github event");
        assert_eq!(github["event"], "pr-opened");
        assert!(github["fields"].as_array().unwrap().iter().any(|field| {
            field["label"] == "repo" && field["value"] == "openai/repo"
        }));
    }

    #[test]
    fn account_scope_fences_session_and_generation_changes() {
        let first = AccountScope::from_session(&json!({
            "userId":"user",
            "sessionId":"one",
            "deviceId":"device",
            "accountGeneration":7
        }))
        .unwrap();
        let second = AccountScope::from_session(&json!({
            "userId":"user",
            "sessionId":"two",
            "deviceId":"device",
            "accountGeneration":8
        }))
        .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn reset_discards_stale_ack_and_fire_ownership() {
        let mut relay = RelayState {
            pending_ack_ids: vec!["event".into()],
            ..RelayState::default()
        };
        let mut fire = FireState::default();
        fire.states.insert("run".into(), FireEntry::Running);
        relay.reset();
        fire.reset();
        assert!(relay.pending_ack_ids.is_empty());
        assert!(fire.states.is_empty());
        assert!(relay.notify_pending);
        assert!(fire.notify_pending);
    }
}
