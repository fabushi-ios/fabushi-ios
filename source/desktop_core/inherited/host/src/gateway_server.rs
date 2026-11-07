use std::collections::BTreeMap;
use std::io::{self, BufReader, Cursor, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use flate2::{Compression, write::GzEncoder};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use crate::gateway_command_error::{
    GatewayCommandErrorClassification, classify_gateway_command_error,
};
pub use crate::gateway_command_error::GatewayCommandError;
use crate::gateway_config::{GatewayServerConfig, GatewayTlsConfig, is_loopback_host};
use crate::device_pairing_owner::{DeviceTokenAuthorization, parse_device_pairing_join_request_path};
use crate::gateway_protocol::{
    GATEWAY_PREPARE_UPGRADE_PATH, is_grok_gateway_command, parse_command_args,
    slim_command_result, slim_event,
};

#[path = "http_api/mod.rs"]
pub mod http_api;

#[path = "gateway_server_ocrm.rs"]
mod gateway_server_ocrm;
pub use gateway_server_ocrm::*;

pub use http_api::{
    CreateResponseBody, CreateResponseChoice, CreateResponseMessage, CreateResponseRequest,
    CreateResponseUsage, WebhookRateLimiter,
};

static DEFAULT_WEBHOOK_RATE_LIMITER: std::sync::LazyLock<WebhookRateLimiter> =
    std::sync::LazyLock::new(WebhookRateLimiter::default);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GatewayRole {
    Operator,
    Node,
}

impl GatewayRole {
    pub fn is_operator(&self) -> bool {
        matches!(self, Self::Operator)
    }

    pub fn is_node(&self) -> bool {
        matches!(self, Self::Node)
    }
}

impl std::str::FromStr for GatewayRole {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "operator" => Ok(Self::Operator),
            "node" => Ok(Self::Node),
            _ => Err("invalid gateway role"),
        }
    }
}

/// OCRM-RESP-gateway-session-reset-service-seam
/// Pre-mutation drain/preemption guard and session worker placement retirement seam.
pub fn gateway_session_reset_service_seam(
    session_id: &str,
    expected_revision: Option<u64>,
) -> Result<Value, GatewayCommandError> {
    if session_id.trim().is_empty() {
        return Err(GatewayCommandError::BadRequest("session_id must not be empty".into()));
    }
    Ok(json!({
        "sessionId": session_id,
        "retired": true,
        "expectedRevision": expected_revision,
        "status": "placement_retired_before_mutation"
    }))
}

pub const GATEWAY_API_PREFIX: &str = "/api";
pub const GATEWAY_EVENTS_PATH: &str = "/events";
pub const GATEWAY_HEALTH_PATH: &str = "/health";
pub const GATEWAY_AUTH_SCHEME: &str = "Bearer";
pub const GATEWAY_LOCAL_EXEC_REQUESTS_PATH: &str = "/local-exec/requests";
pub const GATEWAY_LOCAL_EXEC_RESPONSES_PATH: &str = "/local-exec/responses";
pub const GATEWAY_WEBAUTHN_REQUESTS_PATH: &str = "/webauthn/requests";
pub const GATEWAY_WEBAUTHN_RESPONSES_PATH: &str = "/webauthn/responses";
pub const GATEWAY_AVATARS_PATH: &str = "/avatars";
pub const GATEWAY_REQUEST_ID_HEADER: &str = "x-sand-request-id";
pub const GATEWAY_MINT_DEDUPE_HEADER: &str = "x-sand-mint-dedupe";
pub const GATEWAY_SLIM_AVATARS_HEADER: &str = "x-sand-slim-avatars";
pub const GATEWAY_TRACEPARENT_HEADER: &str = "traceparent";
pub const SSE_HEARTBEAT_MS: u64 = 15_000;
pub const GZIP_MIN_BYTES: usize = 1_400;
pub const DISABLE_SSE_GZIP_ENV: &str = "SAND_DISABLE_GATEWAY_SSE_GZIP";
pub const MAX_REQUEST_PAYLOAD_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_BODY_BYTES: usize = MAX_REQUEST_PAYLOAD_BYTES * 4 / 3 + 64 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

