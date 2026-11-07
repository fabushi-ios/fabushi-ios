use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex, Weak,
    mpsc::{self, RecvTimeoutError, Sender},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::extensions::telemetry::webauthn_proxy_telemetry::{
    WebAuthnFailureCause, WebAuthnProxyReport,
};

pub const SAND_WEBAUTHN_LIVENESS_WINDOW_MS: u64 = 30_000;
pub const SAND_WEBAUTHN_CEREMONY_TIMEOUT_MS: u64 = 120_000;
pub const SAND_NO_WEBAUTHN_MACHINE_MESSAGE: &str =
    "Your computer isn't connected right now, so the security key can't be reached. Open Fabushi on the machine your key is plugged into and try again.";
pub const SAND_WEBAUTHN_MACHINE_UNAVAILABLE_MESSAGE: &str =
    "Your computer looks disconnected, so the security key can't be reached. Reconnect it and try again.";
pub const SAND_WEBAUTHN_CEREMONY_TIMEOUT_MESSAGE: &str =
    "The security key ceremony timed out before it was completed.";

pub type WebAuthnProxyReportSink = Arc<dyn Fn(WebAuthnProxyReport) + Send + Sync>;

#[derive(Clone)]
pub struct SandWebAuthnBridgeOptions {
    pub ceremony_timeout: Duration,
    pub liveness_window: Duration,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub create_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub report: WebAuthnProxyReportSink,
}

impl SandWebAuthnBridgeOptions {
    pub fn production(report: WebAuthnProxyReportSink) -> Self {
        Self {
            ceremony_timeout: Duration::from_millis(SAND_WEBAUTHN_CEREMONY_TIMEOUT_MS),
            liveness_window: Duration::from_millis(SAND_WEBAUTHN_LIVENESS_WINDOW_MS),
            now_ms: Arc::new(system_now_ms),
            create_id: Arc::new(|| Uuid::new_v4().to_string()),
            report,
        }
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopStage {
    stage: &'static str,
    outcome: &'static str,
}

fn parse_desktop_stage(frame: &Value) -> Option<DesktopStage> {
    let stage = frame.get("stage")?.as_str()?;
    let outcome = frame.get("outcome")?.as_str()?;
    match (stage, outcome) {
        ("grant", "ok") => Some(DesktopStage { stage: "grant", outcome: "ok" }),
        ("grant", "declined") => Some(DesktopStage { stage: "grant", outcome: "declined" }),
        ("grant", "failed") => Some(DesktopStage { stage: "grant", outcome: "failed" }),
        ("sign", "ok") => Some(DesktopStage { stage: "sign", outcome: "ok" }),
        ("sign", "failed") => Some(DesktopStage { stage: "sign", outcome: "failed" }),
        _ => None,
    }
}

fn stage_cause(stage: &DesktopStage) -> Option<WebAuthnFailureCause> {
    match stage.outcome {
        "declined" => Some(WebAuthnFailureCause::ConsentDeclined),
        "failed" if stage.stage == "sign" => Some(WebAuthnFailureCause::SignFailed),
        "failed" => Some(WebAuthnFailureCause::DesktopFailed),
        _ => None,
    }
}

pub fn sand_webauthn_origin_class(origin: &str) -> &'static str {
    let Ok(url) = Url::parse(origin) else {
        return "external";
    };
    let Some(hostname) = url.host_str() else {
        return "external";
    };
    if hostname == "cursor.com" {
        "cursor_com"
    } else if hostname.ends_with(".cursor.com") {
        "subdomain"
    } else {
        "external"
    }
}

#[derive(Debug, Clone)]
struct Funnel {
    request_id: String,
    origin_class: String,
    ceremony_kind: String,
    started_at: Instant,
}

#[derive(Debug, Clone)]
enum Settlement {
    Success { credential_json: Value },
    Failure {
        name: String,
        message: String,
        code: Option<String>,
    },
}

struct Pending {
    settle: Sender<Settlement>,
    funnel: Funnel,
    last_stage: Option<DesktopStage>,
}

#[derive(Clone)]
struct Provider {
    id: String,
    send: Sender<Value>,
    last_seen_at_ms: u64,
    has_heartbeat: bool,
    computer_id: Option<String>,
    label: Option<String>,
    ordinal: u64,
}

#[derive(Default)]
struct BridgeState {
    providers: BTreeMap<String, Provider>,
    pending: BTreeMap<String, Pending>,
    next_provider_ordinal: u64,
}

struct BridgeInner {
    options: SandWebAuthnBridgeOptions,
    state: Mutex<BridgeState>,
}

#[derive(Clone)]
pub struct SandWebAuthnBridge {
    inner: Arc<BridgeInner>,
}

#[derive(Debug, Error)]
pub enum SandWebAuthnBridgeError {
    #[error("WebAuthn provider disconnected before ceremony dispatch")]
    ProviderDisconnected,
    #[error("WebAuthn ceremony settlement channel closed")]
    SettlementChannelClosed,
}

pub struct WebAuthnProviderRegistration {
    provider_id: String,
    inner: Weak<BridgeInner>,
}

impl Drop for WebAuthnProviderRegistration {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.providers.remove(&self.provider_id);
        }
    }
}

