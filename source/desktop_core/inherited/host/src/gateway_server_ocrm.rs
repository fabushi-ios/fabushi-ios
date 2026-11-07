//! Grok-aligned OpenClaw Host Gateway protocol and helper operations.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, Cursor, Read, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, RecvTimeoutError, Sender},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::device_pairing_owner::DeviceTokenAuthorization;
use crate::gateway_command_error::{
    GatewayCommandErrorClassification, classify_gateway_command_error,
};
use crate::gateway_protocol::{
    GATEWAY_PREPARE_UPGRADE_PATH, is_grok_gateway_command, parse_command_args,
    slim_command_result, slim_event,
};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectParams {
    pub min_protocol_version: Option<u32>,
    pub max_protocol_version: Option<u32>,
    pub client_id: Option<String>,
    pub client_version: Option<String>,
    pub platform: Option<String>,
    pub role: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub token: Option<String>,
    pub device_id: Option<String>,
    pub device_timestamp: Option<String>,
    pub nonce: Option<String>,
}

pub(crate) fn parse_connect_params(request: &HttpRequest, query: Option<&str>) -> ConnectParams {
    let mut params = ConnectParams::default();
    if let Some(query) = query {
        for part in query.split('&') {
            let mut iter = part.splitn(2, '=');
            let key = iter.next().unwrap_or_default();
            let val = iter.next().unwrap_or_default();
            match key {
                "role" => params.role = Some(val.to_string()),
                "token" => params.token = Some(val.to_string()),
                "deviceId" => params.device_id = Some(val.to_string()),
                "deviceTimestamp" => params.device_timestamp = Some(val.to_string()),
                "nonce" => params.nonce = Some(val.to_string()),
                "minProtocolVersion" => params.min_protocol_version = val.parse().ok(),
                "maxProtocolVersion" => params.max_protocol_version = val.parse().ok(),
                "clientId" => params.client_id = Some(val.to_string()),
                "clientVersion" => params.client_version = Some(val.to_string()),
                "platform" => params.platform = Some(val.to_string()),
                _ => {}
            }
        }
    }
    if let Some(header_val) = request.headers.get("x-sand-connect-params") {
        if let Ok(parsed) = serde_json::from_str::<ConnectParams>(header_val) {
            return parsed;
        }
    }
    if !request.body.is_empty() {
        if let Ok(parsed) = serde_json::from_slice::<ConnectParams>(&request.body) {
            return parsed;
        }
    }
    params
}

pub struct GatewayBridgeSubscription {
    receiver: Receiver<Value>,
    close: Option<GatewayBridgeClose>,
}

impl GatewayBridgeSubscription {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Value, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

impl Drop for GatewayBridgeSubscription {
    fn drop(&mut self) {
        if let Some(close) = self.close.take() {
            close();
        }
    }
}

#[derive(Clone)]
pub struct GatewayBridgeHub {
    request_subscribers: Arc<Mutex<Vec<Sender<Value>>>>,
    response_subscribers: Arc<Mutex<Vec<Sender<Value>>>>,
    on_request_subscribe: Option<GatewayBridgeSubscribeHandler>,
    on_response: Option<GatewayBridgeResponseHandler>,
}

impl Default for GatewayBridgeHub {
    fn default() -> Self {
        Self {
            request_subscribers: Arc::new(Mutex::new(Vec::new())),
            response_subscribers: Arc::new(Mutex::new(Vec::new())),
            on_request_subscribe: None,
            on_response: None,
        }
    }
}

impl GatewayBridgeHub {
    pub fn with_handlers<Subscribe, Response>(
        on_request_subscribe: Subscribe,
        on_response: Response,
    ) -> Self
    where
        Subscribe: Fn(Sender<Value>) -> GatewayBridgeClose + Send + Sync + 'static,
        Response: Fn(Value) + Send + Sync + 'static,
    {
        Self {
            on_request_subscribe: Some(Arc::new(on_request_subscribe)),
            on_response: Some(Arc::new(on_response)),
            ..Self::default()
        }
    }
    pub fn publish_request(&self, frame: Value) {
        if let Ok(mut subscribers) = self.request_subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.send(frame.clone()).is_ok());
        }
    }

    pub fn subscribe_requests(&self) -> GatewayBridgeSubscription {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.request_subscribers.lock() {
            subscribers.push(sender.clone());
        }
        let close = self
            .on_request_subscribe
            .as_ref()
            .map(|handler| handler(sender));
        GatewayBridgeSubscription { receiver, close }
    }

    pub fn submit_responses(&self, batch: Value) {
        if let Some(handler) = self.on_response.as_ref() {
            handler(batch.clone());
        }
        if let Ok(mut subscribers) = self.response_subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.send(batch.clone()).is_ok());
        }
    }

    pub fn subscribe_responses(&self) -> Receiver<Value> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.response_subscribers.lock() {
            subscribers.push(sender);
        }
        receiver
    }

    pub fn request_subscriber_count(&self) -> usize {
        self.request_subscribers
            .lock()
            .map(|subscribers| subscribers.len())
            .unwrap_or_default()
    }

    pub fn response_subscriber_count(&self) -> usize {
        self.response_subscribers
            .lock()
            .map(|subscribers| subscribers.len())
            .unwrap_or_default()
    }
}

