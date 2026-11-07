use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::cursor_backend::{create_cursor_checksum, resolve_sand_ghost_mode_header};
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version, system_now_ms,
};

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::HostExperimentsExtension;

pub const SAND_PRODUCT_ANALYTICS_GATE: &str = "sand_product_analytics";
pub const MAX_DEFERRED_ANALYTICS_EVENTS: usize = 256;
pub const ANALYTICS_BUFFER_LIMIT: usize = 200;
pub const ANALYTICS_FLUSH_INTERVAL_MS: u64 = 3_000;
pub const ANALYTICS_NORMAL_FLUSH_TIMEOUT_MS: u64 = 2_500;
pub const ANALYTICS_TRACK_EVENTS_PATH: &str = "aiserver.v1.AnalyticsService/TrackEvents";
pub const FROZEN_ANALYTICS_SERVICE_BLOB: &str =
    "4ea2919cfcc54b1c549f856188538ae032c5a522";

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAnalyticsEvent {
    pub name: String,
    pub properties: BTreeMap<String, Value>,
}

pub fn product_analytics_event(name: &str, properties: &Value) -> ProductAnalyticsEvent {
    let mut clean = BTreeMap::new();
    if let Some(object) = properties.as_object() {
        for (key, value) in object {
            if matches!(
                value,
                Value::Bool(_) | Value::Number(_) | Value::String(_)
            ) {
                clean.insert(key.clone(), value.clone());
            }
        }
    }
    ProductAnalyticsEvent {
        name: name.to_string(),
        properties: clean,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationRunAnalyticsReport {
    pub conversation_id: String,
    pub automation_id: String,
    pub trigger: String,
    pub outcome: String,
    pub is_group: bool,
    pub sent_message_count: Option<i64>,
}

pub trait AnalyticsClient: Send + Sync {
    fn track_event(&self, name: &str, properties: &Value);
}

pub trait TelemetryService: Send + Sync {
    fn report(&self, method: &'static str, report: &Value);

    fn start_turn(&self, report: &Value) { self.report("startTurn", report); }
    fn report_tool_call_error(&self, report: &Value) { self.report("reportToolCallError", report); }
    fn report_tool_call_stalled(&self, report: &Value) { self.report("reportToolCallStalled", report); }
    fn report_tool_call_started(&self, report: &Value) { self.report("reportToolCallStarted", report); }
    fn report_agent_error(&self, report: &Value) { self.report("reportAgentError", report); }
    fn report_bot_block(&self, report: &Value) { self.report("reportBotBlock", report); }
    fn report_daemon_ping(&self, report: &Value) { self.report("reportDaemonPing", report); }
    fn report_box_boot_stage(&self, report: &Value) { self.report("reportBoxBootStage", report); }
    fn report_exec_daemon_restart(&self, report: &Value) { self.report("reportExecDaemonRestart", report); }
    fn report_supervisor_restart(&self, report: &Value) { self.report("reportSupervisorRestart", report); }
    fn report_turn_interrupt(&self, report: &Value) { self.report("reportTurnInterrupt", report); }
    fn report_turn_await(&self, report: &Value) { self.report("reportTurnAwait", report); }
    fn report_turn_retry(&self, report: &Value) { self.report("reportTurnRetry", report); }
    fn report_user_message_received(&self, report: &Value) { self.report("reportUserMessageReceived", report); }
    fn report_closing_send_nudge(&self, report: &Value) { self.report("reportClosingSendNudge", report); }
    fn report_subagent_revival(&self, report: &Value) { self.report("reportSubagentRevival", report); }
    fn report_shell_revival(&self, report: &Value) { self.report("reportShellRevival", report); }
    fn report_computer_use_usage(&self, report: &Value) { self.report("reportComputerUseUsage", report); }
    fn report_ttft(&self, report: &Value) { self.report("reportTtft", report); }
    fn report_send_dispatch(&self, report: &Value) { self.report("reportSendDispatch", report); }
    fn report_queue_accepted(&self, report: &Value) { self.report("reportQueueAccepted", report); }
    fn report_queue_dequeued(&self, report: &Value) { self.report("reportQueueDequeued", report); }
    fn report_queue_watchdog(&self, report: &Value) { self.report("reportQueueWatchdog", report); }
    fn report_ack_obligation(&self, report: &Value) { self.report("reportAckObligation", report); }
    fn report_pending_wake(&self, report: &Value) { self.report("reportPendingWake", report); }
    fn report_turn_usage(&self, report: &Value) { self.report("reportTurnUsage", report); }
    fn report_turn_empty_delivery(&self, report: &Value) { self.report("reportTurnEmptyDelivery", report); }
    fn report_journal_outcome(&self, report: &Value) { self.report("reportJournalOutcome", report); }
    fn report_auto_review_expire_sweep_failed(&self, report: &Value) { self.report("reportAutoReviewExpireSweepFailed", report); }
    fn report_automation_lifecycle(&self, report: &Value) { self.report("reportAutomationLifecycle", report); }
    fn report_automation_fire_dropped(&self, report: &Value) { self.report("reportAutomationFireDropped", report); }
    fn report_automation_run(&self, report: &Value) { self.report("reportAutomationRun", report); }
}

pub struct AutomationRunAnalyticsTelemetry {
    telemetry: Arc<dyn TelemetryService>,
    analytics: Arc<dyn AnalyticsClient>,
}

impl AutomationRunAnalyticsTelemetry {
    pub fn new(
        telemetry: Arc<dyn TelemetryService>,
        analytics: Arc<dyn AnalyticsClient>,
    ) -> Self {
        Self { telemetry, analytics }
    }
}

impl TelemetryService for AutomationRunAnalyticsTelemetry {
    fn report(&self, method: &'static str, report: &Value) {
        if method == "reportAutomationRun" {
            let sent_message_count = report
                .get("sentMessageCount")
                .or_else(|| report.get("sent_message_count"))
                .and_then(Value::as_i64);
            let mut properties = serde_json::Map::from_iter([
                (
                    "agent_id".into(),
                    report.get("conversationId")
                        .or_else(|| report.get("conversation_id"))
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "automation_id".into(),
                    report.get("automationId")
                        .or_else(|| report.get("automation_id"))
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                (
                    "trigger".into(),
                    report.get("trigger").cloned().unwrap_or(Value::Null),
                ),
                (
                    "outcome".into(),
                    report.get("outcome").cloned().unwrap_or(Value::Null),
                ),
                (
                    "is_group".into(),
                    report.get("isGroup")
                        .or_else(|| report.get("is_group"))
                        .cloned()
                        .unwrap_or(Value::Bool(false)),
                ),
            ]);
            if let Some(count) = sent_message_count {
                properties.insert("sent_message_count".into(), json!(count));
            }
            self.analytics
                .track_event("sand.automation.run", &Value::Object(properties));
        }
        self.telemetry.report(method, report);
    }
}

#[derive(Debug, Clone)]
struct QueuedAnalyticsEvent {
    event: ProductAnalyticsEvent,
    timestamp_ms: u64,
}

enum AnalyticsWorkerCommand {
    Event(QueuedAnalyticsEvent),
    Stop,
}

struct AnalyticsBackendTransport {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    client: Client,
}

impl AnalyticsBackendTransport {
    fn new(
        backend_url: String,
        auth: Arc<HostAuthExtension>,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_millis(ANALYTICS_NORMAL_FLUSH_TIMEOUT_MS))
            .timeout(Duration::from_millis(ANALYTICS_NORMAL_FLUSH_TIMEOUT_MS))
            .build()
            .map_err(|error| format!("build product analytics client: {error}"))?;
        Ok(Self { backend_url, auth, client })
    }

    fn send(&self, queued: &[QueuedAnalyticsEvent]) -> Result<(), String> {
        let url = Url::parse(&self.backend_url)
            .map_err(|error| format!("invalid analytics backend URL: {error}"))?
            .join(ANALYTICS_TRACK_EVENTS_PATH)
            .map_err(|error| format!("join analytics endpoint: {error}"))?;
        let token = self.auth
            .get_access_token()
            .map_err(|error| format!("product analytics access token: {error}"))?;
        let machine_id = self.auth
            .get_machine_id()
            .map_err(|error| format!("product analytics machine id: {error}"))?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&self.backend_url, &token, &machine_id);
        let events = queued
            .iter()
            .map(|queued| {
                let event_data = queued
                    .event
                    .properties
                    .iter()
                    .map(|(key, value)| {
                        let encoded = match value {
                            Value::String(value) => json!({ "stringValue": value }),
                            Value::Bool(value) => json!({ "boolValue": value }),
                            Value::Number(value) => {
                                json!({ "doubleValue": value.as_f64().unwrap_or(0.0) })
                            }
                            _ => Value::Null,
                        };
                        (key.clone(), encoded)
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({
                    "eventName": queued.event.name,
                    "eventData": event_data,
                    "timestamp": queued.timestamp_ms.to_string(),
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({ "events": events });
        let response = self.client
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("connect-protocol-version", "1")
            .header("x-cursor-checksum", create_cursor_checksum(&machine_id, system_now_ms()))
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            .header("x-ghost-mode", ghost_mode)
            .header("x-request-id", Uuid::new_v4().to_string())
            .json(&payload)
            .send()
            .map_err(|error| format!("product analytics transport: {error}"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("product analytics backend status {}", response.status()))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnalyticsStateKind {
    Disabled,
    Deferred,
    Active,
}

pub struct ProductionAnalyticsRuntime {
    state: Arc<Mutex<AnalyticsStateKind>>,
    deferred: Arc<Mutex<VecDeque<QueuedAnalyticsEvent>>>,
    base_properties: BTreeMap<String, Value>,
    last_active_day_keys: Mutex<BTreeMap<String, String>>,
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    sender: Mutex<Option<mpsc::Sender<AnalyticsWorkerCommand>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    gate_stop: Mutex<Option<StopSubscription>>,
}

impl ProductionAnalyticsRuntime {
    pub fn start(
        backend_url: String,
        auth: Arc<HostAuthExtension>,
        experiments: Arc<HostExperimentsExtension>,
    ) -> Arc<Self> {
        let opted_out = std::env::var("SAND_DISABLE_TELEMETRY").as_deref() == Ok("1")
            || std::env::var("SAND_DISABLE_ANALYTICS").as_deref() == Ok("1");
        let state = Arc::new(Mutex::new(if opted_out {
            AnalyticsStateKind::Disabled
        } else {
            AnalyticsStateKind::Deferred
        }));
        let deferred = Arc::new(Mutex::new(VecDeque::new()));

        let flavor = match sand_box_namespace() {
            "dev" => "sand-dev",
            "lab" => "sand-lab",
            _ => "sand",
        };
        let runtime = Arc::new(Self {
            state,
            deferred,
            base_properties: BTreeMap::from([
                ("client".into(), json!("sand")),
                ("sand_version".into(), json!(sand_client_version())),
                ("flavor".into(), json!(flavor)),
                ("os".into(), json!(std::env::consts::OS)),
                ("arch".into(), json!(std::env::consts::ARCH)),
                ("host_in_box".into(), json!(true)),
            ]),
            last_active_day_keys: Mutex::new(BTreeMap::new()),
            backend_url,
            auth,
            sender: Mutex::new(None),
            worker: Mutex::new(None),
            gate_stop: Mutex::new(None),
        });
        if opted_out {
            return runtime;
        }

        let gate = experiments.get_feature_gate_property(SAND_PRODUCT_ANALYTICS_GATE);
        let weak = Arc::downgrade(&runtime);
        let stop = gate.subscribe(Arc::new(move |enabled| {
            if enabled {
                if let Some(runtime) = weak.upgrade() {
                    runtime.activate();
                }
            }
        }));
        *runtime.gate_stop.lock().unwrap_or_else(|p| p.into_inner()) = Some(stop);
        if gate.get() {
            runtime.activate();
        }
        runtime
    }

    fn activate(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if *state != AnalyticsStateKind::Deferred {
            return;
        }

        let transport = match AnalyticsBackendTransport::new(
            self.backend_url.clone(),
            Arc::clone(&self.auth),
        ) {
            Ok(transport) => transport,
            Err(error) => {
                eprintln!("[sand-analytics] failed to enable; disabling ({error})");
                self.deferred
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clear();
                *state = AnalyticsStateKind::Disabled;
                drop(state);
                if let Some(stop) = self
                    .gate_stop
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    stop();
                }
                return;
            }
        };

        let (tx, rx) = mpsc::channel();
        let worker = match thread::Builder::new()
            .name("sand-product-analytics".into())
            .spawn(move || {
                let mut buffer = Vec::<QueuedAnalyticsEvent>::new();
                let mut flush_deadline: Option<Instant> = None;

                let flush = |buffer: &mut Vec<QueuedAnalyticsEvent>| {
                    if buffer.is_empty() {
                        return;
                    }
                    match transport.send(buffer) {
                        Ok(()) => buffer.clear(),
                        Err(error) => {
                            eprintln!("[sand-analytics] {error}");
                            if buffer.len() > ANALYTICS_BUFFER_LIMIT {
                                let overflow = buffer.len() - ANALYTICS_BUFFER_LIMIT;
                                buffer.drain(..overflow);
                            }
                        }
                    }
                };

                loop {
                    let command = if let Some(deadline) = flush_deadline {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        match rx.recv_timeout(remaining) {
                            Ok(command) => Some(command),
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                flush(&mut buffer);
                                flush_deadline = None;
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => None,
                        }
                    } else {
                        rx.recv().ok()
                    };

                    match command {
                        Some(AnalyticsWorkerCommand::Event(event)) => {
                            buffer.push(event);
                            if buffer.len() >= ANALYTICS_BUFFER_LIMIT {
                                flush(&mut buffer);
                                flush_deadline = if buffer.is_empty() {
                                    None
                                } else {
                                    Some(Instant::now() + Duration::from_millis(ANALYTICS_FLUSH_INTERVAL_MS))
                                };
                            } else if flush_deadline.is_none() {
                                flush_deadline =
                                    Some(Instant::now() + Duration::from_millis(ANALYTICS_FLUSH_INTERVAL_MS));
                            }
                        }
                        Some(AnalyticsWorkerCommand::Stop) | None => {
                            flush(&mut buffer);
                            break;
                        }
                    }
                }
            }) {
            Ok(worker) => worker,
            Err(error) => {
                eprintln!("[sand-analytics] failed to enable; disabling ({error})");
                self.deferred
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clear();
                *state = AnalyticsStateKind::Disabled;
                drop(state);
                if let Some(stop) = self
                    .gate_stop
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    stop();
                }
                return;
            }
        };

        *self.sender.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx.clone());
        *self.worker.lock().unwrap_or_else(|p| p.into_inner()) = Some(worker);
        *state = AnalyticsStateKind::Active;
        drop(state);

        let mut deferred = self.deferred.lock().unwrap_or_else(|p| p.into_inner());
        while let Some(event) = deferred.pop_front() {
            let _ = tx.send(AnalyticsWorkerCommand::Event(event));
        }
        drop(deferred);
        if let Some(stop) = self.gate_stop.lock().unwrap_or_else(|p| p.into_inner()).take() {
            stop();
        }
    }

    pub fn can_record_events(&self) -> bool {
        !matches!(
            *self.state.lock().unwrap_or_else(|p| p.into_inner()),
            AnalyticsStateKind::Disabled
        )
    }


    pub fn mark_active(&self, reason: &str) {
        if !self.can_record_events() {
            return;
        }
        let day_key = chrono::Utc::now().format("%Y-%m-%d").to_string();
        {
            let mut keys = self
                .last_active_day_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if keys.get(reason).is_some_and(|value| value == &day_key) {
                return;
            }
            keys.insert(reason.to_string(), day_key);
        }
        self.track(product_analytics_event(
            "sand.app.active",
            &json!({ "reason": reason }),
        ));
    }

    pub fn track(&self, mut event: ProductAnalyticsEvent) {
        let mut enriched = self.base_properties.clone();
        enriched.extend(event.properties);
        event.properties = enriched;
        let queued = QueuedAnalyticsEvent {
            event,
            timestamp_ms: wall_clock_now_ms(),
        };
        match *self.state.lock().unwrap_or_else(|p| p.into_inner()) {
            AnalyticsStateKind::Disabled => {}
            AnalyticsStateKind::Deferred => {
                let mut deferred = self.deferred.lock().unwrap_or_else(|p| p.into_inner());
                if deferred.len() < MAX_DEFERRED_ANALYTICS_EVENTS {
                    deferred.push_back(queued);
                }
            }
            AnalyticsStateKind::Active => {
                if let Some(sender) = self
                    .sender
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                    .cloned()
                {
                    let _ = sender.send(AnalyticsWorkerCommand::Event(queued));
                }
            }
        }
    }
}

impl Drop for ProductionAnalyticsRuntime {
    fn drop(&mut self) {
        if let Ok(stop) = self.gate_stop.get_mut() {
            if let Some(stop) = stop.take() {
                stop();
            }
        }
        if let Ok(sender) = self.sender.get_mut() {
            if let Some(sender) = sender.take() {
                let _ = sender.send(AnalyticsWorkerCommand::Stop);
            }
        }
        if let Ok(worker) = self.worker.get_mut() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
