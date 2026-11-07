use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use super::gateway_reachability::ReachabilityOutcome;
use super::super::cloud_worker_projection::{
    CLOUD_WORKERS_CAPABILITIES_METHOD, CLOUD_WORKERS_DOCTOR_METHOD,
    CLOUD_WORKERS_STATUS_METHOD, CloudWorkerCapabilitiesProjection,
    CloudWorkerDoctorProjection, CloudWorkerStatusProjection,
};
use super::gateway_request_dispatcher::{
    GatewayDispatchError, GatewayJsonResponse, dispatch_http_json_response,
};
use super::gateway_errors::SandGatewayCommandError;
use super::host_supervisor::GatewayConnection;
use super::http_transport::parse_gateway_http_base;
use super::sse_block_decoder::SseBlockDecoder;

pub const SSE_RECONNECT_MIN_MS: u64 = 1_000;
pub const SSE_RECONNECT_MAX_MS: u64 = 10_000;
pub const SSE_STALL_TIMEOUT_MS: u64 = 35_000;
pub const SSE_CONNECT_TIMEOUT_MS: u64 = 15_000;
pub const SEND_POST_TIMEOUT_MS: u64 = 15_000;
pub const ROSTER_READ_TIMEOUT_MS: u64 = 15_000;
pub const TRACE_WINDOW_ROOT_CACHE_MS: u64 = 5_000;
pub const CREATE_AGENT_RETRY_MAX_ATTEMPTS: u32 = 3;
pub const CREATE_AGENT_RETRY_INITIAL_MS: u64 = 1_000;
pub const CREATE_AGENT_RETRY_MAX_MS: u64 = 4_000;
pub const HOST_ACCOUNT_SLOT: &str = "host";
pub const GATEWAY_MINT_DEDUPE_HEADER: &str = "x-sand-mint-dedupe";
pub const GATEWAY_TRACEPARENT_HEADER: &str = "traceparent";
pub const DISABLE_SEND_ACCEPT_RETURN_ENV: &str = "SAND_DISABLE_SEND_ACCEPT_RETURN";
pub const SEND_POST_TIMEOUT_ENV: &str = "SAND_SEND_POST_TIMEOUT_MS";
pub const ROSTER_READ_TIMEOUT_ENV: &str = "SAND_ROSTER_READ_TIMEOUT_MS";
pub const GATEWAY_SLIM_AVATARS_HEADER: &str = "x-sand-slim-avatars";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayClientTiming {
    pub sse_reconnect_min_ms: u64,
    pub sse_reconnect_max_ms: u64,
    pub sse_stall_timeout_ms: u64,
    pub sse_connect_timeout_ms: u64,
    pub send_post_timeout_ms: u64,
    pub roster_read_timeout_ms: u64,
    pub create_agent_retry_max_attempts: u32,
    pub create_agent_retry_initial_ms: u64,
    pub create_agent_retry_max_ms: u64,
}

impl Default for GatewayClientTiming {
    fn default() -> Self {
        Self {
            sse_reconnect_min_ms: SSE_RECONNECT_MIN_MS,
            sse_reconnect_max_ms: SSE_RECONNECT_MAX_MS,
            sse_stall_timeout_ms: SSE_STALL_TIMEOUT_MS,
            sse_connect_timeout_ms: SSE_CONNECT_TIMEOUT_MS,
            send_post_timeout_ms: env_timeout_ms(SEND_POST_TIMEOUT_ENV, SEND_POST_TIMEOUT_MS),
            roster_read_timeout_ms: env_timeout_ms(ROSTER_READ_TIMEOUT_ENV, ROSTER_READ_TIMEOUT_MS),
            create_agent_retry_max_attempts: CREATE_AGENT_RETRY_MAX_ATTEMPTS,
            create_agent_retry_initial_ms: CREATE_AGENT_RETRY_INITIAL_MS,
            create_agent_retry_max_ms: CREATE_AGENT_RETRY_MAX_MS,
        }
    }
}

fn env_timeout_ms(name: &str, fallback: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}