pub type GatewayEventHub = crate::host_event_bus::SandHostEventBus;



pub(crate) fn split_target(target: &str) -> (&str, Option<&str>) {
    target
        .split_once('?')
        .map_or((target, None), |(path, query)| (path, Some(query)))
}

pub(crate) fn host_header_hostname(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest.find(']')?;
        return Some(rest[..end].to_string());
    }
    Some(value.split(':').next().unwrap_or(value).to_string())
}

pub(crate) fn check_device_timestamp_skew(timestamp_str: &str) -> Result<(), &'static str> {
    let ts = timestamp_str.parse::<u64>().map_err(|_| "invalid timestamp format")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff = if ts > now { ts - now } else { now - ts };
    if diff > 120 {
        return Err("TIMESTAMP_SKEW: clock skew exceeds +/-120s");
    }
    Ok(())
}

static NONCE_CACHE: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

pub(crate) fn check_and_record_nonce(nonce: &str) -> Result<(), &'static str> {
    if nonce.is_empty() {
        return Err("invalid nonce");
    }
    let mut lock = NONCE_CACHE.lock().unwrap();
    if lock.contains(nonce) {
        return Err("NONCE_REPLAY: nonce has already been used");
    }
    lock.insert(nonce.to_string());
    if lock.len() > 10000 {
        lock.clear();
        lock.insert(nonce.to_string());
    }
    Ok(())
}

pub(crate) fn is_mutating_openclaw_command(method: &str) -> bool {
    matches!(
        method,
        "device.pair.approve"
            | "device.pair.reject"
            | "device.pair.revoke"
            | "device.pair.rotateToken"
            | "device.pair.setupCode"
            | "chat.send"
            | "chat.abort"
            | "sessions.create"
            | "sessions.patch"
            | "sessions.providerReview.continue"
            | "sessions.delete"
            | "cron.trigger"
            | "config.set"
    )
}

pub(crate) fn is_operator_only_openclaw_command(method: &str) -> bool {
    matches!(
        method,
        "device.pair.approve"
            | "device.pair.reject"
            | "device.pair.list"
            | "device.pair.revoke"
            | "device.pair.rotateToken"
            | "device.pair.setupCode"
            | "chat.send"
            | "chat.abort"
            | "sessions.list"
            | "sessions.get"
            | "sessions.create"
            | "sessions.patch"
            | "sessions.providerReview.continue"
            | "sessions.delete"
            | "cron.list"
            | "cron.trigger"
            | "config.get"
            | "config.set"
            | "system.info"
            | "node.status"
            | "node.list"
    )
}

pub(crate) fn bearer_token(request: &HttpRequest) -> Option<&str> {
    request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix(&format!("{GATEWAY_AUTH_SCHEME} ")))
        .filter(|token| !token.is_empty())
}

pub(crate) fn is_authorized_token(provided: &str, expected: &str) -> bool {
    let provided = provided.as_bytes();
    let expected = expected.as_bytes();
    provided.len() == expected.len() && bool::from(provided.ct_eq(expected))
}

