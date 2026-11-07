use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::cursor_backend::{create_cursor_checksum, resolve_sand_ghost_mode_header};
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version, system_now_ms,
};
use crate::extensions::auth::extension::HostAuthExtension;

use super::HostTelemetryProjection;

pub const BOX_HELP_EVENT: &str = "sand.box_help";
pub const SAND_LOG_KEY: &str = "sand";
pub const HOST_BUILT_AT_MS: &str = "1786556440000";
pub const STRUCTURED_LOG_SUBMIT_PATH: &str = "aiserver.v1.AnalyticsService/SubmitLogs";
pub const STRUCTURED_LOG_SUBMIT_DEADLINE: Duration = Duration::from_secs(15);
pub const HOST_IDENTITY_HOLD_BACKSTOP: Duration = Duration::from_secs(90);
pub const STRUCTURED_LOG_FLUSH_INTERVAL: Duration = Duration::from_secs(3);
pub const STRUCTURED_LOG_MAX_BUFFER_SIZE: usize = 1_000;
pub const STRUCTURED_LOG_BATCH_SIZE: usize = 128;
pub const STRUCTURED_LOG_REPLAY_MAX_AGE: Duration = Duration::from_secs(17 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl StructuredLogLevel {
    fn wire_value(self) -> u8 {
        match self {
            Self::Info => 1,
            Self::Debug => 2,
            Self::Warn => 3,
            Self::Error => 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferedStructuredLog {
    pub level: StructuredLogLevel,
    pub message: String,
    pub metadata: BTreeMap<String, String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuredLogReceipt {
    pub logs_processed: usize,
    pub logs_dropped: usize,
}

pub trait StructuredLogBackend: Send + Sync {
    fn submit(&self, logs: &[BufferedStructuredLog]) -> Result<StructuredLogReceipt, String>;
}

pub struct CursorStructuredLogBackend {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    client: Client,
    platform_tags: BTreeMap<String, String>,
}

impl CursorStructuredLogBackend {
    pub fn new(
        backend_url: String,
        auth: Arc<HostAuthExtension>,
        mut platform_tags: BTreeMap<String, String>,
    ) -> Result<Self, String> {
        platform_tags.retain(|_, value| !value.is_empty());
        let client = Client::builder()
            .connect_timeout(STRUCTURED_LOG_SUBMIT_DEADLINE)
            .timeout(STRUCTURED_LOG_SUBMIT_DEADLINE)
            .build()
            .map_err(|error| format!("build structured-log client: {error}"))?;
        Ok(Self {
            backend_url,
            auth,
            client,
            platform_tags,
        })
    }
}

impl StructuredLogBackend for CursorStructuredLogBackend {
    fn submit(&self, logs: &[BufferedStructuredLog]) -> Result<StructuredLogReceipt, String> {
        if logs.is_empty() {
            return Ok(StructuredLogReceipt {
                logs_processed: 0,
                logs_dropped: 0,
            });
        }
        let url = Url::parse(&self.backend_url)
            .map_err(|error| format!("invalid structured-log backend URL: {error}"))?
            .join(STRUCTURED_LOG_SUBMIT_PATH)
            .map_err(|error| format!("join structured-log endpoint: {error}"))?;
        let token = self
            .auth
            .get_access_token()
            .map_err(|error| format!("structured-log access token: {error}"))?;
        let machine_id = self
            .auth
            .get_machine_id()
            .map_err(|error| format!("structured-log machine id: {error}"))?;
        let ghost_mode = resolve_sand_ghost_mode_header(&self.backend_url, &token, &machine_id);
        let payload = json!({
            "logs": logs.iter().map(|entry| {
                let mut metadata = self.platform_tags.clone();
                metadata.extend(entry.metadata.clone());
                json!({
                    "level": entry.level.wire_value(),
                    "message": entry.message,
                    "metadata": metadata,
                    "timestamp": entry.timestamp_ms.to_string(),
                    "key": SAND_LOG_KEY,
                })
            }).collect::<Vec<_>>(),
        });
        let response = self
            .client
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("connect-protocol-version", "1")
            .header(
                "x-cursor-checksum",
                create_cursor_checksum(&machine_id, system_now_ms()),
            )
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            .header("x-ghost-mode", ghost_mode)
            .header("x-request-id", Uuid::new_v4().to_string())
            .json(&payload)
            .send()
            .map_err(|error| format!("structured-log transport: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "structured-log backend status {}",
                response.status()
            ));
        }
        let body: Value = response
            .json()
            .map_err(|error| format!("structured-log response: {error}"))?;
        let processed = body
            .get("logsProcessed")
            .or_else(|| body.get("logs_processed"))
            .and_then(Value::as_u64)
            .unwrap_or(logs.len() as u64) as usize;
        let dropped = body
            .get("logsDropped")
            .or_else(|| body.get("logs_dropped"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        if processed.saturating_add(dropped) != logs.len() {
            return Err(format!(
                "invalid structured-log receipt: processed={processed} dropped={dropped} requested={}",
                logs.len()
            ));
        }
        Ok(StructuredLogReceipt {
            logs_processed: processed,
            logs_dropped: dropped,
        })
    }
}

enum StructuredLogCommand {
    Enqueue(BufferedStructuredLog),
    SetIdentity(BTreeMap<String, String>),
    Flush(mpsc::Sender<bool>),
    Stop(mpsc::Sender<bool>),
}

pub struct ProductionStructuredLogTransport {
    sender: Mutex<Option<mpsc::Sender<StructuredLogCommand>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ProductionStructuredLogTransport {
    pub fn start(backend: Arc<dyn StructuredLogBackend>, hold_for_identity: bool) -> Arc<Self> {
        Self::start_with_tick(backend, hold_for_identity, None)
    }

    pub fn start_with_tick(
        backend: Arc<dyn StructuredLogBackend>,
        hold_for_identity: bool,
        flush_tick_listener: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Arc<Self> {
        Self::start_with_policy_and_tick(
            backend,
            hold_for_identity,
            STRUCTURED_LOG_FLUSH_INTERVAL,
            HOST_IDENTITY_HOLD_BACKSTOP,
            flush_tick_listener,
        )
    }

    pub fn start_with_policy(
        backend: Arc<dyn StructuredLogBackend>,
        hold_for_identity: bool,
        flush_interval: Duration,
        identity_hold_backstop: Duration,
    ) -> Arc<Self> {
        Self::start_with_policy_and_tick(
            backend,
            hold_for_identity,
            flush_interval,
            identity_hold_backstop,
            None,
        )
    }

    pub fn start_with_policy_and_tick(
        backend: Arc<dyn StructuredLogBackend>,
        hold_for_identity: bool,
        flush_interval: Duration,
        identity_hold_backstop: Duration,
        flush_tick_listener: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sand-host-structured-log-transport".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut held = hold_for_identity;
                let mut identity_tags = BTreeMap::<String, String>::new();
                let mut buffer = VecDeque::<BufferedStructuredLog>::new();

                let flush = |buffer: &mut VecDeque<BufferedStructuredLog>,
                             identity_tags: &BTreeMap<String, String>|
                 -> bool {
                    let now_ms = wall_clock_now_ms();
                    let max_age_ms = STRUCTURED_LOG_REPLAY_MAX_AGE.as_millis() as u64;
                    buffer.retain(|entry| now_ms.saturating_sub(entry.timestamp_ms) <= max_age_ms);
                    let mut all_delivered = true;
                    while !buffer.is_empty() {
                        let take = buffer.len().min(STRUCTURED_LOG_BATCH_SIZE);
                        let mut originals = Vec::with_capacity(take);
                        for _ in 0..take {
                            let Some(entry) = buffer.pop_front() else {
                                break;
                            };
                            originals.push(entry);
                        }
                        let batch = originals
                            .iter()
                            .cloned()
                            .map(|mut entry| {
                                let mut merged = identity_tags.clone();
                                merged.extend(entry.metadata);
                                entry.metadata = merged;
                                entry
                            })
                            .collect::<Vec<_>>();
                        match backend.submit(&batch) {
                            Ok(receipt)
                                if receipt.logs_processed + receipt.logs_dropped == batch.len() => {
                            }
                            _ => {
                                for entry in originals.into_iter().rev() {
                                    buffer.push_front(entry);
                                }
                                all_delivered = false;
                                break;
                            }
                        }
                    }
                    all_delivered
                };

                loop {
                    let timeout = if held {
                        let remaining = identity_hold_backstop.saturating_sub(started.elapsed());
                        remaining.min(flush_interval)
                    } else {
                        flush_interval
                    };
                    match rx.recv_timeout(timeout) {
                        Ok(StructuredLogCommand::Enqueue(entry)) => {
                            buffer.push_back(entry);
                            while buffer.len() > STRUCTURED_LOG_MAX_BUFFER_SIZE {
                                buffer.pop_front();
                            }
                        }
                        Ok(StructuredLogCommand::SetIdentity(tags)) => {
                            identity_tags = clean_metadata(tags);
                            held = false;
                            let _ = flush(&mut buffer, &identity_tags);
                        }
                        Ok(StructuredLogCommand::Flush(done)) => {
                            let result = if held {
                                false
                            } else {
                                flush(&mut buffer, &identity_tags)
                            };
                            let _ = done.send(result);
                        }
                        Ok(StructuredLogCommand::Stop(done)) => {
                            held = false;
                            let result = flush(&mut buffer, &identity_tags);
                            let _ = done.send(result);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if let Some(listener) = flush_tick_listener.as_ref() {
                                listener();
                            }
                            if held && started.elapsed() >= identity_hold_backstop {
                                held = false;
                            }
                            if !held {
                                let _ = flush(&mut buffer, &identity_tags);
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            held = false;
                            let _ = flush(&mut buffer, &identity_tags);
                            break;
                        }
                    }
                }
            })
            .expect("spawn structured-log transport worker");

        Arc::new(Self {
            sender: Mutex::new(Some(tx)),
            worker: Mutex::new(Some(worker)),
        })
    }

    pub fn enqueue(
        &self,
        level: StructuredLogLevel,
        message: impl Into<String>,
        metadata: BTreeMap<String, String>,
    ) -> bool {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned();
        let Some(sender) = sender else {
            return false;
        };
        sender
            .send(StructuredLogCommand::Enqueue(BufferedStructuredLog {
                level,
                message: message.into(),
                metadata: clean_metadata(metadata),
                timestamp_ms: wall_clock_now_ms(),
            }))
            .is_ok()
    }

    pub fn set_identity_tags(&self, tags: BTreeMap<String, String>) -> bool {
        self.sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|sender| sender.send(StructuredLogCommand::SetIdentity(tags)).is_ok())
    }

    pub fn flush(&self) -> bool {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned();
        let Some(sender) = sender else {
            return true;
        };
        let (done_tx, done_rx) = mpsc::channel();
        if sender.send(StructuredLogCommand::Flush(done_tx)).is_err() {
            return false;
        }
        done_rx
            .recv_timeout(STRUCTURED_LOG_SUBMIT_DEADLINE + Duration::from_secs(1))
            .unwrap_or(false)
    }

    pub fn ship_confirmed(
        &self,
        backend: &dyn StructuredLogBackend,
        level: StructuredLogLevel,
        message: impl Into<String>,
        mut metadata: BTreeMap<String, String>,
    ) -> bool {
        metadata = clean_metadata(metadata);
        backend
            .submit(&[BufferedStructuredLog {
                level,
                message: message.into(),
                metadata,
                timestamp_ms: wall_clock_now_ms(),
            }])
            .is_ok_and(|receipt| receipt.logs_processed + receipt.logs_dropped == 1)
    }

    pub fn dispose(&self) -> bool {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let result = if let Some(sender) = sender {
            let (done_tx, done_rx) = mpsc::channel();
            if sender.send(StructuredLogCommand::Stop(done_tx)).is_ok() {
                done_rx
                    .recv_timeout(STRUCTURED_LOG_SUBMIT_DEADLINE + Duration::from_secs(1))
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            true
        };
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
        result
    }
}

impl Drop for ProductionStructuredLogTransport {
    fn drop(&mut self) {
        let sender = self
            .sender
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(sender) = sender {
            let (done_tx, _done_rx) = mpsc::channel();
            let _ = sender.send(StructuredLogCommand::Stop(done_tx));
        }
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

pub fn clean_metadata(mut metadata: BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata.retain(|_, value| !value.is_empty());
    metadata
}

pub fn level_from_str(level: &str) -> StructuredLogLevel {
    match level {
        "debug" => StructuredLogLevel::Debug,
        "warn" => StructuredLogLevel::Warn,
        "error" => StructuredLogLevel::Error,
        _ => StructuredLogLevel::Info,
    }
}

pub fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub fn box_help_telemetry(report: &Value) -> HostTelemetryProjection {
    let conversation_id = report
        .get("conversationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let snapshot_captured = report
        .get("snapshotCaptured")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut metadata = BTreeMap::from([
        ("conversation_id".to_string(), conversation_id),
        (
            "snapshot_captured".to_string(),
            snapshot_captured.to_string(),
        ),
    ]);
    if let Some(reason) = report.get("reason").and_then(Value::as_str) {
        metadata.insert("meta.reason".into(), reason.to_string());
    }

    HostTelemetryProjection {
        level: Some("info"),
        event: Some(BOX_HELP_EVENT),
        metadata,
    }
}
