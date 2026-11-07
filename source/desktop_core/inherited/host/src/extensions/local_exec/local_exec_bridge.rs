use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use uuid::Uuid;

use super::local_exec_error::SandLocalExecError;

pub const SUPERVISED_RANK: i32 = 4;
pub const SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS: u64 = 30_000;
pub const SAND_LOCAL_EXEC_RESPONSE_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_SAND_COMPUTER_ID: &str = "this-computer";
pub const SAND_NO_LOCAL_MACHINE_MESSAGE: &str =
    "Your local machine isn't connected right now (the Fabushi desktop app must be open and online to run commands on it). Try again once it's reachable.";
const COMPUTER_UNAVAILABLE_SUFFIX: &str =
    "is unavailable — it looks disconnected. Reconnect it (or focus the computer you want commands to run on) and try again.";

pub fn sand_computer_unavailable_message(label: Option<&str>) -> String {
    match label.map(str::trim).filter(|value| !value.is_empty()) {
        Some(label) => format!("Your computer \"{label}\" {COMPUTER_UNAVAILABLE_SUFFIX}"),
        None => format!("Your computer {COMPUTER_UNAVAILABLE_SUFFIX}"),
    }
}

pub fn local_exec_provider_rank(supervised: bool, variant: Option<&str>) -> i32 {
    let variant_rank = match variant {
        Some("sand") => 2,
        Some("sand-lab") => 1,
        _ => 0,
    };
    (if supervised { SUPERVISED_RANK } else { 0 }) + variant_rank
}