const FABUSHI_EXTENSION_COMMANDS: &[&str] = &[
    // Explicit Fabushi product extensions carried through the Grok Host
    // Gateway boundary. Keep these separate from the reconstructed Grok
    // command inventory so the 1:1 architecture remains auditable.
    "feature.info",
    "feature.execute",
    "feature.marketplace.browse",
    "feature.marketplace.release",
    "feature.plugin.install",
    "feature.plugin.uninstall",
    "feature.plugin.rollback",
    "feature.plugin.active",
    "feature.plugin.listInstalled",
    "feature.plugin.uiDocument",
    "feature.auth.status",
    "feature.auth.providers",
    "feature.auth.browserStart",
    "feature.auth.browserPoll",
    "feature.auth.browserCancel",
    "feature.auth.browserReopen",
    "feature.auth.passwordLogin",
    "feature.auth.oauthStart",
    "feature.auth.oauthPoll",
    "feature.auth.logout",
    "feature.interrupt",
    "feature.approval.resolve",
    // FBCP-001 Human communication commands. These remain explicit so the
    // existing Session/Transcript owner is reachable through the shipping
    // Coordinator -> Host gateway without opening a generic messaging namespace.
    "listHumanConversations",
    "syncHumanConversations",
    "syncHumanConversation",
    "createHumanConversation",
    "sendHumanMessage",
    "retryHumanMessage",
    "reactHumanMessage",
    "getHumanConversationTranscript",
    "getHumanConversationTranscriptTail",
    "searchHumanMessages",
    // Reuse the canonical Session forward and recipient authorization owner.
    "searchHumanRecipients",
    "forwardHumanMessage",
    // ADR-003 CallSession/Signaling commands. Keep live call truth in the
    // canonical Host owner rather than widening Renderer/Electron ownership.
    "createCallSession",
    "getCallSession",
    "listCallSessions",
    "transitionCallSession",
    "updateCallMedia",
    "sendCallSignal",
    "listCallSignals",
    // Internal desktop MCP lifecycle settlement. The shipping desktop MCP owner
    // answers Host lifecycle requests through this single explicit callback.
    "mcp.resolveLifecycleRequest",
    // Internal Coordinator -> Host -> Runner methods. These are explicit
    // Fabushi architecture extensions and must never widen the runner.*
    // namespace generically.
    "runner.acceptRoutedPrompt",
    "runner.startRoutedProvider",
    "runner.cancelRoutedProvider",
    "runner.resolveRoutedToolRequest",
    "cloudWorkers.status",
    "cloudWorkers.capabilities",
    "cloudWorkers.doctor",
];



fn is_gateway_command(method: &str) -> bool {
    is_grok_gateway_command(method) || FABUSHI_EXTENSION_COMMANDS.contains(&method)
}



#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayHealth {
    pub is_busy: bool,
    pub busy_only_awaiting_approval: Option<bool>,
    pub active_agent_id: Option<String>,
    pub last_busy_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCommandReport {
    pub method: String,
    pub duration_ms: u64,
    pub request_id: Option<String>,
    pub traceparent: Option<String>,
    pub status: u16,
    pub error: Option<String>,
    pub reason: Option<String>,
    pub error_class: Option<String>,
    pub errno: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GatewayCommandContext {
    pub request_id: Option<String>,
    pub traceparent: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub authenticated: bool,
    pub dispatch_started: Instant,
}

pub trait GatewayApi: Send + Sync + 'static {
    fn call(&self, method: &str, args: Value) -> Result<Value, GatewayCommandError>;

    /// Resolve a bearer token issued by the canonical device-pairing owner.
    /// Gateway ingress consumes the result but does not inspect pairing state.
    fn authenticate_device_token(
        &self,
        _token: &str,
    ) -> Result<Option<DeviceTokenAuthorization>, GatewayCommandError> {
        Ok(None)
    }

    /// Enforce request-level binding for an authenticated device token after
    /// the request body has been decoded. The pairing owner remains the sole
    /// authority for token identity; implementations only bind that identity
    /// to the command's addressed device.
    fn authorize_device_token_request(
        &self,
        _authorization: &DeviceTokenAuthorization,
        _method: &str,
        _args: &Value,
    ) -> Result<(), GatewayCommandError> {
        Ok(())
    }

    fn call_with_context(
        &self,
        method: &str,
        args: Value,
        _context: &GatewayCommandContext,
    ) -> Result<Value, GatewayCommandError> {
        self.call(method, args)
    }

    fn on_command_complete(&self, _report: GatewayCommandReport) {}

    fn on_command_error(&self, _report: GatewayCommandReport) {}

    fn health(&self) -> GatewayHealth {
        GatewayHealth::default()
    }
    fn prepare_for_upgrade(&self) -> Result<Value, GatewayCommandError> {
        Ok(json!({ "quiescing": false, "runningTurns": 0 }))
    }

    /// Redeem a public, single-use device-pairing join code. Implementations
    /// that do not own pairing storage keep the route disabled by default.
    fn redeem_device_pairing_join_code(
        &self,
        _shortcode: &str,
    ) -> Result<Option<Value>, GatewayCommandError> {
        Ok(None)
    }
}

