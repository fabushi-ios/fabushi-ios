use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::gateway::gateway_client::{SSE_CONNECT_TIMEOUT_MS, SSE_STALL_TIMEOUT_MS};
use crate::gateway::gateway_reachability::ReachabilityOutcome;
use crate::gateway::gateway_request_dispatcher::GatewayDispatchError;
use crate::gateway::host_supervisor::GatewayConnection;
use crate::gateway::http_transport::{parse_gateway_http_base, read_http_response};
use crate::gateway::sse_block_decoder::SseBlockDecoder;

use crate::webauthn::{
    ApprovedWebAuthnConsent, WebAuthnCeremony, WebAuthnSigner, WebAuthnSignerError,
    WebAuthnSignerResult,
};
use crate::webauthn::signer::{
    SpawnedWebAuthnSigner, WebAuthnPinRequest, WebAuthnSignCancellation,
};

#[derive(Debug, Clone, PartialEq)]
pub enum WebAuthnRequestFrame {
    Welcome { provider_id: String },
    Ceremony {
        request_id: String,
        ceremony: WebAuthnCeremony,
    },
    Cancel { request_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebAuthnResponseFrame {
    Hello {
        computer_id: Option<String>,
        label: Option<String>,
    },
    Stage {
        request_id: String,
        stage: &'static str,
        outcome: &'static str,
    },
    Result {
        request_id: String,
        credential_json: Value,
    },
    Error {
        request_id: String,
        error: WebAuthnSignerError,
    },
    Ping,
}

#[derive(Debug)]
pub struct WebAuthnProvider<S: WebAuthnSigner> {
    signer: S,
    computer_id: Option<String>,
    label: Option<String>,
    provider_id: Option<String>,
    in_flight: HashSet<String>,
}

impl<S: WebAuthnSigner> WebAuthnProvider<S> {
    pub fn new(
        signer: S,
        computer_id: Option<String>,
        label: Option<String>,
    ) -> Self {
        Self {
            signer,
            computer_id,
            label,
            provider_id: None,
            in_flight: HashSet::new(),
        }
    }

    pub fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    pub fn reset_transport(&mut self) {
        self.provider_id = None;
        self.in_flight.clear();
    }

    pub fn heartbeat(&self) -> WebAuthnResponseFrame {
        WebAuthnResponseFrame::Ping
    }

    pub fn handle_frame(
        &mut self,
        frame: WebAuthnRequestFrame,
        consent: Option<ApprovedWebAuthnConsent>,
    ) -> Vec<WebAuthnResponseFrame> {
        match frame {
            WebAuthnRequestFrame::Welcome { provider_id } => {
                self.provider_id = Some(provider_id);
                vec![WebAuthnResponseFrame::Hello {
                    computer_id: self.computer_id.clone(),
                    label: self.label.clone(),
                }]
            }
            WebAuthnRequestFrame::Cancel { request_id } => {
                self.in_flight.remove(&request_id);
                Vec::new()
            }
            WebAuthnRequestFrame::Ceremony {
                request_id,
                ceremony,
            } => self.run_ceremony(request_id, ceremony, consent),
        }
    }

    fn run_ceremony(
        &mut self,
        request_id: String,
        ceremony: WebAuthnCeremony,
        consent: Option<ApprovedWebAuthnConsent>,
    ) -> Vec<WebAuthnResponseFrame> {
        if request_id.trim().is_empty() || !self.in_flight.insert(request_id.clone()) {
            return vec![WebAuthnResponseFrame::Error {
                request_id,
                error: WebAuthnSignerError {
                    name: "InvalidStateError".into(),
                    code: Some("duplicate_request".into()),
                    message: "WebAuthn request id is empty or already active".into(),
                },
            }];
        }

        if consent.as_ref().is_some_and(|value| !value.approved) {
            self.in_flight.remove(&request_id);
            return vec![
                WebAuthnResponseFrame::Stage {
                    request_id: request_id.clone(),
                    stage: "grant",
                    outcome: "declined",
                },
                WebAuthnResponseFrame::Error {
                    request_id,
                    error: WebAuthnSignerError {
                        name: "NotAllowedError".into(),
                        code: Some("consent_declined".into()),
                        message: "The security key request was declined on this computer".into(),
                    },
                },
            ];
        }

        let mut frames = Vec::new();
        if consent.is_some() {
            frames.push(WebAuthnResponseFrame::Stage {
                request_id: request_id.clone(),
                stage: "grant",
                outcome: "ok",
            });
        }

        match self.signer.sign(&ceremony, consent.as_ref()) {
            WebAuthnSignerResult::Success { credential_json } => {
                frames.push(WebAuthnResponseFrame::Stage {
                    request_id: request_id.clone(),
                    stage: "sign",
                    outcome: "ok",
                });
                frames.push(WebAuthnResponseFrame::Result {
                    request_id: request_id.clone(),
                    credential_json,
                });
            }
            WebAuthnSignerResult::Failed { error } => {
                frames.push(WebAuthnResponseFrame::Stage {
                    request_id: request_id.clone(),
                    stage: "sign",
                    outcome: "failed",
                });
                frames.push(WebAuthnResponseFrame::Error {
                    request_id: request_id.clone(),
                    error,
                });
            }
        }
        self.in_flight.remove(&request_id);
        frames
    }
}


pub const GATEWAY_WEBAUTHN_REQUESTS_PATH: &str = "/webauthn/requests";
pub const GATEWAY_WEBAUTHN_RESPONSES_PATH: &str = "/webauthn/responses";
pub const WEBAUTHN_HEARTBEAT_INTERVAL_MS: u64 = 10_000;

fn webauthn_io_outcome(error: &std::io::Error) -> ReachabilityOutcome {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => ReachabilityOutcome::Refused,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ReachabilityOutcome::Timeout,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::AddrNotAvailable => ReachabilityOutcome::Dns,
        _ => ReachabilityOutcome::Network,
    }
}

pub fn parse_webauthn_request_frame(value: Value) -> Result<WebAuthnRequestFrame, String> {
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "WebAuthn request frame has no kind".to_string())?;
    match kind {
        "welcome" => Ok(WebAuthnRequestFrame::Welcome {
            provider_id: value
                .get("providerId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "WebAuthn welcome frame has no providerId".to_string())?
                .to_string(),
        }),
        "ceremony" => Ok(WebAuthnRequestFrame::Ceremony {
            request_id: value
                .get("requestId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "WebAuthn ceremony frame has no requestId".to_string())?
                .to_string(),
            ceremony: serde_json::from_value(
                value
                    .get("ceremony")
                    .cloned()
                    .ok_or_else(|| "WebAuthn ceremony frame has no ceremony".to_string())?,
            )
            .map_err(|error| format!("invalid WebAuthn ceremony: {error}"))?,
        }),
        "cancel" => Ok(WebAuthnRequestFrame::Cancel {
            request_id: value
                .get("requestId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "WebAuthn cancel frame has no requestId".to_string())?
                .to_string(),
        }),
        other => Err(format!("unknown WebAuthn request frame kind {other}")),
    }
}

pub fn webauthn_response_frame_value(frame: &WebAuthnResponseFrame) -> Value {
    match frame {
        WebAuthnResponseFrame::Hello {
            computer_id,
            label,
        } => {
            let mut value = json!({ "kind": "hello" });
            if let Some(computer_id) = computer_id {
                value["computerId"] = Value::String(computer_id.clone());
            }
            if let Some(label) = label {
                value["label"] = Value::String(label.clone());
            }
            value
        }
        WebAuthnResponseFrame::Stage {
            request_id,
            stage,
            outcome,
        } => json!({
            "kind": "stage",
            "requestId": request_id,
            "stage": stage,
            "outcome": outcome,
        }),
        WebAuthnResponseFrame::Result {
            request_id,
            credential_json,
        } => json!({
            "kind": "result",
            "requestId": request_id,
            "credentialJson": credential_json,
        }),
        WebAuthnResponseFrame::Error { request_id, error } => {
            let mut value = json!({
                "kind": "error",
                "requestId": request_id,
                "name": error.name,
                "message": error.message,
            });
            if let Some(code) = &error.code {
                value["code"] = Value::String(code.clone());
            }
            value
        }
        WebAuthnResponseFrame::Ping => json!({ "kind": "ping" }),
    }
}

pub fn post_webauthn_frames(
    connection: &GatewayConnection,
    provider_id: Option<&str>,
    frames: &[WebAuthnResponseFrame],
) -> Result<(), GatewayDispatchError> {
    let endpoint = parse_gateway_http_base(&connection.base_url)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let addresses = endpoint.socket_addrs().map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: format!("WebAuthn response endpoint unreachable (dns): {error}"),
        }
    })?;
    if addresses.is_empty() {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: "WebAuthn response endpoint unreachable (dns)".into(),
        });
    }

    let timeout = Duration::from_millis(SSE_CONNECT_TIMEOUT_MS);
    let mut stream = endpoint.connect_any(&addresses, timeout).map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: webauthn_io_outcome(&error),
            message: format!("WebAuthn response endpoint connect failed: {error}"),
        }
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;

    let mut batch = json!({
        "frames": frames
            .iter()
            .map(webauthn_response_frame_value)
            .collect::<Vec<_>>(),
    });
    if let Some(provider_id) = provider_id.filter(|provider_id| !provider_id.is_empty()) {
        batch["providerId"] = Value::String(provider_id.to_string());
    }
    let body = serde_json::to_vec(&batch)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let path = format!("{}{}", endpoint.base_path, GATEWAY_WEBAUTHN_RESPONSES_PATH);
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        endpoint.host,
        body.len()
    )
    .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    for (name, value) in &connection.headers {
        write!(stream, "{name}: {value}\r\n")
            .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    }
    write!(stream, "\r\n")
        .and_then(|_| stream.write_all(&body))
        .and_then(|_| stream.flush())
        .map_err(|error| GatewayDispatchError::Unreachable {
            outcome: webauthn_io_outcome(&error),
            message: format!("WebAuthn response delivery failed: {error}"),
        })?;

    let response = read_http_response(&mut stream, 64 * 1024, 1024 * 1024).map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: webauthn_io_outcome(&error),
            message: format!("WebAuthn response acknowledgement failed: {error}"),
        }
    })?;
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    if matches!(response.status, 401 | 403) {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::AccessDenied,
            message: format!(
                "WebAuthn response endpoint rejected authorization with HTTP {}",
                response.status
            ),
        });
    }
    if response.status >= 500 {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Http(response.status),
            message: format!(
                "WebAuthn response endpoint failed with HTTP {}",
                response.status
            ),
        });
    }
    Err(GatewayDispatchError::Transport(format!(
        "WebAuthn response endpoint rejected the batch with HTTP {}",
        response.status
    )))
}

