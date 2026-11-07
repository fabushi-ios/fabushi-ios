use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::http_transport::{parse_gateway_http_base, read_http_response};

pub const HEALTH_TIMEOUT_MS: u64 = 1_500;
pub const HEALTH_PROBE_TTL_MS: u64 = 5_000;
pub const DISABLE_HEALTH_TTL_ENV: &str = "SAND_DISABLE_GATEWAY_HEALTH_TTL";
pub const DISABLE_STREAM_LIVENESS_ENV: &str = "SAND_DISABLE_GATEWAY_STREAM_LIVENESS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayReachability {
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayHealthDecision {
    UseCached,
    Probe,
    Reconnect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConnection {
    pub base_url: String,
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionAttempt {
    pub health_epoch: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the gateway connection resolve was abandoned before it answered")]
pub struct GatewayConnectResolveAbandonedError;

#[derive(Debug)]
pub struct GatewayHostSupervisor {
    reachability: GatewayReachability,
    health_epoch: u64,
    transport_live: bool,
    last_healthy_ms: Option<u64>,
    health_ttl_ms: u64,
    health_ttl_disabled: bool,
    stream_liveness_disabled: bool,
    connection: Option<GatewayConnection>,
    latest_attempt: Option<ConnectionAttempt>,
    next_attempt_generation: u64,
}

impl GatewayHostSupervisor {
    pub fn new(health_ttl_ms: u64) -> Self {
        Self::with_feature_flags(
            health_ttl_ms,
            env::var(DISABLE_HEALTH_TTL_ENV).ok().as_deref() == Some("1"),
            env::var(DISABLE_STREAM_LIVENESS_ENV).ok().as_deref() == Some("1"),
        )
    }

    pub fn with_feature_flags(
        health_ttl_ms: u64,
        health_ttl_disabled: bool,
        stream_liveness_disabled: bool,
    ) -> Self {
        Self {
            reachability: GatewayReachability::Unknown,
            health_epoch: 0,
            transport_live: false,
            last_healthy_ms: None,
            health_ttl_ms,
            health_ttl_disabled,
            stream_liveness_disabled,
            connection: None,
            latest_attempt: None,
            next_attempt_generation: 0,
        }
    }

    pub fn health_epoch(&self) -> u64 {
        self.health_epoch
    }

    pub fn connection(&self) -> Option<&GatewayConnection> {
        self.connection.as_ref()
    }

    pub fn latest_attempt(&self) -> Option<ConnectionAttempt> {
        self.latest_attempt
    }

    pub fn mark_transport_live(&mut self, live: bool) {
        self.transport_live = live;
        if live {
            self.reachability = GatewayReachability::Reachable;
        }
    }

    pub fn record_health(&mut self, now_ms: u64, reachable: bool) {
        self.reachability = if reachable {
            GatewayReachability::Reachable
        } else {
            GatewayReachability::Unreachable
        };
        self.last_healthy_ms = reachable.then_some(now_ms);
    }

    pub fn begin_connection_attempt(&mut self) -> ConnectionAttempt {
        if let Some(current) = self.latest_attempt {
            if current.health_epoch == self.health_epoch {
                return current;
            }
        }
        self.next_attempt_generation = self.next_attempt_generation.saturating_add(1);
        let attempt = ConnectionAttempt {
            health_epoch: self.health_epoch,
            generation: self.next_attempt_generation,
        };
        self.latest_attempt = Some(attempt);
        attempt
    }

    pub fn settle_connection_attempt(
        &mut self,
        attempt: ConnectionAttempt,
        connection: GatewayConnection,
    ) -> Result<(), GatewayConnectResolveAbandonedError> {
        if attempt.health_epoch != self.health_epoch || self.latest_attempt != Some(attempt) {
            return Err(GatewayConnectResolveAbandonedError);
        }
        self.connection = Some(connection);
        self.last_healthy_ms = None;
        self.reachability = GatewayReachability::Unknown;
        self.latest_attempt = None;
        Ok(())
    }

    pub fn fail_connection_attempt(
        &mut self,
        attempt: ConnectionAttempt,
    ) -> Result<(), GatewayConnectResolveAbandonedError> {
        if attempt.health_epoch != self.health_epoch || self.latest_attempt != Some(attempt) {
            return Err(GatewayConnectResolveAbandonedError);
        }
        self.latest_attempt = None;
        self.reachability = GatewayReachability::Unreachable;
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.health_epoch = self.health_epoch.saturating_add(1);
        self.last_healthy_ms = None;
        self.transport_live = false;
        self.reachability = GatewayReachability::Unknown;
        self.latest_attempt = None;
    }

    pub fn decision(&self, now_ms: u64) -> GatewayHealthDecision {
        if self.connection.is_some() && self.transport_live && !self.stream_liveness_disabled {
            return GatewayHealthDecision::UseCached;
        }
        if self.connection.is_some() {
            if !self.health_ttl_disabled {
                if let Some(last) = self.last_healthy_ms {
                    if now_ms.saturating_sub(last) < self.health_ttl_ms {
                        return GatewayHealthDecision::UseCached;
                    }
                }
            }
            if self.reachability == GatewayReachability::Unreachable {
                return GatewayHealthDecision::Reconnect;
            }
            return GatewayHealthDecision::Probe;
        }
        match self.reachability {
            GatewayReachability::Unreachable => GatewayHealthDecision::Reconnect,
            _ => GatewayHealthDecision::Probe,
        }
    }

    pub fn probe_cached_connection(&mut self, now_ms: u64) -> io::Result<bool> {
        let Some(connection) = self.connection.clone() else {
            return Ok(false);
        };
        // Grok's supervisor treats transport/parse failures from /health as an
        // unhealthy cached endpoint and immediately transitions to reconnect.
        // A probe failure must not escape as an unrelated Coordinator I/O error.
        let healthy = fetch_health(
            &connection,
            Duration::from_millis(HEALTH_TIMEOUT_MS),
        )
        .ok()
        .flatten()
        .is_some();
        self.record_health(now_ms, healthy);
        Ok(healthy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiscoveryInfo {
    pub port: u16,
    pub pid: u32,
    pub started_at: u64,
    pub scheme: Option<String>,
    pub host: Option<String>,
    pub token: Option<String>,
}

impl GatewayDiscoveryInfo {
    pub fn into_connection(self) -> io::Result<GatewayConnection> {
        if self.port == 0 || self.pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Host gateway discovery port/pid",
            ));
        }
        let scheme = self.scheme.unwrap_or_else(|| "http".into());
        if scheme != "http" && scheme != "https" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Host gateway discovery scheme",
            ));
        }
        let host = self
            .host
            .filter(|host| !host.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1".into());
        let host_for_url = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host
        };
        let mut headers = BTreeMap::new();
        if let Some(token) = self.token.filter(|token| !token.is_empty()) {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        Ok(GatewayConnection {
            base_url: format!("{scheme}://{host_for_url}:{}", self.port),
            headers,
        })
    }
}

pub fn read_gateway_discovery(path: &Path) -> io::Result<GatewayConnection> {
    let bytes = fs::read(path)?;
    let info = serde_json::from_slice::<GatewayDiscoveryInfo>(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    info.into_connection()
}

pub fn fetch_health(
    connection: &GatewayConnection,
    timeout: Duration,
) -> io::Result<Option<Value>> {
    let parsed = parse_gateway_http_base(&connection.base_url)?;
    let addresses = parsed.socket_addrs()?;
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "Host gateway health address did not resolve",
        ));
    }
    let mut stream = parsed.connect_any(&addresses, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let path = format!("{}/health", parsed.base_path);
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        parsed.host
    )?;
    for (name, value) in &connection.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.flush()?;

    let response = read_http_response(&mut stream, 64 * 1024, 4 * 1024 * 1024)?;
    if !(200..300).contains(&response.status) {
        return Ok(None);
    }
    let payload: Value = serde_json::from_slice(&response.body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok((payload.get("ok").and_then(Value::as_bool) == Some(true)).then_some(payload))
}