pub fn is_permanent_refusal(outcome: ReachabilityOutcome) -> bool {
    matches!(
        outcome,
        ReachabilityOutcome::NoStorage
            | ReachabilityOutcome::BoxBlocked
            | ReachabilityOutcome::AccessDenied
    )
}

pub fn is_pre_dispatch(outcome: ReachabilityOutcome) -> bool {
    matches!(outcome, ReachabilityOutcome::Refused | ReachabilityOutcome::Dns)
}

pub fn extract_gateway_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("error")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("gateway endpoint changed from {previous} to {next}")]
pub struct GatewayEndpointChangedError {
    pub previous: String,
    pub next: String,
}

pub fn with_auth(
    headers: &BTreeMap<String, String>,
    token: Option<&str>,
) -> BTreeMap<String, String> {
    let mut output = headers.clone();
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        output.insert("authorization".into(), format!("Bearer {token}"));
    }
    output
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayEvent {
    pub channel: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayClientState {
    pub connected: bool,
    pub reconnect_attempt: u32,
    pub endpoint_generation: u64,
    pub stream_live: bool,
    pub last_event_ms: Option<u64>,
    pub last_outcome: Option<ReachabilityOutcome>,
    pub connection: Option<GatewayConnection>,
}

impl Default for GatewayClientState {
    fn default() -> Self {
        Self {
            connected: false,
            reconnect_attempt: 0,
            endpoint_generation: 0,
            stream_live: false,
            last_event_ms: None,
            last_outcome: None,
            connection: None,
        }
    }
}

impl GatewayClientState {
    pub fn install_connection(
        &mut self,
        connection: GatewayConnection,
    ) -> Result<u64, GatewayEndpointChangedError> {
        if let Some(previous) = &self.connection {
            if previous.base_url != connection.base_url && self.connected {
                return Err(GatewayEndpointChangedError {
                    previous: previous.base_url.clone(),
                    next: connection.base_url,
                });
            }
        }
        let changed = self
            .connection
            .as_ref()
            .is_none_or(|previous| previous.base_url != connection.base_url);
        if changed {
            self.endpoint_generation = self.endpoint_generation.saturating_add(1);
        }
        self.connection = Some(connection);
        Ok(self.endpoint_generation)
    }

    pub fn connected(&mut self, now_ms: u64) {
        self.connected = true;
        self.stream_live = true;
        self.reconnect_attempt = 0;
        self.last_outcome = None;
        self.last_event_ms = Some(now_ms);
    }

    pub fn mark_event(&mut self, now_ms: u64) {
        self.connected = true;
        self.stream_live = true;
        self.last_event_ms = Some(now_ms);
    }

    pub fn stream_stalled(&self, now_ms: u64, timing: GatewayClientTiming) -> bool {
        self.stream_live
            && self
                .last_event_ms
                .is_some_and(|last| now_ms.saturating_sub(last) >= timing.sse_stall_timeout_ms)
    }

    pub fn disconnected(&mut self, outcome: ReachabilityOutcome) -> Option<Duration> {
        self.connected = false;
        self.stream_live = false;
        self.last_outcome = Some(outcome);
        if is_permanent_refusal(outcome) || !outcome.retryable() {
            return None;
        }
        self.reconnect_attempt = self.reconnect_attempt.saturating_add(1);
        let exponent = self.reconnect_attempt.saturating_sub(1).min(4);
        let delay_ms = SSE_RECONNECT_MIN_MS
            .saturating_mul(1_u64 << exponent)
            .min(SSE_RECONNECT_MAX_MS);
        Some(Duration::from_millis(delay_ms))
    }

    pub fn force_reconnect(&mut self) {
        self.connected = false;
        self.stream_live = false;
        self.last_event_ms = None;
    }
}

#[derive(Debug)]
pub struct CoordinatorGatewayClient {
    pub state: GatewayClientState,
    pub timing: GatewayClientTiming,
    closed: bool,
    dev_induced_offline: bool,
    client_paused: bool,
}

impl Default for CoordinatorGatewayClient {
    fn default() -> Self {
        Self {
            state: GatewayClientState::default(),
            timing: GatewayClientTiming::default(),
            closed: false,
            dev_induced_offline: false,
            client_paused: false,
        }
    }
}

impl CoordinatorGatewayClient {
    pub fn install_connection(
        &mut self,
        connection: GatewayConnection,
    ) -> Result<u64, GatewayEndpointChangedError> {
        if self.closed {
            return Err(GatewayEndpointChangedError {
                previous: "closed".into(),
                next: connection.base_url,
            });
        }
        self.state.install_connection(connection)
    }

    pub fn start(&mut self, now_ms: u64) -> Result<(), &'static str> {
        if self.closed {
            return Err("gateway client is closed");
        }
        if self.state.connection.is_none() {
            return Err("gateway connection is unresolved");
        }
        self.state.connected(now_ms);
        Ok(())
    }

    pub fn set_dev_induced_offline(&mut self, induced: bool) -> bool {
        if self.dev_induced_offline != induced {
            self.dev_induced_offline = induced;
            self.state.force_reconnect();
            if induced {
                self.state.last_outcome = Some(ReachabilityOutcome::Network);
            }
        }
        self.dev_induced_offline
    }

    pub fn set_client_paused(&mut self, paused: bool) -> bool {
        if self.client_paused != paused {
            self.client_paused = paused;
            self.state.force_reconnect();
            if paused {
                self.state.last_outcome = Some(ReachabilityOutcome::BoxBlocked);
            }
        }
        self.client_paused
    }

    pub fn is_transport_suppressed(&self) -> bool {
        self.dev_induced_offline || self.client_paused
    }

    pub fn connection_for_dispatch(&self) -> Result<&GatewayConnection, GatewayDispatchError> {
        if self.closed {
            return Err(GatewayDispatchError::Transport(
                "gateway client is closed".into(),
            ));
        }
        if self.client_paused {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::BoxBlocked,
                message: "gateway transport is paused by the client".into(),
            });
        }
        if self.dev_induced_offline {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway transport is offline by dev control".into(),
            });
        }
        if !self.state.connected {
            return Err(GatewayDispatchError::Unreachable {
                outcome: self.state.last_outcome.unwrap_or(ReachabilityOutcome::Network),
                message: "gateway transport is not connected".into(),
            });
        }
        self.state.connection.as_ref().ok_or_else(|| {
            GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway connection is unresolved".into(),
            }
        })
    }

    pub fn dispatch_command<F>(
        &mut self,
        method: &str,
        args: Value,
        dispatch: F,
    ) -> Result<Value, GatewayDispatchError>
    where
        F: FnOnce(&GatewayConnection, &str, Value) -> Result<Value, GatewayDispatchError>,
    {
        if self.closed {
            return Err(GatewayDispatchError::Transport(
                "gateway client is closed".into(),
            ));
        }
        if self.client_paused {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::BoxBlocked,
                message: "gateway transport is paused by the client".into(),
            });
        }
        if self.dev_induced_offline {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway transport is offline by dev control".into(),
            });
        }
        let connection = self.state.connection.as_ref().ok_or_else(|| {
            GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway connection is unresolved".into(),
            }
        })?;
        let result = dispatch(connection, method, args);
        if let Err(GatewayDispatchError::Unreachable { outcome, .. }) = &result {
            self.state.disconnected(*outcome);
        }
        result
    }

    pub fn dispatch_cloud_worker_status<F>(
        &mut self,
        dispatch: F,
    ) -> Result<CloudWorkerStatusProjection, GatewayDispatchError>
    where
        F: FnOnce(&GatewayConnection, &str, Value) -> Result<Value, GatewayDispatchError>,
    {
        let value = self.dispatch_command(
            CLOUD_WORKERS_STATUS_METHOD,
            Value::Object(Default::default()),
            dispatch,
        )?;
        let projection: CloudWorkerStatusProjection = serde_json::from_value(value).map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker status projection is invalid: {error}"
            )))
        })?;
        projection.validate().map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker status projection is invalid: {error}"
            )))
        })?;
        Ok(projection)
    }

    pub fn dispatch_cloud_worker_doctor<F>(
        &mut self,
        args: Value,
        dispatch: F,
    ) -> Result<CloudWorkerDoctorProjection, GatewayDispatchError>
    where
        F: FnOnce(&GatewayConnection, &str, Value) -> Result<Value, GatewayDispatchError>,
    {
        let value = self.dispatch_command(CLOUD_WORKERS_DOCTOR_METHOD, args, dispatch)?;
        let projection: CloudWorkerDoctorProjection = serde_json::from_value(value).map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker doctor projection is invalid: {error}"
            )))
        })?;
        projection.validate().map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker doctor projection is invalid: {error}"
            )))
        })?;
        Ok(projection)
    }

    pub fn dispatch_cloud_worker_capabilities<F>(
        &mut self,
        dispatch: F,
    ) -> Result<CloudWorkerCapabilitiesProjection, GatewayDispatchError>
    where
        F: FnOnce(&GatewayConnection, &str, Value) -> Result<Value, GatewayDispatchError>,
    {
        let value = self.dispatch_command(
            CLOUD_WORKERS_CAPABILITIES_METHOD,
            Value::Object(Default::default()),
            dispatch,
        )?;
        let projection: CloudWorkerCapabilitiesProjection = serde_json::from_value(value).map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker capabilities projection is invalid: {error}"
            )))
        })?;
        projection.validate().map_err(|error| {
            GatewayDispatchError::Command(SandGatewayCommandError::new(format!(
                "cloud worker capabilities projection is invalid: {error}"
            )))
        })?;
        Ok(projection)
    }

    pub fn accept_event(
        &mut self,
        now_ms: u64,
        channel: impl Into<String>,
        payload: Value,
    ) -> Option<GatewayEvent> {
        if self.closed || self.is_transport_suppressed() {
            return None;
        }
        self.state.mark_event(now_ms);
        Some(GatewayEvent {
            channel: channel.into(),
            payload,
        })
    }

    pub fn transport_down(
        &mut self,
        outcome: ReachabilityOutcome,
    ) -> Option<Duration> {
        self.state.disconnected(outcome)
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.state.force_reconnect();
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCommandSpan {
    pub method: String,
    pub root_traceparent: String,
    pub span_id: String,
    pub start_epoch_ms: u64,
    pub duration_ms: u64,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTransportStage {
    pub account_slot: String,
    pub client_nonce: String,
    pub stage: String,
    pub attempt: u32,
    pub traceparent: Option<String>,
    pub start_epoch_ms: u64,
    pub duration_ms: u64,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayCommandExecution {
    pub value: Value,
    pub command_spans: Vec<GatewayCommandSpan>,
    pub transport_stages: Vec<GatewayTransportStage>,
}

#[derive(Debug, Clone)]
struct CachedTraceWindowRoot {
    root: Option<String>,
    expires_at_ms: u64,
}

#[derive(Debug)]
pub struct GatewayCommandPolicy {
    pub timing: GatewayClientTiming,
    send_accept_return_disabled: bool,
    mint_dedupe_proven_base_urls: Mutex<HashSet<String>>,
    send_dedupe_proven_base_urls: Mutex<HashSet<String>>,
    trace_window_root: Mutex<Option<CachedTraceWindowRoot>>,
}

impl Default for GatewayCommandPolicy {
    fn default() -> Self {
        Self::with_timing(GatewayClientTiming::default())
    }
}

impl GatewayCommandPolicy {
    pub fn with_timing(timing: GatewayClientTiming) -> Self {
        Self {
            timing,
            send_accept_return_disabled: env::var(DISABLE_SEND_ACCEPT_RETURN_ENV)
                .ok()
                .as_deref()
                == Some("1"),
            mint_dedupe_proven_base_urls: Mutex::new(HashSet::new()),
            send_dedupe_proven_base_urls: Mutex::new(HashSet::new()),
            trace_window_root: Mutex::new(None),
        }
    }

    pub fn trace_window_root_needs_refresh(&self, now_ms: u64) -> bool {
        self.trace_window_root
            .lock()
            .map(|cached| {
                cached
                    .as_ref()
                    .is_none_or(|cached| now_ms >= cached.expires_at_ms)
            })
            .unwrap_or(false)
    }

    pub fn cache_trace_window_root(&self, root: Option<String>, now_ms: u64) {
        if let Ok(mut cached) = self.trace_window_root.lock() {
            *cached = Some(CachedTraceWindowRoot {
                root: root.filter(|value| !value.is_empty()),
                expires_at_ms: now_ms.saturating_add(TRACE_WINDOW_ROOT_CACHE_MS),
            });
        }
    }

    pub fn cached_trace_window_root(&self, now_ms: u64) -> Option<String> {
        self.trace_window_root
            .lock()
            .ok()
            .and_then(|cached| cached.clone())
            .filter(|cached| now_ms < cached.expires_at_ms)
            .and_then(|cached| cached.root)
    }

    pub fn mint_dedupe_proven(&self, base_url: &str) -> bool {
        self.mint_dedupe_proven_base_urls
            .lock()
            .map(|urls| urls.contains(base_url))
            .unwrap_or(false)
    }

    pub fn send_dedupe_proven(&self, base_url: &str) -> bool {
        self.send_dedupe_proven_base_urls
            .lock()
            .map(|urls| urls.contains(base_url))
            .unwrap_or(false)
    }

    fn mark_mint_dedupe(&self, base_url: &str) {
        if let Ok(mut urls) = self.mint_dedupe_proven_base_urls.lock() {
            urls.insert(base_url.to_string());
        }
    }

    fn mark_send_dedupe(&self, base_url: &str) {
        if let Ok(mut urls) = self.send_dedupe_proven_base_urls.lock() {
            urls.insert(base_url.to_string());
        }
    }
}

fn retryable_command_error(error: &GatewayDispatchError) -> bool {
    match error {
        GatewayDispatchError::Command(_) => false,
        GatewayDispatchError::Unreachable { outcome, .. } => {
            !is_permanent_refusal(*outcome)
        }
        GatewayDispatchError::Transport(_) => true,
    }
}

fn retry_delay_ms(timing: GatewayClientTiming, completed_attempts: u32) -> u64 {
    let exponent = completed_attempts.saturating_sub(1).min(8);
    timing
        .create_agent_retry_initial_ms
        .saturating_mul(1_u64 << exponent)
        .min(timing.create_agent_retry_max_ms)
}

fn derive_child_traceparent(root: &str) -> Option<(String, String)> {
    let parts = root.split('-').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0].len() != 2
        || parts[1].len() != 32
        || parts[2].len() != 16
        || parts[3].len() != 2
        || !parts.iter().all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return None;
    }
    let span_id = Uuid::new_v4().simple().to_string()[..16].to_string();
    Some((
        format!("{}-{}-{}-{}", parts[0], parts[1], span_id, parts[3]),
        span_id,
    ))
}

fn request_with_policy(
    policy: &GatewayCommandPolicy,
    connection: &GatewayConnection,
    method: &str,
    args: Value,
    timeout_ms: u64,
    now_ms: u64,
    command_spans: &mut Vec<GatewayCommandSpan>,
) -> Result<GatewayJsonResponse, GatewayDispatchError> {
    let trace_root = policy.cached_trace_window_root(now_ms);
    let trace = trace_root
        .as_deref()
        .and_then(derive_child_traceparent);
    let mut extra_headers = BTreeMap::new();
    if let Some((traceparent, _)) = trace.as_ref() {
        extra_headers.insert(GATEWAY_TRACEPARENT_HEADER.to_string(), traceparent.clone());
    } else if let Some(root) = trace_root.as_ref() {
        extra_headers.insert(GATEWAY_TRACEPARENT_HEADER.to_string(), root.clone());
    }

    let start_epoch_ms = u64::try_from(Utc::now().timestamp_millis()).unwrap_or_default();
    let started = Instant::now();
    let result = dispatch_http_json_response(
        connection,
        method,
        args,
        Duration::from_millis(timeout_ms),
        &extra_headers,
    );
    if let (Some(root), Some((_, span_id))) = (trace_root, trace) {
        command_spans.push(GatewayCommandSpan {
            method: method.to_string(),
            root_traceparent: root,
            span_id,
            start_epoch_ms,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            is_error: result.is_err(),
        });
    }
    if let Ok(response) = &result {
        if response.header(GATEWAY_MINT_DEDUPE_HEADER) == Some("1") {
            policy.mark_mint_dedupe(&connection.base_url);
        }
    }
    result
}

fn transport_identity(args: &Value) -> (Option<String>, Option<String>) {
    let client_nonce = args
        .get("clientNonce")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let traceparent = args
        .get("traceparent")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (client_nonce, traceparent)
}

fn push_send_stage(
    stages: &mut Vec<GatewayTransportStage>,
    client_nonce: Option<&str>,
    traceparent: Option<&str>,
    stage: &str,
    attempt: u32,
    start_epoch_ms: u64,
    started: Instant,
    is_error: bool,
) {
    let Some(client_nonce) = client_nonce else {
        return;
    };
    stages.push(GatewayTransportStage {
        account_slot: HOST_ACCOUNT_SLOT.to_string(),
        client_nonce: client_nonce.to_string(),
        stage: stage.to_string(),
        attempt,
        traceparent: traceparent.map(str::to_string),
        start_epoch_ms,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        is_error,
    });
}

pub fn dispatch_gateway_command<Resolve>(
    policy: &GatewayCommandPolicy,
    method: &str,
    args: Value,
    now_ms: u64,
    mut resolve_connection: Resolve,
) -> Result<GatewayCommandExecution, GatewayDispatchError>
where
    Resolve: FnMut(Option<&str>) -> Result<GatewayConnection, GatewayDispatchError>,
{
    let mut command_spans = Vec::new();
    let mut transport_stages = Vec::new();

    if method == "sendPrompt" && !policy.send_accept_return_disabled {
        let (client_nonce, traceparent) = transport_identity(&args);
        let mut required_base_url: Option<String> = None;
        for attempt in 0..2_u32 {
            let connect_epoch_ms =
                u64::try_from(Utc::now().timestamp_millis()).unwrap_or_default();
            let connect_started = Instant::now();
            let connection = match resolve_connection(required_base_url.as_deref()) {
                Ok(connection) => {
                    if let Some(required) = required_base_url.as_deref() {
                        if connection.base_url != required {
                            return Err(GatewayDispatchError::Transport(format!(
                                "send retry aborted: gateway endpoint changed from {required} to {}",
                                connection.base_url
                            )));
                        }
                    }
                    push_send_stage(
                        &mut transport_stages,
                        client_nonce.as_deref(),
                        traceparent.as_deref(),
                        "gateway-connect",
                        attempt,
                        connect_epoch_ms,
                        connect_started,
                        false,
                    );
                    connection
                }
                Err(error) => {
                    push_send_stage(
                        &mut transport_stages,
                        client_nonce.as_deref(),
                        traceparent.as_deref(),
                        "gateway-connect",
                        attempt,
                        connect_epoch_ms,
                        connect_started,
                        true,
                    );
                    let retry = attempt == 0
                        && client_nonce.is_some()
                        && retryable_command_error(&error);
                    if retry {
                        continue;
                    }
                    return Err(error);
                }
            };

            let post_epoch_ms =
                u64::try_from(Utc::now().timestamp_millis()).unwrap_or_default();
            let post_started = Instant::now();
            let result = request_with_policy(
                policy,
                &connection,
                method,
                args.clone(),
                policy.timing.send_post_timeout_ms,
                now_ms,
                &mut command_spans,
            );
            push_send_stage(
                &mut transport_stages,
                client_nonce.as_deref(),
                traceparent.as_deref(),
                "gateway-post",
                attempt,
                post_epoch_ms,
                post_started,
                result.is_err(),
            );
            match result {
                Ok(response) => {
                    if response
                        .value
                        .get("accepted")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        policy.mark_send_dedupe(&connection.base_url);
                    }
                    return Ok(GatewayCommandExecution {
                        value: response.value,
                        command_spans,
                        transport_stages,
                    });
                }
                Err(error) => {
                    let retry = attempt == 0
                        && client_nonce.is_some()
                        && retryable_command_error(&error)
                        && policy.send_dedupe_proven(&connection.base_url);
                    if retry {
                        required_base_url = Some(connection.base_url);
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        unreachable!("sendPrompt retry loop must settle");
    }

    if matches!(method, "listAgents" | "countAgents") {
        let mut last_error = None;
        for attempt in 0..2_u32 {
            let connection = match resolve_connection(None) {
                Ok(connection) => connection,
                Err(error) => {
                    let retry = attempt == 0 && retryable_command_error(&error);
                    if retry {
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            };
            match request_with_policy(
                policy,
                &connection,
                method,
                serde_json::json!({}),
                policy.timing.roster_read_timeout_ms,
                now_ms,
                &mut command_spans,
            ) {
                Ok(response) => {
                    return Ok(GatewayCommandExecution {
                        value: response.value,
                        command_spans,
                        transport_stages,
                    });
                }
                Err(error) => {
                    let retry = attempt == 0 && retryable_command_error(&error);
                    if retry {
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        return Err(last_error.unwrap_or_else(|| {
            GatewayDispatchError::Transport("bounded roster read exhausted".into())
        }));
    }

    if method == "createAgent" {
        let mut stamped = args;
        if let Some(object) = stamped.as_object_mut() {
            let needs_nonce = object
                .get("clientNonce")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            if needs_nonce {
                object.insert(
                    "clientNonce".into(),
                    Value::String(Uuid::new_v4().to_string()),
                );
            }
        }
        let mut pinned_base_url: Option<String> = None;
        let mut last_error = None;
        let max_attempts = policy.timing.create_agent_retry_max_attempts.max(1);
        for attempt in 1..=max_attempts {
            let connection = match resolve_connection(pinned_base_url.as_deref()) {
                Ok(connection) => {
                    if let Some(required) = pinned_base_url.as_deref() {
                        if connection.base_url != required {
                            return Err(last_error.unwrap_or_else(|| {
                                GatewayDispatchError::Transport(format!(
                                    "createAgent retry aborted: gateway endpoint changed from {required} to {}",
                                    connection.base_url
                                ))
                            }));
                        }
                    }
                    connection
                }
                Err(error) => {
                    let retry = attempt < max_attempts
                        && retryable_command_error(&error);
                    if !retry {
                        return Err(error);
                    }
                    last_error = Some(error);
                    thread_sleep_retry(policy.timing, attempt);
                    continue;
                }
            };

            match request_with_policy(
                policy,
                &connection,
                method,
                stamped.clone(),
                policy.timing.send_post_timeout_ms,
                now_ms,
                &mut command_spans,
            ) {
                Ok(response) => {
                    return Ok(GatewayCommandExecution {
                        value: response.value,
                        command_spans,
                        transport_stages,
                    });
                }
                Err(error) => {
                    let retry = attempt < max_attempts
                        && retryable_command_error(&error)
                        && policy.mint_dedupe_proven(&connection.base_url);
                    if !retry {
                        return Err(error);
                    }
                    pinned_base_url = Some(connection.base_url);
                    last_error = Some(error);
                    thread_sleep_retry(policy.timing, attempt);
                }
            }
        }
        return Err(last_error.unwrap_or_else(|| {
            GatewayDispatchError::Transport("createAgent retry policy exhausted".into())
        }));
    }

    let connection = resolve_connection(None)?;
    let response = request_with_policy(
        policy,
        &connection,
        method,
        args,
        policy.timing.send_post_timeout_ms,
        now_ms,
        &mut command_spans,
    )?;
    Ok(GatewayCommandExecution {
        value: response.value,
        command_spans,
        transport_stages,
    })
}

fn thread_sleep_retry(timing: GatewayClientTiming, completed_attempts: u32) {
    let delay_ms = retry_delay_ms(timing, completed_attempts);
    if delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }
}


fn sse_outcome_for_io(error: &std::io::Error) -> ReachabilityOutcome {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => ReachabilityOutcome::Refused,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ReachabilityOutcome::Timeout,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::AddrNotAvailable => ReachabilityOutcome::Dns,
        _ => ReachabilityOutcome::Network,
    }
}

fn sse_data(block: &str) -> Option<String> {
    let data = block
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>();
    (!data.is_empty()).then(|| data.join("\n"))
}

pub fn stream_http_events<Connected, Event, Continue>(
    connection: &GatewayConnection,
    on_connected: Connected,
    mut on_event: Event,
    should_continue: Continue,
) -> Result<(), GatewayDispatchError>
where
    Connected: FnOnce(),
    Event: FnMut(Value),
    Continue: Fn() -> bool,
{
    let endpoint = parse_gateway_http_base(&connection.base_url)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let host = endpoint.host.clone();
    let base_path = endpoint.base_path.clone();
    let addresses = endpoint.socket_addrs().map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: format!("gateway events unreachable (dns): {error}"),
        }
    })?;
    if addresses.is_empty() {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: "gateway events unreachable (dns)".into(),
        });
    }
    let connect_timeout = Duration::from_millis(SSE_CONNECT_TIMEOUT_MS);
    let mut stream = endpoint.connect_any(&addresses, connect_timeout).map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: sse_outcome_for_io(&error),
            message: format!("gateway events connect failed: {error}"),
        }
    })?;
    stream
        .set_write_timeout(Some(connect_timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(SSE_STALL_TIMEOUT_MS)))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let path = format!("{base_path}/events");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\n{GATEWAY_SLIM_AVATARS_HEADER}: 1\r\nConnection: keep-alive\r\n"
    )
    .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    for (name, value) in &connection.headers {
        write!(stream, "{name}: {value}\r\n")
            .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    }
    write!(stream, "\r\n")
        .and_then(|_| stream.flush())
        .map_err(|error| GatewayDispatchError::Unreachable {
            outcome: sse_outcome_for_io(&error),
            message: format!("gateway events request failed: {error}"),
        })?;

    let mut buffered = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).map_err(|error| GatewayDispatchError::Unreachable {
            outcome: sse_outcome_for_io(&error),
            message: format!("gateway events handshake failed: {error}"),
        })?;
        if count == 0 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway events ended during handshake".into(),
            });
        }
        buffered.extend_from_slice(&chunk[..count]);
        if let Some(index) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if buffered.len() > 64 * 1024 {
            return Err(GatewayDispatchError::Transport(
                "gateway events response headers are too large".into(),
            ));
        }
    };
    let headers = std::str::from_utf8(&buffered[..header_end]).map_err(|_| {
        GatewayDispatchError::Transport("gateway events headers are not UTF-8".into())
    })?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| GatewayDispatchError::Transport(
            "gateway events response has no HTTP status".into(),
        ))?;
    if !(200..300).contains(&status) {
        if matches!(status, 401 | 403) {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::AccessDenied,
                message: format!("gateway events failed with HTTP {status}"),
            });
        }
        if status >= 500 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Http(status),
                message: format!("gateway events failed with HTTP {status}"),
            });
        }
        return Err(GatewayDispatchError::Command(
            super::gateway_errors::SandGatewayCommandError::new(format!(
                "gateway events failed with HTTP {status}"
            )),
        ));
    }

    on_connected();
    let mut decoder = SseBlockDecoder::default();
    let initial = &buffered[header_end..];
    if !initial.is_empty() {
        for block in decoder.push_bytes(initial) {
            if let Some(data) = sse_data(&block) {
                let value = serde_json::from_str::<Value>(&data).map_err(|error| {
                    GatewayDispatchError::Transport(format!(
                        "gateway events contained invalid JSON: {error}"
                    ))
                })?;
                on_event(value);
            }
        }
    }
    while should_continue() {
        let count = stream.read(&mut chunk).map_err(|error| GatewayDispatchError::Unreachable {
            outcome: sse_outcome_for_io(&error),
            message: format!("gateway events stream failed: {error}"),
        })?;
        if count == 0 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Network,
                message: "gateway events stream ended".into(),
            });
        }
        for block in decoder.push_bytes(&chunk[..count]) {
            if let Some(data) = sse_data(&block) {
                let value = serde_json::from_str::<Value>(&data).map_err(|error| {
                    GatewayDispatchError::Transport(format!(
                        "gateway events contained invalid JSON: {error}"
                    ))
                })?;
                on_event(value);
            }
        }
    }
    Ok(())
}