fn webauthn_sse_data(block: &str) -> Option<String> {
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>();
    (!data.is_empty()).then(|| data.join("\n"))
}

pub fn stream_webauthn_requests<Connected, Frame, Continue>(
    connection: &GatewayConnection,
    on_connected: Connected,
    mut on_frame: Frame,
    should_continue: Continue,
) -> Result<(), GatewayDispatchError>
where
    Connected: FnOnce(),
    Frame: FnMut(WebAuthnRequestFrame),
    Continue: Fn() -> bool,
{
    let endpoint = parse_gateway_http_base(&connection.base_url)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let addresses = endpoint.socket_addrs().map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: format!("WebAuthn request stream unreachable (dns): {error}"),
        }
    })?;
    if addresses.is_empty() {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: "WebAuthn request stream unreachable (dns)".into(),
        });
    }

    let connect_timeout = Duration::from_millis(SSE_CONNECT_TIMEOUT_MS);
    let mut stream = endpoint.connect_any(&addresses, connect_timeout).map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: webauthn_io_outcome(&error),
            message: format!("WebAuthn request stream connect failed: {error}"),
        }
    })?;
    stream
        .set_write_timeout(Some(connect_timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let cancellation_poll = Duration::from_millis(250);
    stream
        .set_read_timeout(Some(cancellation_poll))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;

    let path = format!("{}{}", endpoint.base_path, GATEWAY_WEBAUTHN_REQUESTS_PATH);
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n",
        endpoint.host
    )
    .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    for (name, value) in &connection.headers {
        write!(stream, "{name}: {value}\r\n")
            .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    }
    write!(stream, "\r\n")
        .and_then(|_| stream.flush())
        .map_err(|error| GatewayDispatchError::Unreachable {
            outcome: webauthn_io_outcome(&error),
            message: format!("WebAuthn request stream handshake failed: {error}"),
        })?;

    let mut buffered = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let handshake_started = Instant::now();
    let header_end = loop {
        if !should_continue() {
            return Ok(());
        }
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                if !should_continue() {
                    return Ok(());
                }
                if handshake_started.elapsed() >= connect_timeout {
                    return Err(GatewayDispatchError::Unreachable {
                        outcome: ReachabilityOutcome::Timeout,
                        message: "WebAuthn request stream handshake timed out".into(),
                    });
                }
                continue;
            }
            Err(error) => {
                return Err(GatewayDispatchError::Unreachable {
                    outcome: webauthn_io_outcome(&error),
                    message: format!("WebAuthn request stream handshake failed: {error}"),
                });
            }
        };
        if count == 0 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "WebAuthn request stream ended during handshake".into(),
            });
        }
        buffered.extend_from_slice(&chunk[..count]);
        if let Some(index) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if buffered.len() > 64 * 1024 {
            return Err(GatewayDispatchError::Transport(
                "WebAuthn request stream response headers are too large".into(),
            ));
        }
    };

    let headers = std::str::from_utf8(&buffered[..header_end]).map_err(|_| {
        GatewayDispatchError::Transport(
            "WebAuthn request stream response headers are not UTF-8".into(),
        )
    })?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| {
            GatewayDispatchError::Transport(
                "WebAuthn request stream response has no HTTP status".into(),
            )
        })?;
    if !(200..300).contains(&status) {
        if matches!(status, 401 | 403) {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::AccessDenied,
                message: format!("WebAuthn request stream failed with HTTP {status}"),
            });
        }
        if status >= 500 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Http(status),
                message: format!("WebAuthn request stream failed with HTTP {status}"),
            });
        }
        return Err(GatewayDispatchError::Transport(format!(
            "WebAuthn request stream failed with HTTP {status}"
        )));
    }

    on_connected();
    let mut decoder = SseBlockDecoder::default();
    let deliver = |decoder: &mut SseBlockDecoder,
                   bytes: &[u8],
                   on_frame: &mut Frame| {
        for block in decoder.push_bytes(bytes) {
            let Some(data) = webauthn_sse_data(&block) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            if let Ok(frame) = parse_webauthn_request_frame(value) {
                on_frame(frame);
            }
        }
    };
    deliver(&mut decoder, &buffered[header_end..], &mut on_frame);
    if !should_continue() {
        return Ok(());
    }

    let mut last_data = Instant::now();
    loop {
        if !should_continue() {
            return Ok(());
        }
        let count = match stream.read(&mut chunk) {
            Ok(count) => count,
            Err(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                if !should_continue() {
                    return Ok(());
                }
                if last_data.elapsed() >= Duration::from_millis(SSE_STALL_TIMEOUT_MS) {
                    return Err(GatewayDispatchError::Unreachable {
                        outcome: ReachabilityOutcome::Timeout,
                        message: "WebAuthn request stream stalled".into(),
                    });
                }
                continue;
            }
            Err(error) => {
                return Err(GatewayDispatchError::Unreachable {
                    outcome: webauthn_io_outcome(&error),
                    message: format!("WebAuthn request stream failed: {error}"),
                });
            }
        };
        if count == 0 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "WebAuthn request stream closed".into(),
            });
        }
        last_data = Instant::now();
        deliver(&mut decoder, &chunk[..count], &mut on_frame);
        if !should_continue() {
            return Ok(());
        }
    }
}