pub fn bounded_local_exec_variant(variant: Option<&str>) -> Option<&str> {
    match variant {
        Some("sand" | "sand-lab" | "sand-dev") => variant,
        Some(_) => Some("unknown"),
        None => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecComputer {
    pub id: String,
    pub label: String,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecProviderInfo {
    pub local_root: String,
    pub terminals_folder: String,
}

#[derive(Clone)]
struct Provider {
    id: String,
    send: Sender<Value>,
    sequence: u64,
    registered_at_ms: u64,
    last_seen_at_ms: u64,
    has_heartbeat: bool,
    info: Option<LocalExecProviderInfo>,
    computer_id: Option<String>,
    label: Option<String>,
    supervised: Option<bool>,
    variant: Option<String>,
}

impl Provider {
    fn computer_id(&self) -> &str {
        self.computer_id.as_deref().unwrap_or(DEFAULT_SAND_COMPUTER_ID)
    }

    fn label(&self) -> &str {
        self.label.as_deref().unwrap_or("this computer")
    }

    fn live(&self, now_ms: u64) -> bool {
        !self.has_heartbeat
            || now_ms.saturating_sub(self.last_seen_at_ms) <= SAND_LOCAL_EXEC_LIVENESS_WINDOW_MS
    }

    fn rank(&self) -> i32 {
        local_exec_provider_rank(self.supervised.unwrap_or(false), self.variant.as_deref())
    }
}

struct BridgeState {
    providers: HashMap<String, Provider>,
    pending: HashMap<String, Sender<Value>>,
    ever_registered: bool,
    empty_since_ms: u64,
    next_sequence: u64,
}

impl BridgeState {
    fn new(now_ms: u64) -> Self {
        Self {
            providers: HashMap::new(),
            pending: HashMap::new(),
            ever_registered: false,
            empty_since_ms: now_ms,
            next_sequence: 0,
        }
    }
}

type Now = Arc<dyn Fn() -> u64 + Send + Sync>;
type RandomId = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalExecProviderLifecycleReport {
    Registered {
        provider_id: String,
        provider_count: usize,
    },
    Hello {
        provider_id: String,
        provider_count: usize,
        hello_delay_ms: u64,
        computer_id_present: bool,
        rehello: bool,
        supervised: Option<bool>,
        variant: Option<String>,
    },
    Detached {
        provider_id: String,
        provider_count: usize,
        age_ms: u64,
        had_hello: bool,
        has_heartbeat: bool,
        was_live: bool,
        emptied: bool,
    },
}

pub type ProviderLifecycleReporter =
    Arc<dyn Fn(LocalExecProviderLifecycleReport) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecRefusalCause {
    NoProviders,
    ComputerUnknown,
    StaleHeartbeat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecRefusalReport {
    pub cause: LocalExecRefusalCause,
    pub site: String,
    pub conversation_id: Option<String>,
    pub provider_count: usize,
    pub live_provider_count: usize,
    pub ever_registered: bool,
    pub empty_for_ms: Option<u64>,
}

pub type RefusalReporter = Arc<dyn Fn(LocalExecRefusalReport) + Send + Sync>;

#[derive(Clone)]
pub struct SandLocalExecBridge {
    state: Arc<Mutex<BridgeState>>,
    now_ms: Now,
    random_id: RandomId,
    provider_reporter: Option<ProviderLifecycleReporter>,
    refusal_reporter: Option<RefusalReporter>,
}

impl SandLocalExecBridge {
    pub fn production() -> Self {
        Self::production_with_reporter(None)
    }

    pub fn production_with_reporter(
        provider_reporter: Option<ProviderLifecycleReporter>,
    ) -> Self {
        Self::production_with_reporters(provider_reporter, None)
    }

    pub fn production_with_reporters(
        provider_reporter: Option<ProviderLifecycleReporter>,
        refusal_reporter: Option<RefusalReporter>,
    ) -> Self {
        Self::with_sources_and_reporters(
            Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            }),
            Arc::new(|| Uuid::new_v4().to_string()),
            provider_reporter,
            refusal_reporter,
        )
    }

    pub fn with_sources(now_ms: Now, random_id: RandomId) -> Self {
        Self::with_sources_and_reporters(now_ms, random_id, None, None)
    }

    pub fn with_sources_and_reporter(
        now_ms: Now,
        random_id: RandomId,
        provider_reporter: Option<ProviderLifecycleReporter>,
    ) -> Self {
        Self::with_sources_and_reporters(now_ms, random_id, provider_reporter, None)
    }

    pub fn with_sources_and_reporters(
        now_ms: Now,
        random_id: RandomId,
        provider_reporter: Option<ProviderLifecycleReporter>,
        refusal_reporter: Option<RefusalReporter>,
    ) -> Self {
        let now = now_ms();
        Self {
            state: Arc::new(Mutex::new(BridgeState::new(now))),
            now_ms,
            random_id,
            provider_reporter,
            refusal_reporter,
        }
    }

    pub fn register_provider(&self, send: Sender<Value>) -> LocalExecProviderRegistration {
        let now = (self.now_ms)();
        let id = (self.random_id)();
        let provider_count = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);
            state.ever_registered = true;
            state.providers.insert(
                id.clone(),
                Provider {
                    id: id.clone(),
                    send: send.clone(),
                    sequence,
                    registered_at_ms: now,
                    last_seen_at_ms: now,
                    has_heartbeat: false,
                    info: None,
                    computer_id: None,
                    label: None,
                    supervised: None,
                    variant: None,
                },
            );
            state.providers.len()
        };
        self.report_provider(LocalExecProviderLifecycleReport::Registered {
            provider_id: id.clone(),
            provider_count,
        });
        let _ = send.send(json!({ "kind": "welcome", "providerId": id }));
        LocalExecProviderRegistration {
            bridge: self.clone(),
            provider_id: Some(id),
        }
    }

    fn detach_provider(&self, provider_id: &str) {
        let now = (self.now_ms)();
        let report = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let removed = state.providers.remove(provider_id);
            removed.map(|provider| {
                let emptied = state.providers.is_empty();
                if emptied {
                    state.empty_since_ms = now;
                }
                let was_live = provider.live(now);
                LocalExecProviderLifecycleReport::Detached {
                    provider_id: provider.id,
                    provider_count: state.providers.len(),
                    age_ms: now.saturating_sub(provider.registered_at_ms),
                    had_hello: provider.info.is_some(),
                    has_heartbeat: provider.has_heartbeat,
                    was_live,
                    emptied,
                }
            })
        };
        if let Some(report) = report {
            self.report_provider(report);
        }
    }

    pub fn has_provider(&self) -> bool {
        !self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).providers.is_empty()
    }

    pub fn provider_count(&self) -> usize {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).providers.len()
    }

    pub fn ever_registered(&self) -> bool {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).ever_registered
    }

    pub fn get_provider_info(&self) -> Option<LocalExecProviderInfo> {
        let now = (self.now_ms)();
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.best_provider(&state, now, None)
            .or_else(|| state.providers.values().max_by_key(|provider| provider.sequence))
            .and_then(|provider| provider.info.clone())
    }

    pub fn list_computers(&self) -> Vec<LocalExecComputer> {
        let now = (self.now_ms)();
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut newest: HashMap<String, &Provider> = HashMap::new();
        for provider in state.providers.values() {
            let id = provider.computer_id().to_string();
            let replace = newest
                .get(&id)
                .is_none_or(|current| provider.last_seen_at_ms >= current.last_seen_at_ms);
            if replace {
                newest.insert(id, provider);
            }
        }
        let mut computers = newest
            .into_iter()
            .map(|(id, provider)| LocalExecComputer {
                id,
                label: provider.label().to_string(),
                connected: provider.live(now),
            })
            .collect::<Vec<_>>();
        computers.sort_by(|left, right| left.id.cmp(&right.id));
        computers
    }

    pub fn active_computer(&self) -> Option<LocalExecComputer> {
        let now = (self.now_ms)();
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.best_provider(&state, now, None).map(|provider| LocalExecComputer {
            id: provider.computer_id().to_string(),
            label: provider.label().to_string(),
            connected: true,
        })
    }

    pub fn is_computer_live(&self, computer_id: &str) -> bool {
        let now = (self.now_ms)();
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.best_provider(&state, now, Some(computer_id)).is_some()
    }

    pub fn check_live_computer_for_ask(&self) -> bool {
        self.check_live_computer_for_ask_with_agent(None)
    }

    pub fn check_live_computer_for_ask_with_agent(&self, agent_id: Option<&str>) -> bool {
        self.assert_computer_available(None, "ask_gate", agent_id)
            .is_ok()
    }

    pub fn assert_computer_available(
        &self,
        computer_id: Option<&str>,
        site: &str,
        agent_id: Option<&str>,
    ) -> Result<(), SandLocalExecError> {
        let now = (self.now_ms)();
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.best_provider(&state, now, computer_id).is_some() {
            return Ok(());
        }
        let report = self.refusal_report(&state, now, computer_id, site, agent_id);
        let error = self.unavailable_error(&state, now, computer_id);
        drop(state);
        self.report_refusal(report);
        Err(error)
    }

    pub fn submit_responses(&self, batch: Value) {
        let provider_id = batch.get("providerId").and_then(Value::as_str).map(str::to_string);
        let frames = batch
            .get("frames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let now = (self.now_ms)();
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut provider_reports = Vec::new();
        let fallback = state
            .providers
            .values()
            .max_by_key(|provider| provider.sequence)
            .map(|provider| provider.id.clone());
        let selected = provider_id.or(fallback);

        for frame in frames {
            let kind = frame.get("kind").and_then(Value::as_str);
            if matches!(kind, Some("hello" | "ping")) {
                if let Some(id) = selected.as_deref() {
                    let provider_count = state.providers.len();
                    if let Some(provider) = state.providers.get_mut(id) {
                        provider.last_seen_at_ms = now;
                        if kind == Some("ping") {
                            provider.has_heartbeat = true;
                        }
                        if let Some(supervised) = frame.get("supervised").and_then(Value::as_bool) {
                            provider.supervised = Some(supervised);
                        }
                        if kind == Some("hello") {
                            let rehello = provider.info.is_some();
                            let local_root = frame.get("localRoot").and_then(Value::as_str);
                            let terminals_folder = frame.get("terminalsFolder").and_then(Value::as_str);
                            if let (Some(local_root), Some(terminals_folder)) = (local_root, terminals_folder) {
                                provider.info = Some(LocalExecProviderInfo {
                                    local_root: local_root.to_string(),
                                    terminals_folder: terminals_folder.to_string(),
                                });
                            }
                            if let Some(computer_id) = frame
                                .get("computerId")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                            {
                                provider.computer_id = Some(computer_id.to_string());
                            }
                            if let Some(label) = frame
                                .get("label")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                            {
                                provider.label = Some(label.to_string());
                            }
                            if let Some(variant) = frame
                                .get("variant")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                            {
                                provider.variant = Some(variant.to_string());
                            }
                            provider_reports.push(LocalExecProviderLifecycleReport::Hello {
                                provider_id: provider.id.clone(),
                                provider_count,
                                hello_delay_ms: now.saturating_sub(provider.registered_at_ms),
                                computer_id_present: provider.computer_id.is_some(),
                                rehello,
                                supervised: provider.supervised,
                                variant: bounded_local_exec_variant(provider.variant.as_deref())
                                    .map(str::to_string),
                            });
                        }
                    }
                }
                continue;
            }

            if let Some(request_id) = frame.get("requestId").and_then(Value::as_str) {
                if let Some(sender) = state.pending.get(request_id) {
                    let _ = sender.send(frame.clone());
                }
            }
        }
        drop(state);
        for report in provider_reports {
            self.report_provider(report);
        }
    }

    pub fn retire_approval(&self, approval_id: &str) {
        let request_id = (self.random_id)();
        let frame = json!({
            "kind": "retire-approval",
            "requestId": request_id,
            "approvalId": approval_id,
        });
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for provider in state.providers.values() {
            let _ = provider.send.send(frame.clone());
        }
    }

    pub fn request(
        &self,
        frame: Value,
        computer_id: Option<&str>,
    ) -> Result<LocalExecRequest, SandLocalExecError> {
        self.request_with_before_send(frame, computer_id, |_| Ok(()))
    }

    pub fn request_with_before_send<F>(
        &self,
        mut frame: Value,
        computer_id: Option<&str>,
        before_send: F,
    ) -> Result<LocalExecRequest, SandLocalExecError>
    where
        F: FnOnce(LocalExecCancellationHandle) -> Result<(), SandLocalExecError>,
    {
        let now = (self.now_ms)();
        let request_id = (self.random_id)();
        let (send, receiver) = mpsc::channel();
        let provider = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let provider = self
                .best_provider(&state, now, computer_id)
                .cloned()
                .ok_or_else(|| self.unavailable_error(&state, now, computer_id))?;
            state.pending.insert(request_id.clone(), send);
            provider
        };

        let Some(object) = frame.as_object_mut() else {
            self.finish_request(&request_id);
            return Err(SandLocalExecError::new(
                "local-exec request frame must be a JSON object",
            ));
        };
        object.insert("requestId".into(), Value::String(request_id.clone()));
        let cancel_sent = Arc::new(AtomicBool::new(false));
        if let Err(error) = before_send(LocalExecCancellationHandle {
            request_id: request_id.clone(),
            provider_send: provider.send.clone(),
            cancel_sent: Arc::clone(&cancel_sent),
        }) {
            self.finish_request(&request_id);
            return Err(error);
        }
        if provider.send.send(frame).is_err() {
            self.finish_request(&request_id);
            return Err(SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE));
        }

        Ok(LocalExecRequest {
            bridge: self.clone(),
            request_id,
            provider_send: provider.send,
            receiver,
            closed: false,
            cancel_sent,
        })
    }

    fn finish_request(&self, request_id: &str) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .remove(request_id);
    }

    fn report_provider(&self, report: LocalExecProviderLifecycleReport) {
        if let Some(reporter) = &self.provider_reporter {
            reporter(report);
        }
    }

    fn report_refusal(&self, report: LocalExecRefusalReport) {
        if let Some(reporter) = &self.refusal_reporter {
            reporter(report);
        }
    }

    fn refusal_report(
        &self,
        state: &BridgeState,
        now_ms: u64,
        computer_id: Option<&str>,
        site: &str,
        agent_id: Option<&str>,
    ) -> LocalExecRefusalReport {
        let live_provider_count = state
            .providers
            .values()
            .filter(|provider| provider.live(now_ms))
            .count();
        let cause = if state.providers.is_empty() {
            LocalExecRefusalCause::NoProviders
        } else if computer_id.is_some_and(|id| {
            !state.providers.values().any(|provider| provider.computer_id() == id)
        }) {
            LocalExecRefusalCause::ComputerUnknown
        } else {
            LocalExecRefusalCause::StaleHeartbeat
        };
        LocalExecRefusalReport {
            cause,
            site: site.to_string(),
            conversation_id: agent_id.map(str::to_string),
            provider_count: state.providers.len(),
            live_provider_count,
            ever_registered: state.ever_registered,
            empty_for_ms: (cause == LocalExecRefusalCause::NoProviders)
                .then(|| now_ms.saturating_sub(state.empty_since_ms)),
        }
    }

    fn unavailable_error(
        &self,
        state: &BridgeState,
        now_ms: u64,
        computer_id: Option<&str>,
    ) -> SandLocalExecError {
        if state.providers.is_empty() {
            return SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE);
        }
        if self.best_provider(state, now_ms, computer_id).is_some() {
            return SandLocalExecError::new("local-exec provider unexpectedly available");
        }

        let candidate = state
            .providers
            .values()
            .filter(|provider| computer_id.is_none_or(|id| provider.computer_id() == id))
            .max_by(|left, right| {
                left.rank()
                    .cmp(&right.rank())
                    .then_with(|| left.last_seen_at_ms.cmp(&right.last_seen_at_ms))
                    .then_with(|| left.sequence.cmp(&right.sequence))
            });

        match (computer_id, candidate) {
            (Some(_), None) => SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE),
            (_, Some(provider)) => SandLocalExecError::new(
                sand_computer_unavailable_message(Some(provider.label())),
            ),
            (None, None) => SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE),
        }
    }

    fn best_provider<'a>(
        &self,
        state: &'a BridgeState,
        now_ms: u64,
        computer_id: Option<&str>,
    ) -> Option<&'a Provider> {
        state
            .providers
            .values()
            .filter(|provider| provider.live(now_ms))
            .filter(|provider| computer_id.is_none_or(|id| provider.computer_id() == id))
            .max_by(|left, right| {
                left.rank()
                    .cmp(&right.rank())
                    .then_with(|| left.last_seen_at_ms.cmp(&right.last_seen_at_ms))
                    .then_with(|| left.sequence.cmp(&right.sequence))
            })
    }
}