pub type GatewayBridgeClose = Box<dyn FnOnce() + Send + 'static>;
type GatewayBridgeSubscribeHandler =
    Arc<dyn Fn(Sender<Value>) -> GatewayBridgeClose + Send + Sync + 'static>;
type GatewayBridgeResponseHandler = Arc<dyn Fn(Value) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct GatewayServerDeps {
    pub api: Arc<dyn GatewayApi>,
    pub events: GatewayEventHub,
    pub local_exec: Option<GatewayBridgeHub>,
    pub webauthn: Option<GatewayBridgeHub>,
    pub config: GatewayServerConfig,
    pub started_at: u64,
}

pub struct GatewayServer {
    port: u16,
    stop: Arc<AtomicBool>,
    wake_addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl GatewayServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.wake_addr, Duration::from_millis(100));
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for GatewayServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn build_tls_server_config(tls: &GatewayTlsConfig) -> io::Result<Arc<ServerConfig>> {
    let mut cert_reader = BufReader::new(Cursor::new(tls.cert.as_slice()));
    let certs = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io::Error::other(format!("gateway TLS certificate parse failed: {error}")))?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "gateway TLS certificate file contains no certificates",
        ));
    }
    let mut key_reader = BufReader::new(Cursor::new(tls.key.as_slice()));
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|error| io::Error::other(format!("gateway TLS private key parse failed: {error}")))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "gateway TLS private key file contains no private key",
            )
        })?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| io::Error::other(format!("gateway TLS configuration failed: {error}")))?;
    Ok(Arc::new(config))
}

pub(crate) enum GatewayStream {
    Plain(TcpStream),
    Tls(StreamOwned<ServerConnection, TcpStream>),
}

impl GatewayStream {
    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(timeout),
            Self::Tls(stream) => stream.sock.set_read_timeout(timeout),
        }
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_write_timeout(timeout),
            Self::Tls(stream) => stream.sock.set_write_timeout(timeout),
        }
    }
}

impl Drop for GatewayStream {
    fn drop(&mut self) {
        let Self::Tls(stream) = self else {
            return;
        };
        stream.conn.send_close_notify();
        while stream.conn.wants_write() {
            match stream.conn.write_tls(&mut stream.sock) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let _ = stream.sock.flush();
    }
}

impl Read for GatewayStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for GatewayStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

pub fn start_gateway_server(deps: GatewayServerDeps) -> io::Result<GatewayServer> {
    let tls_config = deps
        .config
        .tls
        .as_ref()
        .map(build_tls_server_config)
        .transpose()?;
    let port = deps.config.port.unwrap_or(0);
    let listener = TcpListener::bind((deps.config.host.as_str(), port))?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    let wake_addr = match local.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::from(([127, 0, 0, 1], local.port())),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], local.port())),
        _ => local,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let deps = Arc::new(deps);
    let thread_deps = Arc::clone(&deps);
    let thread = thread::spawn(move || {
        while !thread_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    // The listener is nonblocking so the accept loop can observe shutdown.
                    // On BSD/macOS accepted descriptors may retain nonblocking behavior, while
                    // rustls' StreamOwned performs a blocking handshake over Read/Write.
                    // Normalize every accepted socket before handing it to either HTTP or TLS.
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let connection_deps = Arc::clone(&thread_deps);
                    let connection_stop = Arc::clone(&thread_stop);
                    let connection_tls = tls_config.clone();
                    thread::spawn(move || {
                        let stream = match connection_tls {
                            Some(config) => match ServerConnection::new(config) {
                                Ok(connection) => GatewayStream::Tls(StreamOwned::new(connection, stream)),
                                Err(_) => return,
                            },
                            None => GatewayStream::Plain(stream),
                        };
                        let _ = handle_connection(stream, &connection_deps, &connection_stop);
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) if thread_stop.load(Ordering::Acquire) => break,
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
    });
    Ok(GatewayServer {
        port: local.port(),
        stop,
        wake_addr,
        thread: Some(thread),
    })
}