pub type WebAuthnResolveConnection = Arc<
    dyn Fn() -> Result<GatewayConnection, GatewayDispatchError> + Send + Sync,
>;
pub type WebAuthnConsentCallback = Arc<
    dyn Fn(&WebAuthnCeremony) -> Result<ApprovedWebAuthnConsent, String> + Send + Sync,
>;
pub type WebAuthnStatusCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type WebAuthnPinCallback = Arc<
    dyn Fn(WebAuthnPinRequest, &str) -> Option<String> + Send + Sync,
>;
pub type WebAuthnFinishCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct ProductionWebAuthnRuntimeOptions {
    pub signer: SpawnedWebAuthnSigner,
    pub resolve_connection: WebAuthnResolveConnection,
    pub request_consent: WebAuthnConsentCallback,
    pub update_status: WebAuthnStatusCallback,
    pub request_pin: WebAuthnPinCallback,
    pub finish_consent: WebAuthnFinishCallback,
    pub computer_id: Option<String>,
    pub label: Option<String>,
}

pub struct ProductionWebAuthnRuntime {
    options: ProductionWebAuthnRuntimeOptions,
    closed: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    cancellations: Arc<Mutex<HashMap<String, WebAuthnSignCancellation>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProductionWebAuthnRuntime {
    pub fn new(options: ProductionWebAuthnRuntimeOptions) -> Self {
        Self {
            options,
            closed: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(true)),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            worker: None,
        }
    }