pub struct LocalExecProviderRegistration {
    bridge: SandLocalExecBridge,
    provider_id: Option<String>,
}

impl Drop for LocalExecProviderRegistration {
    fn drop(&mut self) {
        if let Some(provider_id) = self.provider_id.take() {
            self.bridge.detach_provider(&provider_id);
        }
    }
}

#[derive(Clone)]
pub struct LocalExecCancellationHandle {
    request_id: String,
    provider_send: Sender<Value>,
    cancel_sent: Arc<AtomicBool>,
}

impl LocalExecCancellationHandle {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn cancel(&self) -> bool {
        if self
            .cancel_sent
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if self.provider_send.send(json!({
            "kind": "cancel",
            "requestId": self.request_id,
        })).is_ok() {
            return true;
        }
        // A failed channel send is not delivery. Leave the one-shot gate open
        // so callers can distinguish transport failure from an accepted send.
        self.cancel_sent.store(false, Ordering::Release);
        false
    }
}

pub struct LocalExecRequest {
    bridge: SandLocalExecBridge,
    request_id: String,
    provider_send: Sender<Value>,
    receiver: Receiver<Value>,
    closed: bool,
    cancel_sent: Arc<AtomicBool>,
}

impl LocalExecRequest {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn cancellation_handle(&self) -> LocalExecCancellationHandle {
        LocalExecCancellationHandle {
            request_id: self.request_id.clone(),
            provider_send: self.provider_send.clone(),
            cancel_sent: Arc::clone(&self.cancel_sent),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Value, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn recv_response(&self) -> Result<Value, SandLocalExecError> {
        self.recv_timeout(Duration::from_millis(SAND_LOCAL_EXEC_RESPONSE_TIMEOUT_MS))
            .map_err(|_| SandLocalExecError::new("local-exec response timed out"))
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.bridge.finish_request(&self.request_id);
        let _ = self.cancellation_handle().cancel();
    }
}

impl Drop for LocalExecRequest {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod cancellation_delivery_tests {
    use super::*;

    #[test]
    fn disconnected_provider_is_not_reported_as_cancelled_delivery() {
        let (send, receive) = mpsc::channel::<Value>();
        drop(receive);
        let handle = LocalExecCancellationHandle {
            request_id: "request-disconnected".into(),
            provider_send: send,
            cancel_sent: Arc::new(AtomicBool::new(false)),
        };

        assert!(!handle.cancel());
        assert!(!handle.cancel_sent.load(Ordering::Acquire));
    }
}