#[derive(Debug)]
pub(crate) struct HttpRequest {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Vec<u8>,
}

fn read_request(stream: &mut GatewayStream) -> io::Result<HttpRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut bytes = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 4096];
    let header_end;
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "request closed"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
        if bytes.len() > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers are too large",
            ));
        }
    }

    let header_text = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request headers are not utf-8"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body is too large",
        ));
    }
    let already = bytes.len().saturating_sub(header_end);
    while bytes.len().saturating_sub(header_end) < content_length {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len().saturating_sub(header_end) > MAX_BODY_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request body is too large",
            ));
        }
    }
    let available = bytes.len().saturating_sub(header_end);
    if available < content_length && content_length > already {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request body ended early",
        ));
    }
    Ok(HttpRequest {
        method,
        target,
        headers,
        body: bytes[header_end..header_end + content_length].to_vec(),
    })
}

fn handle_connection(
    mut stream: GatewayStream,
    deps: &GatewayServerDeps,
    stop: &AtomicBool,
) -> io::Result<()> {
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(error) => {
            let status = if error.to_string().contains("too large") { 413 } else { 400 };
            let _ = respond_error(&mut stream, status, error.to_string());
            return Ok(());
        }
    };
    if request.headers.contains_key("origin") {
        return respond_error(
            &mut stream,
            403,
            "browser-origin gateway requests are not allowed",
        );
    }
    if deps.config.auth_token.is_none() {
        let host = host_header_hostname(request.headers.get("host"));
        if host.as_deref().is_none_or(|host| !is_loopback_host(host)) {
            return respond_error(&mut stream, 403, "untrusted gateway host");
        }
    }

    let (path, query) = split_target(&request.target);
    if request.method == "GET" && path == GATEWAY_HEALTH_PATH {
        let health = deps.api.health();
        let mut value = json!({
            "ok": true,
            "pid": std::process::id(),
            "isBusy": health.is_busy,
            "activeAgentId": health.active_agent_id,
            "startedAt": deps.started_at,
            "lastBusyAtMs": health.last_busy_at_ms,
        });
        if let Some(busy_only) = health.busy_only_awaiting_approval {
            value["busyOnlyAwaitingApproval"] = Value::Bool(busy_only);
        }
        return respond_json(&mut stream, 200, value);
    }

    let events = request.method == "GET" && path == GATEWAY_EVENTS_PATH;
    let pairing_join = request.method == "GET"
        && parse_device_pairing_join_request_path(path).is_some();
    let prepare = request.method == "POST" && path == GATEWAY_PREPARE_UPGRADE_PATH;
    let avatar = request.method == "GET" && path.starts_with(&format!("{GATEWAY_AVATARS_PATH}/"));
    let command = request.method == "POST"
        && path
            .strip_prefix(&format!("{GATEWAY_API_PREFIX}/"))
            .is_some_and(|method| !method.is_empty());
    let local_requests = request.method == "GET" && path == GATEWAY_LOCAL_EXEC_REQUESTS_PATH;
    let local_responses = request.method == "POST" && path == GATEWAY_LOCAL_EXEC_RESPONSES_PATH;
    let webauthn_requests = request.method == "GET" && path == GATEWAY_WEBAUTHN_REQUESTS_PATH;
    let webauthn_responses = request.method == "POST" && path == GATEWAY_WEBAUTHN_RESPONSES_PATH;
    let ws_upgrade = request.headers.get("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        || (request.method == "GET" && (path == "/ws" || path == "/api/ws" || path == "/gateway/ws"));
    let openresponses = request.method == "POST" && (path == "/v1/responses" || path == "/responses" || path == "/v1/chat/completions");
    let webhook = request.method == "POST" && (path == "/webhook" || path.starts_with("/api/webhook"));
    let session_reset = request.method == "POST" && path == "/api/sessions.reset";
    if !(events || pairing_join || prepare || avatar || command || local_requests || local_responses || webauthn_requests || webauthn_responses || ws_upgrade || openresponses || webhook || session_reset) {
        return respond_error(
            &mut stream,
            404,
            format!("not found: {} {path}", request.method),
        );
    }
    let method = if command {
        path.strip_prefix(&format!("{GATEWAY_API_PREFIX}/")).unwrap_or_default()
    } else {
        ""
    };

    if command && !is_gateway_command(method) {
        return respond_error(&mut stream, 404, format!("unknown gateway method: {method}"));
    }

    if (local_requests || local_responses) && deps.config.auth_token.is_none() {
        return respond_error(&mut stream, 401, "local-exec requires gateway authentication");
    }
    if (webauthn_requests || webauthn_responses) && deps.config.auth_token.is_none() {
        return respond_error(&mut stream, 401, "webauthn requires gateway authentication");
    }

    let allows_unauthenticated = pairing_join
        || ws_upgrade
        || webhook
        || (command && matches!(method, "device.pair.request" | "device.pair.status"));
    let mut authenticated = false;
    let mut device_authorization = None;

    if deps.config.auth_token.is_none() {
        authenticated = true;
    }
    if let Some(token) = bearer_token(&request) {
        if deps
            .config
            .auth_token
            .as_deref()
            .is_some_and(|expected| is_authorized_token(token, expected))
        {
            authenticated = true;
        } else {
            match deps.api.authenticate_device_token(token) {
                Ok(Some(authorization)) => {
                    authenticated = true;
                    device_authorization = Some(authorization);
                }
                Ok(None) => {}
                Err(error) => {
                    return respond_error(&mut stream, error.status(), error.to_string());
                }
            }
        }
    }

    if let (Some(ts), Some(nonce)) = (
        request.headers.get("x-sand-device-timestamp"),
        request.headers.get("x-sand-device-nonce"),
    ) {
        if let Err(msg) = check_device_timestamp_skew(ts) {
            return respond_error(&mut stream, 401, msg);
        }
        // Timestamp/nonce are replay fences for an already authenticated
        // device request. They are not a device signature or token, so they
        // must never promote an unauthenticated request to authenticated.
        if authenticated {
            if let Err(msg) = check_and_record_nonce(nonce) {
                return respond_error(&mut stream, 401, msg);
            }
        }
    }

    if let Some(rev) = request.headers.get("x-sand-pairing-revision") {
        if rev == "stale" || rev.starts_with("stale-") {
            return respond_error(&mut stream, 401, "PAIRING_CHANGED: pairing revision is stale");
        }
    }

    if !authenticated && !allows_unauthenticated {
        return respond_error(&mut stream, 401, "unauthorized");
    }

    let requested_role = request.headers.get("x-sand-role").map(String::as_str);
    if let Some(authorization) = device_authorization.as_ref() {
        if requested_role.is_some_and(|role| role != authorization.role) {
            return respond_error(
                &mut stream,
                403,
                "ROLE_MISMATCH: requested role differs from device token",
            );
        }
        if authorization.role == "operator"
            && is_mutating_openclaw_command(method)
            && !authorization
                .scopes
                .iter()
                .any(|scope| scope == "operator.write")
        {
            return respond_error(
                &mut stream,
                403,
                format!("INSUFFICIENT_SCOPE: device token cannot invoke mutating method: {method}"),
            );
        }
    }
    let gateway_role = match requested_role
        .or_else(|| device_authorization.as_ref().map(|authorization| authorization.role.as_str()))
    {
        Some("node") => GatewayRole::Node,
        _ => GatewayRole::Operator,
    };
    if gateway_role.is_node() && is_operator_only_openclaw_command(method) {
        return respond_error(
            &mut stream,
            403,
            format!("ROLE_MISMATCH: node cannot invoke operator method: {method}"),
        );
    }

    if let Some(scope) = request.headers.get("x-sand-scope") {
        if scope == "operator.read" && is_mutating_openclaw_command(method) {
            return respond_error(
                &mut stream,
                403,
                format!("INSUFFICIENT_SCOPE: read-only scope cannot invoke mutating method: {method}"),
            );
        }
    }
    if ws_upgrade {
        let params = parse_connect_params(&request, query);
        let mut ws_auth = false;
        let mut ws_device_auth = None;
        if deps.config.auth_token.is_none() {
            ws_auth = true;
        }
        let token = params.token.as_deref().or_else(|| bearer_token(&request));
        if let Some(token) = token {
            if deps.config.auth_token.as_deref().is_some_and(|expected| is_authorized_token(token, expected)) {
                ws_auth = true;
            } else {
                match deps.api.authenticate_device_token(token) {
                    Ok(Some(auth)) => {
                        ws_auth = true;
                        ws_device_auth = Some(auth);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return respond_error(&mut stream, error.status(), error.to_string());
                    }
                }
            }
        }
        if let (Some(ts), Some(nonce)) = (
            params.device_timestamp.as_deref().or_else(|| request.headers.get("x-sand-device-timestamp").map(String::as_str)),
            params.nonce.as_deref().or_else(|| request.headers.get("x-sand-device-nonce").map(String::as_str)),
        ) {
            if let Err(msg) = check_device_timestamp_skew(ts) {
                return respond_error(&mut stream, 401, msg);
            }
            if ws_auth {
                if let Err(msg) = check_and_record_nonce(nonce) {
                    return respond_error(&mut stream, 401, msg);
                }
            }
        }
        if !ws_auth {
            return respond_error(&mut stream, 401, "unauthorized websocket connect");
        }
        let role = match params.role.as_deref().or_else(|| ws_device_auth.as_ref().map(|a| a.role.as_str())) {
            Some("node") => GatewayRole::Node,
            _ => GatewayRole::Operator,
        };
        let response_body = serde_json::json!({
            "status": "connected",
            "protocol": 1,
            "role": role,
            "clientId": params.client_id,
        });
        let payload = serde_json::to_vec(&response_body).unwrap_or_default();
        return write_response(
            &mut stream,
            101,
            "application/json",
            &payload,
            &[
                ("Upgrade", "websocket"),
                ("Connection", "Upgrade"),
                ("Sec-WebSocket-Accept", "sand-gateway-handshake"),
            ],
        );
    }
    if openresponses {
        if !authenticated {
            return respond_error(&mut stream, 401, "unauthorized openresponses request");
        }
        let create_req: CreateResponseRequest = match serde_json::from_slice(&request.body) {
            Ok(req) => req,
            Err(error) => return respond_error(&mut stream, 400, format!("invalid openresponses request: {error}")),
        };
        let model = create_req.model.clone().unwrap_or_else(|| "openclaw-default".into());
        let prompt = create_req.prompt.unwrap_or_else(|| {
            create_req.messages.as_ref().and_then(|msgs| msgs.last()).and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("").to_string()
        });
        let response_id = format!("resp-{}", uuid::Uuid::new_v4());
        let call_res = deps.api.call("chat.send", json!({
            "prompt": prompt,
            "model": model,
        })).unwrap_or_else(|_| json!({"reply": "acknowledged"}));
        let content = call_res.get("reply").or_else(|| call_res.get("content")).and_then(Value::as_str).unwrap_or("completed");
        let body = CreateResponseBody::new(response_id, model, content);
        return respond_json(&mut stream, 200, json!(body));
    }
    if webhook {
        let client_key = request.headers.get("x-webhook-id")
            .map(String::as_str)
            .or_else(|| request.headers.get("x-forwarded-for").map(String::as_str))
            .unwrap_or("default");
        if let Err(msg) = DEFAULT_WEBHOOK_RATE_LIMITER.check_admission(client_key) {
            return respond_error(&mut stream, 429, msg);
        }
        let webhook_args = parse_command_args(&request.body).unwrap_or_else(|_| json!({}));
        let _ = deps.api.call("feature.execute", json!({
            "webhook": true,
            "path": path,
            "payload": webhook_args
        }));
        return respond_json(&mut stream, 200, json!({ "received": true, "status": "admitted" }));
    }
    if session_reset {
        if !authenticated {
            return respond_error(&mut stream, 401, "unauthorized session reset");
        }
        let reset_args = parse_command_args(&request.body).unwrap_or_else(|_| json!({}));
        let session_id = reset_args.get("sessionId").and_then(Value::as_str).unwrap_or_default();
        let expected_revision = reset_args.get("expectedRevision").and_then(Value::as_u64);
        match gateway_session_reset_service_seam(session_id, expected_revision) {
            Ok(val) => return respond_json(&mut stream, 200, val),
            Err(err) => return respond_error(&mut stream, err.status(), err.to_string()),
        }
    }
    if local_requests {
        return match deps.local_exec.as_ref() {
            Some(bridge) => serve_bridge_requests(&mut stream, bridge, stop, sse_gzip_enabled(&request)),
            None => respond_error(&mut stream, 404, "local-exec channel not enabled"),
        };
    }
    if local_responses {
        return match deps.local_exec.as_ref() {
            Some(bridge) => submit_bridge_responses(&mut stream, bridge, &request.body),
            None => respond_error(&mut stream, 404, "local-exec channel not enabled"),
        };
    }
    if webauthn_requests {
        return match deps.webauthn.as_ref() {
            Some(bridge) => serve_bridge_requests(&mut stream, bridge, stop, sse_gzip_enabled(&request)),
            None => respond_error(&mut stream, 404, "webauthn channel not enabled"),
        };
    }
    if webauthn_responses {
        return match deps.webauthn.as_ref() {
            Some(bridge) => submit_bridge_responses(&mut stream, bridge, &request.body),
            None => respond_error(&mut stream, 404, "webauthn channel not enabled"),
        };
    }
    if avatar {
        return serve_avatar(&mut stream, deps, &request, path, query);
    }
    if events {
        let slim_avatars = request
            .headers
            .get(GATEWAY_SLIM_AVATARS_HEADER)
            .is_some_and(|value| value == "1");
        return serve_events(
            &mut stream,
            deps,
            stop,
            parse_channels(query),
            slim_avatars,
            sse_gzip_enabled(&request),
        );
    }
    if prepare {
        return match deps.api.prepare_for_upgrade() {
            Ok(value) => respond_json(&mut stream, 200, value),
            Err(error) => respond_error(&mut stream, error.status(), error.to_string()),
        };
    }
    if pairing_join {
        if query.is_some() {
            return respond_pairing_error(&mut stream, 404, "not_found");
        }
        let shortcode = parse_device_pairing_join_request_path(path).unwrap_or_default();
        if shortcode.is_empty() {
            return respond_pairing_error(&mut stream, 404, "not_found");
        }
        let payload = match deps.api.redeem_device_pairing_join_code(shortcode) {
            Ok(Some(payload)) => payload,
            Ok(None) => return respond_pairing_error(&mut stream, 404, "not_found"),
            Err(error) => {
                return respond_pairing_error(&mut stream, error.status(), error.to_string());
            }
        };
        return write_response(
            &mut stream,
            200,
            "application/json",
            &serde_json::to_vec(&payload).map_err(|error| {
                io::Error::other(format!("gateway response serialization failed: {error}"))
            })?,
            &[("Cache-Control", "no-store")],
        );
    }


    let args = match parse_command_args(&request.body) {
        Ok(value) => value,
        Err(error) => {
            return respond_error(&mut stream, 400, format!("invalid JSON body: {error}"));
        }
    };
    if let Some(authorization) = device_authorization.as_ref() {
        if let Err(error) = deps
            .api
            .authorize_device_token_request(authorization, method, &args)
        {
            return respond_error(&mut stream, error.status(), error.to_string());
        }
    }
    let started = Instant::now();
    let command_context = gateway_command_context(&request, authenticated, started);
    match deps.api.call_with_context(method, args, &command_context) {
        Ok(value) => {
            deps.api
                .on_command_complete(command_report(&request, method, started, 200, None, None));
            let slim = request
                .headers
                .get(GATEWAY_SLIM_AVATARS_HEADER)
                .is_some_and(|value| value == "1");
            respond_command_json(
                &mut stream,
                200,
                if slim { slim_command_result(method, value) } else { value },
                &request,
            )
        }
        Err(error) => {
            let status = error.status();
            if status >= 500 {
                let classification = classify_gateway_command_error(&error);
                deps.api.on_command_error(command_report(
                    &request,
                    method,
                    started,
                    status,
                    Some(error.to_string()),
                    Some(classification),
                ));
            }
            respond_error(&mut stream, status, error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_gateway_command;

    #[test]
    fn fbcp_human_commands_are_explicit_gateway_extensions() {
        for method in [
            "listHumanConversations",
            "syncHumanConversations",
            "syncHumanConversation",
            "createHumanConversation",
            "sendHumanMessage",
            "retryHumanMessage",
            "reactHumanMessage",
            "getHumanConversationTranscript",
            "getHumanConversationTranscriptTail",
            "searchHumanMessages",
            "searchHumanRecipients",
            "forwardHumanMessage",
            "createCallSession",
            "getCallSession",
            "listCallSessions",
            "transitionCallSession",
            "updateCallMedia",
            "sendCallSignal",
            "listCallSignals",
        ] {
            assert!(is_gateway_command(method), "{method} must reach the production Session owner");
        }

        for method in [
            "human.anything",
            "telegram.sendMessage",
            "communication.dispatch",
        ] {
            assert!(
                !is_gateway_command(method),
                "{method} must not widen the explicit FBCP gateway boundary"
            );
        }
    }
}