    pub fn start(&mut self) {
        self.paused.store(false, Ordering::Release);
        if self.worker.is_some() {
            return;
        }
        let options = self.options.clone();
        let closed = Arc::clone(&self.closed);
        let paused = Arc::clone(&self.paused);
        let cancellations = Arc::clone(&self.cancellations);
        self.worker = Some(thread::spawn(move || {
            run_production_webauthn_runtime(options, closed, paused, cancellations);
        }));
    }

    pub fn stop(&mut self) {
        self.paused.store(true, Ordering::Release);
        cancel_all_webauthn(&self.cancellations);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn in_flight_count(&self) -> usize {
        self.cancellations.lock().map(|value| value.len()).unwrap_or_default()
    }

    pub fn dispose(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ProductionWebAuthnRuntime {
    fn drop(&mut self) {
        self.dispose();
    }
}

fn cancel_all_webauthn(
    cancellations: &Arc<Mutex<HashMap<String, WebAuthnSignCancellation>>>,
) {
    let active = cancellations
        .lock()
        .map(|mut entries| entries.drain().map(|(_, value)| value).collect::<Vec<_>>())
        .unwrap_or_default();
    for cancellation in active {
        cancellation.cancel();
    }
}

fn sleep_webauthn_backoff(
    closed: &AtomicBool,
    paused: &AtomicBool,
    duration: Duration,
) {
    let started = Instant::now();
    while started.elapsed() < duration {
        if closed.load(Ordering::Acquire) || paused.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn webauthn_reconnect_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(5);
    Duration::from_millis((1_000_u64.saturating_mul(1_u64 << exponent)).min(30_000))
}

fn deliver_webauthn_frames_with_retry(
    connection: &GatewayConnection,
    provider_id: Option<String>,
    frames: &[WebAuthnResponseFrame],
    cancellation: Option<&WebAuthnSignCancellation>,
) {
    for attempt in 0..5_u32 {
        if cancellation.is_some_and(WebAuthnSignCancellation::is_cancelled) {
            return;
        }
        if post_webauthn_frames(connection, provider_id.as_deref(), frames).is_ok() {
            return;
        }
        if attempt < 4 {
            thread::sleep(Duration::from_millis(
                (250_u64.saturating_mul(1_u64 << attempt)).min(4_000),
            ));
        }
    }
}

fn run_production_webauthn_runtime(
    options: ProductionWebAuthnRuntimeOptions,
    closed: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    cancellations: Arc<Mutex<HashMap<String, WebAuthnSignCancellation>>>,
) {
    let provider_id = Arc::new(Mutex::new(None::<String>));
    let mut reconnect_attempt = 0_u32;

    while !closed.load(Ordering::Acquire) {
        if paused.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(50));
            reconnect_attempt = 0;
            continue;
        }

        let connection = match (options.resolve_connection)() {
            Ok(connection) => connection,
            Err(_) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                sleep_webauthn_backoff(
                    &closed,
                    &paused,
                    webauthn_reconnect_delay(reconnect_attempt),
                );
                continue;
            }
        };

        let heartbeat_stop = Arc::new(AtomicBool::new(false));
        let heartbeat_flag = Arc::clone(&heartbeat_stop);
        let heartbeat_closed = Arc::clone(&closed);
        let heartbeat_paused = Arc::clone(&paused);
        let heartbeat_connection = connection.clone();
        let heartbeat_provider_id = Arc::clone(&provider_id);
        let heartbeat = thread::spawn(move || {
            while !heartbeat_flag.load(Ordering::Acquire)
                && !heartbeat_closed.load(Ordering::Acquire)
                && !heartbeat_paused.load(Ordering::Acquire)
            {
                let started = Instant::now();
                while started.elapsed() < Duration::from_millis(WEBAUTHN_HEARTBEAT_INTERVAL_MS) {
                    if heartbeat_flag.load(Ordering::Acquire)
                        || heartbeat_closed.load(Ordering::Acquire)
                        || heartbeat_paused.load(Ordering::Acquire)
                    {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let current_provider_id = heartbeat_provider_id
                    .lock()
                    .ok()
                    .and_then(|value| value.clone());
                let _ = post_webauthn_frames(
                    &heartbeat_connection,
                    current_provider_id.as_deref(),
                    &[WebAuthnResponseFrame::Ping],
                );
            }
        });

        let stream_connection = connection.clone();
        let frame_connection = connection.clone();
        let stream_provider_id = Arc::clone(&provider_id);
        let stream_cancellations = Arc::clone(&cancellations);
        let stream_options = options.clone();
        let stream_closed = Arc::clone(&closed);
        let stream_paused = Arc::clone(&paused);
        let stream_result = stream_webauthn_requests(
            &stream_connection,
            || {},
            move |frame| match frame {
                WebAuthnRequestFrame::Welcome { provider_id: next_provider_id } => {
                    if let Ok(mut current) = stream_provider_id.lock() {
                        *current = Some(next_provider_id.clone());
                    }
                    let _ = post_webauthn_frames(
                        &frame_connection,
                        Some(&next_provider_id),
                        &[WebAuthnResponseFrame::Hello {
                            computer_id: stream_options.computer_id.clone(),
                            label: stream_options.label.clone(),
                        }],
                    );
                }
                WebAuthnRequestFrame::Cancel { request_id } => {
                    let cancellation = stream_cancellations
                        .lock()
                        .ok()
                        .and_then(|mut values| values.remove(&request_id));
                    if let Some(cancellation) = cancellation {
                        cancellation.cancel();
                    }
                }
                WebAuthnRequestFrame::Ceremony {
                    request_id,
                    ceremony,
                } => {
                    let cancellation = WebAuthnSignCancellation::default();
                    let duplicate = stream_cancellations
                        .lock()
                        .map(|mut values| {
                            if values.contains_key(&request_id) {
                                true
                            } else {
                                values.insert(request_id.clone(), cancellation.clone());
                                false
                            }
                        })
                        .unwrap_or(true);
                    if duplicate {
                        let current_provider_id = stream_provider_id
                            .lock()
                            .ok()
                            .and_then(|value| value.clone());
                        deliver_webauthn_frames_with_retry(
                            &frame_connection,
                            current_provider_id,
                            &[WebAuthnResponseFrame::Error {
                                request_id,
                                error: WebAuthnSignerError {
                                    name: "InvalidStateError".into(),
                                    code: Some("duplicate_request".into()),
                                    message: "WebAuthn request is already active".into(),
                                },
                            }],
                            None,
                        );
                        return;
                    }

                    let ceremony_connection = frame_connection.clone();
                    let ceremony_provider_id = Arc::clone(&stream_provider_id);
                    let ceremony_cancellations = Arc::clone(&stream_cancellations);
                    let ceremony_options = stream_options.clone();
                    thread::spawn(move || {
                        let consent = match (ceremony_options.request_consent)(&ceremony) {
                            Ok(consent) => consent,
                            Err(message) => {
                                let current_provider_id = ceremony_provider_id
                                    .lock()
                                    .ok()
                                    .and_then(|value| value.clone());
                                deliver_webauthn_frames_with_retry(
                                    &ceremony_connection,
                                    current_provider_id,
                                    &[
                                        WebAuthnResponseFrame::Stage {
                                            request_id: request_id.clone(),
                                            stage: "grant",
                                            outcome: "failed",
                                        },
                                        WebAuthnResponseFrame::Error {
                                            request_id: request_id.clone(),
                                            error: WebAuthnSignerError {
                                                name: "NotAllowedError".into(),
                                                code: Some("consent_failed".into()),
                                                message,
                                            },
                                        },
                                    ],
                                    Some(&cancellation),
                                );
                                ceremony_cancellations
                                    .lock()
                                    .ok()
                                    .map(|mut values| values.remove(&request_id));
                                (ceremony_options.finish_consent)();
                                return;
                            }
                        };

                        if !consent.approved {
                            let current_provider_id = ceremony_provider_id
                                .lock()
                                .ok()
                                .and_then(|value| value.clone());
                            deliver_webauthn_frames_with_retry(
                                &ceremony_connection,
                                current_provider_id,
                                &[
                                    WebAuthnResponseFrame::Stage {
                                        request_id: request_id.clone(),
                                        stage: "grant",
                                        outcome: "declined",
                                    },
                                    WebAuthnResponseFrame::Error {
                                        request_id: request_id.clone(),
                                        error: WebAuthnSignerError {
                                            name: "NotAllowedError".into(),
                                            code: Some("consent_declined".into()),
                                            message: "The security key request was declined on this computer".into(),
                                        },
                                    },
                                ],
                                Some(&cancellation),
                            );
                            ceremony_cancellations
                                .lock()
                                .ok()
                                .map(|mut values| values.remove(&request_id));
                            (ceremony_options.finish_consent)();
                            return;
                        }

                        let current_provider_id = ceremony_provider_id
                            .lock()
                            .ok()
                            .and_then(|value| value.clone());
                        deliver_webauthn_frames_with_retry(
                            &ceremony_connection,
                            current_provider_id.clone(),
                            &[WebAuthnResponseFrame::Stage {
                                request_id: request_id.clone(),
                                stage: "grant",
                                outcome: "ok",
                            }],
                            Some(&cancellation),
                        );

                        if cancellation.is_cancelled() {
                            ceremony_cancellations
                                .lock()
                                .ok()
                                .map(|mut values| values.remove(&request_id));
                            (ceremony_options.finish_consent)();
                            return;
                        }

                        let status = Arc::clone(&ceremony_options.update_status);
                        let pin = Arc::clone(&ceremony_options.request_pin);
                        let result = ceremony_options.signer.sign_interactive(
                            &ceremony,
                            Some(&consent),
                            &cancellation,
                            move |message| status(message),
                            move |request, prompt_id| pin(request, prompt_id),
                        );
                        if !cancellation.is_cancelled() {
                            let frames = match result {
                                WebAuthnSignerResult::Success { credential_json } => vec![
                                    WebAuthnResponseFrame::Stage {
                                        request_id: request_id.clone(),
                                        stage: "sign",
                                        outcome: "ok",
                                    },
                                    WebAuthnResponseFrame::Result {
                                        request_id: request_id.clone(),
                                        credential_json,
                                    },
                                ],
                                WebAuthnSignerResult::Failed { error } => vec![
                                    WebAuthnResponseFrame::Stage {
                                        request_id: request_id.clone(),
                                        stage: "sign",
                                        outcome: "failed",
                                    },
                                    WebAuthnResponseFrame::Error {
                                        request_id: request_id.clone(),
                                        error,
                                    },
                                ],
                            };
                            deliver_webauthn_frames_with_retry(
                                &ceremony_connection,
                                current_provider_id,
                                &frames,
                                Some(&cancellation),
                            );
                        }
                        ceremony_cancellations
                            .lock()
                            .ok()
                            .map(|mut values| values.remove(&request_id));
                        (ceremony_options.finish_consent)();
                    });
                }
            },
            move || {
                !stream_closed.load(Ordering::Acquire)
                    && !stream_paused.load(Ordering::Acquire)
            },
        );

        heartbeat_stop.store(true, Ordering::Release);
        let _ = heartbeat.join();
        if let Ok(mut current) = provider_id.lock() {
            *current = None;
        }
        cancel_all_webauthn(&cancellations);

        if closed.load(Ordering::Acquire) || paused.load(Ordering::Acquire) {
            reconnect_attempt = 0;
            continue;
        }
        if stream_result.is_ok() {
            reconnect_attempt = 0;
        } else {
            reconnect_attempt = reconnect_attempt.saturating_add(1);
        }
        sleep_webauthn_backoff(
            &closed,
            &paused,
            webauthn_reconnect_delay(reconnect_attempt.max(1)),
        );
    }
}
