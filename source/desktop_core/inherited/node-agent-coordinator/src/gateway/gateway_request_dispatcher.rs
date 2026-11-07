use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::time::Duration;

use serde_json::Value;

use crate::gateway::gateway_errors::SandGatewayCommandError;
use crate::gateway::gateway_reachability::ReachabilityOutcome;
use crate::gateway::host_supervisor::GatewayConnection;
use crate::gateway::http_transport::{parse_gateway_http_base, read_http_response};
use crate::protocol::{Failure, ReplyOutcome, COORDINATOR_UNKNOWN_METHOD};

pub const GATEWAY_COMMAND_FAILED: &str = "gateway-command-failed";
pub const GATEWAY_UNREACHABLE: &str = "gateway-unreachable";
pub const GATEWAY_TRANSPORT_FAILED: &str = "gateway-transport-failed";
pub const GATEWAY_SLIM_AVATARS_HEADER: &str = "x-sand-slim-avatars";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayDispatchError {
    #[error("{0}")]
    Command(#[from] SandGatewayCommandError),
    #[error("{message}")]
    Unreachable {
        outcome: ReachabilityOutcome,
        message: String,
    },
    #[error("{0}")]
    Transport(String),
}

pub fn failure_for(error: &GatewayDispatchError) -> Failure {
    match error {
        GatewayDispatchError::Command(error) => {
            Failure::new(GATEWAY_COMMAND_FAILED, error.message.clone())
        }
        GatewayDispatchError::Unreachable { outcome, message } => Failure::with_transport_kind(
            GATEWAY_UNREACHABLE,
            message.clone(),
            outcome.transport_kind(),
        ),
        GatewayDispatchError::Transport(message) => {
            Failure::new(GATEWAY_TRANSPORT_FAILED, message.clone())
        }
    }
}

pub type GatewayHandler =
    Box<dyn Fn(Value) -> Result<Value, GatewayDispatchError> + Send + Sync>;

#[derive(Default)]
pub struct GatewayRequestDispatcher {
    handlers: HashMap<String, GatewayHandler>,
}

impl std::fmt::Debug for GatewayRequestDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayRequestDispatcher")
            .field("method_count", &self.handlers.len())
            .finish()
    }
}

impl GatewayRequestDispatcher {
    pub fn register(&mut self, method: impl Into<String>, handler: GatewayHandler) {
        self.handlers.insert(method.into(), handler);
    }

    pub fn serves(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    pub fn dispatch_reply(&self, method: &str, args: Value) -> ReplyOutcome {
        let Some(handler) = self.handlers.get(method) else {
            return ReplyOutcome::Failed {
                failure: Failure::new(
                    COORDINATOR_UNKNOWN_METHOD,
                    format!("no coordinator method named {method}"),
                ),
            };
        };

        match handler(args) {
            Ok(value) => ReplyOutcome::Ok { value },
            Err(error) => ReplyOutcome::Failed {
                failure: failure_for(&error),
            },
        }
    }

    pub fn dispatch(&self, method: &str, args: Value) -> Result<Value, Failure> {
        match self.dispatch_reply(method, args) {
            ReplyOutcome::Ok { value } => Ok(value),
            ReplyOutcome::Failed { failure } => Err(failure),
        }
    }
}


fn classify_connect_error(error: &std::io::Error) -> ReachabilityOutcome {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => ReachabilityOutcome::Refused,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ReachabilityOutcome::Timeout,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::AddrNotAvailable => ReachabilityOutcome::Dns,
        _ => ReachabilityOutcome::Network,
    }
}

fn gateway_error_message(body: &[u8], fallback: &str) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("error").and_then(Value::as_str).map(str::to_string))
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayJsonResponse {
    pub value: Value,
    pub headers: Vec<(String, String)>,
}

impl GatewayJsonResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub fn dispatch_http_json_response(
    connection: &GatewayConnection,
    method: &str,
    args: Value,
    timeout: Duration,
    extra_headers: &BTreeMap<String, String>,
) -> Result<GatewayJsonResponse, GatewayDispatchError> {
    if method.is_empty()
        || method
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    {
        return Err(GatewayDispatchError::Transport(
            "invalid Host gateway method".into(),
        ));
    }
    let parsed = parse_gateway_http_base(&connection.base_url)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let addresses = parsed.socket_addrs().map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: format!("gateway {method} unreachable (dns): {error}"),
        }
    })?;
    if addresses.is_empty() {
        return Err(GatewayDispatchError::Unreachable {
            outcome: ReachabilityOutcome::Dns,
            message: format!("gateway {method} unreachable (dns)"),
        });
    }
    let mut stream = parsed.connect_any(&addresses, timeout).map_err(|error| {
        GatewayDispatchError::Unreachable {
            outcome: classify_connect_error(&error),
            message: format!("gateway {method} unreachable: {error}"),
        }
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;

    let body = serde_json::to_vec(&args)
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    let path = format!("{}{GATEWAY_API_PREFIX}/{method}", parsed.base_path);
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{GATEWAY_SLIM_AVATARS_HEADER}: 1\r\nConnection: close\r\n",
        parsed.host,
        body.len()
    )
    .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    for (name, value) in &connection.headers {
        write!(stream, "{name}: {value}\r\n")
            .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    }
    for (name, value) in extra_headers {
        write!(stream, "{name}: {value}\r\n")
            .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    }
    write!(stream, "\r\n")
        .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
    stream
        .write_all(&body)
        .and_then(|_| stream.flush())
        .map_err(|error| GatewayDispatchError::Unreachable {
            outcome: classify_connect_error(&error),
            message: format!("gateway {method} write failed: {error}"),
        })?;

    let response = read_http_response(&mut stream, 64 * 1024, 512 * 1024 * 1024)
        .map_err(|error| GatewayDispatchError::Unreachable {
            outcome: classify_connect_error(&error),
            message: format!("gateway {method} read failed: {error}"),
        })?;
    if !(200..300).contains(&response.status) {
        let message = gateway_error_message(
            &response.body,
            &format!("gateway {method} failed with HTTP {}", response.status),
        );
        if response.status >= 500 {
            return Err(GatewayDispatchError::Unreachable {
                outcome: ReachabilityOutcome::Http(response.status),
                message,
            });
        }
        return Err(GatewayDispatchError::Command(SandGatewayCommandError::new(message)));
    }
    let value = serde_json::from_slice(&response.body)
        .map_err(|error| GatewayDispatchError::Transport(format!(
            "Host gateway returned invalid JSON: {error}"
        )))?;
    Ok(GatewayJsonResponse {
        value,
        headers: response.headers,
    })
}

pub fn dispatch_http_json(
    connection: &GatewayConnection,
    method: &str,
    args: Value,
) -> Result<Value, GatewayDispatchError> {
    dispatch_http_json_response(
        connection,
        method,
        args,
        Duration::from_millis(15_000),
        &BTreeMap::new(),
    )
    .map(|response| response.value)
}

const GATEWAY_API_PREFIX: &str = "/api";
