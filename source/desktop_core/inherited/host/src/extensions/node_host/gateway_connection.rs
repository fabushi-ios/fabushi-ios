//! OpenClaw Node Host Gateway Connection, Origin Scopes, and Device Token Reuse
//!
//! Mirrors:
//! - `src/node-host/gateway-auth.ts`
//! - `src/node-host/gateway-candidate-connection.ts`
//! - `src/node-host/client.ts`
//! - `src/node-host/connection.ts`
//! - `src/node-host/node-event-params.ts`
//! - `src/node-host/local-id.ts`
//! - `src/node-host/host-stats.ts`
//! - `src/node-host/startup-state-readiness.ts`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeHostGatewayConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_path: Option<String>,
}

impl NodeHostGatewayConfig {
    pub fn new(host: impl Into<String>, port: u16, tls: bool) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
            tls_fingerprint: None,
            context_path: None,
        }
    }

    pub fn with_tls_fingerprint(mut self, fingerprint: impl Into<String>) -> Self {
        self.tls_fingerprint = Some(fingerprint.into());
        self
    }

    pub fn with_context_path(mut self, context_path: impl Into<String>) -> Self {
        self.context_path = Some(context_path.into());
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("node host gateway host must not be empty".into());
        }
        if host.chars().any(|character| character.is_control())
            || host.chars().any(|character| matches!(character, '/' | '?' | '#' | '@'))
        {
            return Err("node host gateway host contains unsafe URL characters".into());
        }
        if host.contains(':') {
            let bracketed = host.starts_with('[') && host.ends_with(']');
            let ipv6 = host.parse::<std::net::Ipv6Addr>().is_ok()
                || (bracketed && host[1..host.len() - 1].parse::<std::net::Ipv6Addr>().is_ok());
            if !ipv6 {
                return Err("node host gateway host has an invalid colon form".into());
            }
        }
        if !self.tls {
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .map(|address| address.is_loopback())
                    .unwrap_or(false);
            if !loopback {
                return Err("plaintext node host gateway endpoints must be loopback".into());
            }
        }
        if let Some(path) = self.context_path.as_deref() {
            let path = path.trim();
            if path.len() > 2048
                || path.chars().any(|character| character.is_control())
                || path.contains('?')
                || path.contains('#')
                || path.contains('\\')
            {
                return Err("node host gateway context path is unsafe or too long".into());
            }
        }
        if let Some(fingerprint) = self.tls_fingerprint.as_deref() {
            let fingerprint = fingerprint.trim();
            if !self.tls {
                return Err("node host gateway TLS fingerprint requires TLS".into());
            }
            if fingerprint.len() != 64
                || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("node host gateway TLS fingerprint is invalid".into());
            }
        }
        Ok(())
    }

    pub fn to_url(&self) -> Result<String, String> {
        self.validate()?;
        let scheme = if self.tls { "https" } else { "http" };
        Ok(format!("{}://{}:{}", scheme, self.host, self.port))
    }
}

pub fn gateway_origin_scope(url_str: &str) -> String {
    let trimmed = url_str.trim();
    if trimmed.is_empty() {
        return "default".into();
    }
    let Ok(parsed) = url::Url::parse(trimmed) else {
        return trimmed.to_string();
    };

    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    let default_port = match scheme.as_str() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    let port = parsed
        .port()
        .filter(|port| Some(*port) != default_port)
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = parsed.path();
    let path = if path.is_empty() || path == "/" {
        String::new()
    } else {
        path.trim_end_matches('/').to_string()
    };
    format!("{scheme}://{host}{port}{path}")
}

pub fn can_reuse_node_host_device_token(
    saved_url: Option<&str>,
    candidate_urls: &[&str],
    device_token: Option<&str>,
) -> bool {
    let token = match device_token {
        Some(t) if !t.trim().is_empty() => t,
        _ => return false,
    };

    let saved_scope = match saved_url {
        Some(u) if !u.trim().is_empty() => gateway_origin_scope(u),
        _ => return false,
    };

    if candidate_urls.is_empty() {
        return false;
    }

    let all_match = candidate_urls
        .iter()
        .all(|cand| gateway_origin_scope(cand) == saved_scope);

    all_match && !token.is_empty()
}

/// Resolve an auxiliary stream attach path without leaving the enrolled
/// Gateway origin or mount path. WebSocket ownership and credentials remain
/// with the eventual stream transport owner.
pub fn attach_gateway_stream_url(
    gateway_url: &str,
    attach_path: &str,
    expected_attach_path: &str,
) -> Result<String, String> {
    let gateway = url::Url::parse(gateway_url.trim())
        .map_err(|error| format!("invalid node stream gateway URL: {error}"))?;
    let attached = gateway
        .join(attach_path)
        .map_err(|error| format!("invalid node stream attach path: {error}"))?;
    if !matches!(attached.scheme(), "ws" | "wss") {
        return Err("node stream gateway URL must use WebSocket transport".into());
    }
    if attached.origin() != gateway.origin() || attached.path() != expected_attach_path {
        return Err("node stream attach path must stay on the connected gateway".into());
    }
    let mount = gateway.path().trim_end_matches('/');
    let path = if mount.is_empty() {
        attached.path().to_string()
    } else {
        format!("{mount}{}", attached.path())
    };
    let mut result = attached;
    result.set_path(&path);
    Ok(result.to_string())
}

#[cfg(test)]
mod stream_tests {
    use super::attach_gateway_stream_url;

    #[test]
    fn stream_attach_preserves_gateway_mount_and_rejects_origin_escape() {
        assert_eq!(
            attach_gateway_stream_url(
                "wss://gateway.example/ws/",
                "/node-stream",
                "/node-stream",
            )
            .expect("attach URL"),
            "wss://gateway.example/ws/node-stream"
        );
        assert!(attach_gateway_stream_url(
            "wss://gateway.example/ws",
            "https://other.example/node-stream",
            "/node-stream",
        )
        .is_err());
    }

    #[test]
    fn stream_attach_rejects_non_websocket_gateway() {
        assert!(attach_gateway_stream_url(
            "https://gateway.example/ws",
            "/node-stream",
            "/node-stream",
        )
        .is_err());
    }
}