pub(crate) fn write_response(
    stream: &mut GatewayStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> io::Result<()> {
    let reason = match status {
        101 => "Switching Protocols",
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        304 => "Not Modified",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    for (name, value) in extra_headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(body)?;
    stream.flush()
}

pub(crate) fn respond_json(stream: &mut GatewayStream, status: u16, value: Value) -> io::Result<()> {
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| io::Error::other(format!("gateway response serialization failed: {error}")))?;
    write_response(
        stream,
        status,
        "application/json",
        &encoded,
        &[(GATEWAY_MINT_DEDUPE_HEADER, "1")],
    )
}

pub(crate) fn client_accepts_gzip(request: &HttpRequest) -> bool {
    request
        .headers
        .get("accept-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("gzip"))
}

pub(crate) fn respond_command_json(
    stream: &mut GatewayStream,
    status: u16,
    value: Value,
    request: &HttpRequest,
) -> io::Result<()> {
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| io::Error::other(format!("gateway response serialization failed: {error}")))?;
    if encoded.len() < GZIP_MIN_BYTES || !client_accepts_gzip(request) {
        return write_response(
            stream,
            status,
            "application/json",
            &encoded,
            &[(GATEWAY_MINT_DEDUPE_HEADER, "1")],
        );
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&encoded)?;
    let zipped = encoder
        .finish()
        .map_err(|error| io::Error::other(format!("gateway gzip finish failed: {error}")))?;
    write_response(
        stream,
        status,
        "application/json",
        &zipped,
        &[
            ("Content-Encoding", "gzip"),
            ("Vary", "Accept-Encoding"),
            (GATEWAY_MINT_DEDUPE_HEADER, "1"),
        ],
    )
}

pub(crate) fn respond_error(stream: &mut GatewayStream, status: u16, message: impl Into<String>) -> io::Result<()> {
    let encoded = serde_json::to_vec(&json!({ "error": message.into() }))
        .map_err(|error| io::Error::other(format!("gateway error serialization failed: {error}")))?;
    write_response(stream, status, "application/json", &encoded, &[])
}

pub(crate) fn respond_pairing_error(
    stream: &mut GatewayStream,
    status: u16,
    message: impl Into<String>,
) -> io::Result<()> {
    let encoded = serde_json::to_vec(&json!({ "error": message.into() }))
        .map_err(|error| io::Error::other(format!("gateway error serialization failed: {error}")))?;
    write_response(
        stream,
        status,
        "application/json",
        &encoded,
        &[("Cache-Control", "no-store")],
    )
}

pub(crate) fn parse_channels(query: Option<&str>) -> Option<Vec<String>> {
    let raw = query?
        .split('&')
        .find_map(|part| part.split_once('=').filter(|(key, _)| *key == "channels").map(|(_, value)| value))?;
    let channels = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!channels.is_empty()).then_some(channels)
}

pub(crate) fn event_channel(event: &Value) -> Option<&str> {
    event.get("channel").and_then(Value::as_str)
}

pub(crate) fn valid_traceparent(value: &str) -> bool {
    let mut parts = value.split('-');
    let Some(version) = parts.next() else { return false };
    let Some(trace_id) = parts.next() else { return false };
    let Some(parent_id) = parts.next() else { return false };
    let Some(flags) = parts.next() else { return false };
    if parts.next().is_some()
        || version.len() != 2
        || trace_id.len() != 32
        || parent_id.len() != 16
        || flags.len() != 2
        || ![version, trace_id, parent_id, flags]
            .into_iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return false;
    }
    !trace_id.bytes().all(|byte| byte == b'0') && !parent_id.bytes().all(|byte| byte == b'0')
}

pub(crate) fn gateway_command_context(
    request: &HttpRequest,
    authenticated: bool,
    dispatch_started: Instant,
) -> GatewayCommandContext {
    let request_id = request
        .headers
        .get(GATEWAY_REQUEST_ID_HEADER)
        .filter(|value| !value.is_empty())
        .cloned();
    let traceparent = request
        .headers
        .get(GATEWAY_TRACEPARENT_HEADER)
        .filter(|value| valid_traceparent(value))
        .cloned();
    let (trace_id, span_id) = traceparent
        .as_deref()
        .and_then(|value| {
            let mut parts = value.split('-');
            let _version = parts.next()?;
            let trace_id = parts.next()?.to_string();
            let span_id = parts.next()?.to_string();
            Some((trace_id, span_id))
        })
        .map(|(trace_id, span_id)| (Some(trace_id), Some(span_id)))
        .unwrap_or((None, None));
    GatewayCommandContext {
        request_id,
        traceparent,
        trace_id,
        span_id,
        authenticated,
        dispatch_started,
    }
}