impl SandWebAuthnBridge {
    pub fn new(options: SandWebAuthnBridgeOptions) -> Self {
        Self {
            inner: Arc::new(BridgeInner {
                options,
                state: Mutex::new(BridgeState::default()),
            }),
        }
    }

    pub fn production(report: WebAuthnProxyReportSink) -> Self {
        Self::new(SandWebAuthnBridgeOptions::production(report))
    }

    pub fn register_provider(
        &self,
        send: Sender<Value>,
    ) -> WebAuthnProviderRegistration {
        let provider_id = (self.inner.options.create_id)();
        let now = (self.inner.options.now_ms)();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let ordinal = state.next_provider_ordinal;
            state.next_provider_ordinal = state.next_provider_ordinal.saturating_add(1);
            state.providers.insert(
                provider_id.clone(),
                Provider {
                    id: provider_id.clone(),
                    send: send.clone(),
                    last_seen_at_ms: now,
                    has_heartbeat: false,
                    computer_id: None,
                    label: None,
                    ordinal,
                },
            );
        }
        let _ = send.send(json!({ "kind": "welcome", "providerId": provider_id }));
        WebAuthnProviderRegistration {
            provider_id,
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub fn provider_counts(&self) -> (usize, usize) {
        let state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = (self.inner.options.now_ms)();
        let live = state
            .providers
            .values()
            .filter(|provider| self.provider_is_live(provider, now))
            .count();
        (state.providers.len(), live)
    }

    pub fn submit_responses(&self, batch: Value) {
        let provider_id = batch
            .get("providerId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if let Some(provider_id) = provider_id.as_deref() {
            let now = (self.inner.options.now_ms)();
            let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(provider) = state.providers.get_mut(provider_id) {
                provider.last_seen_at_ms = now;
            }
        }

        let Some(frames) = batch.get("frames").and_then(Value::as_array) else {
            return;
        };
        for frame in frames {
            match frame.get("kind").and_then(Value::as_str).unwrap_or("") {
                "hello" => {
                    let Some(provider_id) = provider_id.as_deref() else {
                        continue;
                    };
                    let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(provider) = state.providers.get_mut(provider_id) {
                        if let Some(computer_id) = frame.get("computerId").and_then(Value::as_str) {
                            provider.computer_id = Some(computer_id.to_string());
                        }
                        if let Some(label) = frame.get("label").and_then(Value::as_str) {
                            provider.label = Some(label.to_string());
                        }
                    }
                }
                "ping" => {
                    let Some(provider_id) = provider_id.as_deref() else {
                        continue;
                    };
                    let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(provider) = state.providers.get_mut(provider_id) {
                        provider.has_heartbeat = true;
                    }
                }
                "stage" => {
                    let Some(request_id) = frame.get("requestId").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(stage) = parse_desktop_stage(frame) else {
                        continue;
                    };
                    let funnel = {
                        let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        state.pending.get_mut(request_id).map(|pending| {
                            pending.last_stage = Some(stage.clone());
                            pending.funnel.clone()
                        })
                    };
                    if let Some(funnel) = funnel {
                        self.emit(
                            &funnel,
                            stage.stage,
                            stage.outcome,
                            stage_cause(&stage),
                            None,
                            None,
                            None,
                        );
                    }
                }
                "result" => {
                    let Some(request_id) = frame.get("requestId").and_then(Value::as_str) else {
                        continue;
                    };
                    self.settle(
                        request_id,
                        Settlement::Success {
                            credential_json: frame
                                .get("credentialJson")
                                .cloned()
                                .unwrap_or(Value::Null),
                        },
                    );
                }
                "error" => {
                    let Some(request_id) = frame.get("requestId").and_then(Value::as_str) else {
                        continue;
                    };
                    self.settle(
                        request_id,
                        Settlement::Failure {
                            name: frame
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("UnknownError")
                                .to_string(),
                            message: frame
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("WebAuthn ceremony failed")
                                .to_string(),
                            code: frame
                                .get("code")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    pub fn request_ceremony(&self, ceremony: Value) -> Result<Value, SandWebAuthnBridgeError> {
        let request_id = (self.inner.options.create_id)();
        let origin_class = ceremony
            .get("origin")
            .and_then(Value::as_str)
            .map(sand_webauthn_origin_class)
            .unwrap_or("external")
            .to_string();
        let ceremony_kind = if ceremony.get("kind").and_then(Value::as_str) == Some("create") {
            "create"
        } else {
            "get"
        }
        .to_string();
        let funnel = Funnel {
            request_id: request_id.clone(),
            origin_class,
            ceremony_kind,
            started_at: Instant::now(),
        };

        let (provider, provider_count, live_provider_count) = self.select_provider();
        let Some(provider) = provider else {
            let cause = if provider_count == 0 {
                WebAuthnFailureCause::NoProvider
            } else {
                WebAuthnFailureCause::ProviderStale
            };
            self.emit(
                &funnel,
                "request",
                "failed",
                Some(cause),
                Some(provider_count as i64),
                Some(live_provider_count as i64),
                None,
            );
            self.emit(&funnel, "complete", "failed", Some(cause), None, None, None);
            let message = if provider_count == 0 {
                SAND_NO_WEBAUTHN_MACHINE_MESSAGE
            } else {
                SAND_WEBAUTHN_MACHINE_UNAVAILABLE_MESSAGE
            };
            return Ok(failure("NotAllowedError", message));
        };

        let (settle_tx, settle_rx) = mpsc::channel();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending.insert(
                request_id.clone(),
                Pending {
                    settle: settle_tx,
                    funnel: funnel.clone(),
                    last_stage: None,
                },
            );
        }

        if provider
            .send
            .send(json!({
                "kind": "ceremony",
                "requestId": request_id,
                "ceremony": ceremony,
            }))
            .is_err()
        {
            {
                let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.pending.remove(&funnel.request_id);
                state.providers.remove(&provider.id);
            }
            self.emit(
                &funnel,
                "request",
                "failed",
                Some(WebAuthnFailureCause::DispatchFailed),
                Some(provider_count as i64),
                Some(live_provider_count as i64),
                None,
            );
            self.emit(
                &funnel,
                "complete",
                "failed",
                Some(WebAuthnFailureCause::DispatchFailed),
                None,
                None,
                None,
            );
            return Err(SandWebAuthnBridgeError::ProviderDisconnected);
        }

        self.emit(
            &funnel,
            "request",
            "ok",
            None,
            Some(provider_count as i64),
            Some(live_provider_count as i64),
            None,
        );

        match settle_rx.recv_timeout(self.inner.options.ceremony_timeout) {
            Ok(Settlement::Success { credential_json }) => {
                Ok(json!({ "ok": true, "credentialJson": credential_json }))
            }
            Ok(Settlement::Failure { name, message, .. }) => Ok(failure(&name, &message)),
            Err(RecvTimeoutError::Timeout) => {
                let was_pending = {
                    let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    state.pending.remove(&funnel.request_id).is_some()
                };
                if was_pending {
                    self.emit(
                        &funnel,
                        "complete",
                        "timeout",
                        Some(WebAuthnFailureCause::Timeout),
                        None,
                        None,
                        None,
                    );
                }
                let _ = provider.send.send(json!({
                    "kind": "cancel",
                    "requestId": funnel.request_id,
                }));
                Ok(failure(
                    "NotAllowedError",
                    SAND_WEBAUTHN_CEREMONY_TIMEOUT_MESSAGE,
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                state.pending.remove(&funnel.request_id);
                Err(SandWebAuthnBridgeError::SettlementChannelClosed)
            }
        }
    }

    fn provider_is_live(&self, provider: &Provider, now_ms: u64) -> bool {
        !provider.has_heartbeat
            || now_ms.saturating_sub(provider.last_seen_at_ms)
                <= self.inner.options.liveness_window.as_millis().min(u64::MAX as u128) as u64
    }

    fn select_provider(&self) -> (Option<Provider>, usize, usize) {
        let state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = (self.inner.options.now_ms)();
        let mut best: Option<Provider> = None;
        let mut live_provider_count = 0;
        for provider in state.providers.values() {
            if !self.provider_is_live(provider, now) {
                continue;
            }
            live_provider_count += 1;
            let replace = best.as_ref().is_none_or(|current| {
                provider.last_seen_at_ms > current.last_seen_at_ms
                    || (provider.last_seen_at_ms == current.last_seen_at_ms
                        && provider.ordinal < current.ordinal)
            });
            if replace {
                best = Some(provider.clone());
            }
        }
        (best, state.providers.len(), live_provider_count)
    }

    fn settle(&self, request_id: &str, settlement: Settlement) {
        let pending = {
            let mut state = self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending.remove(request_id)
        };
        let Some(pending) = pending else {
            return;
        };

        match &settlement {
            Settlement::Success { .. } => {
                self.emit(&pending.funnel, "complete", "ok", None, None, None, None);
            }
            Settlement::Failure { name, code, .. } => {
                let cause = pending
                    .last_stage
                    .as_ref()
                    .and_then(stage_cause)
                    .unwrap_or(WebAuthnFailureCause::DesktopFailed);
                self.emit(
                    &pending.funnel,
                    "complete",
                    "failed",
                    Some(cause),
                    None,
                    None,
                    Some((name.as_str(), code.as_deref())),
                );
            }
        }
        let _ = pending.settle.send(settlement);
    }

    fn emit(
        &self,
        funnel: &Funnel,
        stage: &str,
        outcome: &str,
        cause: Option<WebAuthnFailureCause>,
        provider_count: Option<i64>,
        live_provider_count: Option<i64>,
        raw_error: Option<(&str, Option<&str>)>,
    ) {
        let (raw_dom_error_name, raw_sign_error_class) = match raw_error {
            Some((name, code)) => (Some(name.to_string()), code.map(str::to_string)),
            None => (None, None),
        };
        (self.inner.options.report)(WebAuthnProxyReport {
            outcome: outcome.to_string(),
            stage: stage.to_string(),
            origin_class: funnel.origin_class.clone(),
            ceremony_kind: funnel.ceremony_kind.clone(),
            request_id: funnel.request_id.clone(),
            elapsed_ms: funnel.started_at.elapsed().as_secs_f64() * 1000.0,
            provider_count,
            live_provider_count,
            cause,
            raw_dom_error_name,
            raw_sign_error_class,
        });
    }
}

fn failure(name: &str, message: &str) -> Value {
    json!({
        "ok": false,
        "error": {
            "name": name,
            "message": message,
        }
    })
}