pub(crate) fn command_report(
    request: &HttpRequest,
    method: &str,
    started: Instant,
    status: u16,
    error: Option<String>,
    classification: Option<GatewayCommandErrorClassification>,
) -> GatewayCommandReport {
    let (reason, error_class, errno) = classification
        .map(|value| {
            (
                Some(value.reason.to_string()),
                Some(value.error_class),
                value.errno,
            )
        })
        .unwrap_or((None, None, None));
    GatewayCommandReport {
        method: method.to_string(),
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        request_id: request
            .headers
            .get(GATEWAY_REQUEST_ID_HEADER)
            .filter(|value| !value.is_empty())
            .cloned(),
        traceparent: request
            .headers
            .get(GATEWAY_TRACEPARENT_HEADER)
            .filter(|value| valid_traceparent(value))
            .cloned(),
        status,
        error,
        reason,
        error_class,
        errno,
    }
}

pub(crate) fn sse_gzip_enabled(request: &HttpRequest) -> bool {
    std::env::var(DISABLE_SSE_GZIP_ENV).ok().as_deref() != Some("1")
        && client_accepts_gzip(request)
}

pub(crate) fn write_sse_headers(stream: &mut GatewayStream, gzip: bool) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache, no-transform\r\nConnection: keep-alive\r\n"
    )?;
    if gzip {
        write!(
            stream,
            "Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n"
        )?;
    }
    write!(stream, "\r\n")?;
    stream.flush()
}

pub(crate) fn serve_event_body<W: Write>(
    sink: &mut W,
    receiver: Receiver<Value>,
    stop: &AtomicBool,
    channels: Option<Vec<String>>,
    slim_avatars: bool,
) -> io::Result<()> {
    sink.write_all(b"retry: 1000\n\n")?;
    sink.flush()?;
    let heartbeat = Duration::from_millis(SSE_HEARTBEAT_MS);
    while !stop.load(Ordering::Acquire) {
        match receiver.recv_timeout(heartbeat) {
            Ok(event) => {
                if channels
                    .as_ref()
                    .is_some_and(|allowed| event_channel(&event).is_some_and(|channel| !allowed.iter().any(|value| value == channel)))
                {
                    continue;
                }
                let event = if slim_avatars { slim_event(event) } else { event };
                let encoded = serde_json::to_string(&event)
                    .map_err(|error| io::Error::other(format!("serialize gateway event: {error}")))?;
                write!(sink, "data: {encoded}\n\n")?;
                sink.flush()?;
            }
            Err(RecvTimeoutError::Timeout) => {
                sink.write_all(b":ping\n\n")?;
                sink.flush()?;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

pub(crate) fn serve_events(
    stream: &mut GatewayStream,
    deps: &GatewayServerDeps,
    stop: &AtomicBool,
    channels: Option<Vec<String>>,
    slim_avatars: bool,
    gzip: bool,
) -> io::Result<()> {
    let receiver = deps.events.subscribe();
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_sse_headers(stream, gzip)?;
    if gzip {
        let mut encoder = GzEncoder::new(stream, Compression::default());
        let result = serve_event_body(&mut encoder, receiver, stop, channels, slim_avatars);
        let _ = encoder.try_finish();
        result
    } else {
        serve_event_body(stream, receiver, stop, channels, slim_avatars)
    }
}

pub(crate) fn decode_path_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            let high = digit(bytes[index + 1])?;
            let low = digit(bytes[index + 2])?;
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).ok()
}

pub(crate) fn query_value<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    })
}

pub(crate) fn serve_avatar(
    stream: &mut GatewayStream,
    deps: &GatewayServerDeps,
    request: &HttpRequest,
    path: &str,
    query: Option<&str>,
) -> io::Result<()> {
    if request
        .headers
        .get("sec-fetch-site")
        .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return respond_error(stream, 403, "cross-site avatar loads are not allowed");
    }
    let encoded_id = path
        .strip_prefix(&format!("{GATEWAY_AVATARS_PATH}/"))
        .unwrap_or_default();
    let Some(agent_id) = decode_path_component(encoded_id).filter(|value| !value.is_empty()) else {
        return respond_error(stream, 404, "missing agent id");
    };
    let avatar = match deps.api.call("getAgentAvatar", json!({ "id": agent_id })) {
        Ok(value) => value,
        Err(error) => return respond_error(stream, error.status(), error.to_string()),
    };
    let version = avatar.get("version").and_then(Value::as_str);
    let data_url = avatar.get("dataUrl").and_then(Value::as_str);
    let (Some(version), Some(data_url)) = (version, data_url) else {
        return respond_error(stream, 404, "agent has no avatar");
    };
    if query_value(query, "v").is_some_and(|requested| requested != version) {
        return respond_error(stream, 404, "no such avatar version");
    }
    let Some(rest) = data_url.strip_prefix("data:") else {
        return respond_error(stream, 404, "agent has no avatar");
    };
    let Some((mime, encoded)) = rest.split_once(";base64,") else {
        return respond_error(stream, 404, "agent has no avatar");
    };
    if mime.is_empty() {
        return respond_error(stream, 404, "agent has no avatar");
    }
    let bytes = match BASE64_STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(_) => return respond_error(stream, 404, "agent has no avatar"),
    };
    let etag = format!("\"{version}\"");
    let immutable = query_value(query, "v").is_some();
    if request.headers.get("if-none-match").is_some_and(|value| value == &etag) {
        return write_response(
            stream,
            304,
            mime,
            &[],
            &[
                ("Cache-Control", if immutable { "private, max-age=31536000, immutable" } else { "no-store" }),
                ("ETag", &etag),
            ],
        );
    }
    write_response(
        stream,
        200,
        mime,
        &bytes,
        &[
            ("Cache-Control", if immutable { "private, max-age=31536000, immutable" } else { "no-store" }),
            ("ETag", &etag),
            ("Content-Disposition", "attachment"),
            ("X-Content-Type-Options", "nosniff"),
            ("Content-Security-Policy", "default-src 'none'; sandbox"),
        ],
    )
}

pub(crate) fn serve_bridge_body<W: Write>(
    sink: &mut W,
    subscription: GatewayBridgeSubscription,
    stop: &AtomicBool,
) -> io::Result<()> {
    sink.write_all(b"retry: 1000\n\n")?;
    sink.flush()?;
    let heartbeat = Duration::from_millis(SSE_HEARTBEAT_MS);
    while !stop.load(Ordering::Acquire) {
        match subscription.recv_timeout(heartbeat) {
            Ok(frame) => {
                let encoded = serde_json::to_string(&frame)
                    .map_err(|error| io::Error::other(format!("serialize gateway bridge frame: {error}")))?;
                write!(sink, "data: {encoded}\n\n")?;
                sink.flush()?;
            }
            Err(RecvTimeoutError::Timeout) => {
                sink.write_all(b":ping\n\n")?;
                sink.flush()?;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

pub(crate) fn serve_bridge_requests(
    stream: &mut GatewayStream,
    bridge: &GatewayBridgeHub,
    stop: &AtomicBool,
    gzip: bool,
) -> io::Result<()> {
    let receiver = bridge.subscribe_requests();
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    write_sse_headers(stream, gzip)?;
    if gzip {
        let mut encoder = GzEncoder::new(stream, Compression::default());
        let result = serve_bridge_body(&mut encoder, receiver, stop);
        let _ = encoder.try_finish();
        result
    } else {
        serve_bridge_body(stream, receiver, stop)
    }
}

pub(crate) fn submit_bridge_responses(
    stream: &mut GatewayStream,
    bridge: &GatewayBridgeHub,
    body: &[u8],
) -> io::Result<()> {
    let batch = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(body) {
            Ok(value) => value,
            Err(error) => {
                return respond_error(stream, 400, format!("invalid JSON body: {error}"));
            }
        }
    };
    bridge.submit_responses(batch);
    respond_json(stream, 200, json!({ "ok": true }))
}


