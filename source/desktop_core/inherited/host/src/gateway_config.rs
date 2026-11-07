use std::collections::BTreeMap;
use std::fs;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayTlsConfig {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayServerConfig {
    pub host: String,
    pub port: Option<u16>,
    pub auth_token: Option<String>,
    pub tls: Option<GatewayTlsConfig>,
}

#[derive(Debug, Error)]
pub enum GatewayConfigError {
    #[error("Gateway TLS needs both SAND_GATEWAY_TLS_CERT and SAND_GATEWAY_TLS_KEY.")]
    MissingTlsPair,
    #[error("Failed to read gateway TLS cert/key ({cert_path}, {key_path}): {detail}")]
    TlsRead {
        cert_path: String,
        key_path: String,
        detail: String,
    },
}

pub fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1" | "[::1]"
    )
}

fn is_truthy_env(value: Option<&String>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

fn read_port(value: Option<&String>) -> Option<u16> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    value.parse::<u16>().ok().filter(|port| *port > 0)
}

fn resolve_tls(
    env: &BTreeMap<String, String>,
) -> Result<Option<GatewayTlsConfig>, GatewayConfigError> {
    let cert_path = env
        .get("SAND_GATEWAY_TLS_CERT")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let key_path = env
        .get("SAND_GATEWAY_TLS_KEY")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    match (cert_path, key_path) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => Err(GatewayConfigError::MissingTlsPair),
        (Some(cert_path), Some(key_path)) => {
            let cert = fs::read(cert_path).map_err(|error| GatewayConfigError::TlsRead {
                cert_path: cert_path.to_string(),
                key_path: key_path.to_string(),
                detail: error.to_string(),
            })?;
            let key = fs::read(key_path).map_err(|error| GatewayConfigError::TlsRead {
                cert_path: cert_path.to_string(),
                key_path: key_path.to_string(),
                detail: error.to_string(),
            })?;
            Ok(Some(GatewayTlsConfig { cert, key }))
        }
    }
}

fn default_gateway_token() -> String {
    let first = Uuid::new_v4().into_bytes();
    let second = Uuid::new_v4().into_bytes();
    let mut bytes = [0_u8; 32];
    bytes[..16].copy_from_slice(&first);
    bytes[16..].copy_from_slice(&second);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn resolve_gateway_server_config_with<F>(
    env: &BTreeMap<String, String>,
    generate_token: F,
) -> Result<GatewayServerConfig, GatewayConfigError>
where
    F: FnOnce() -> String,
{
    let host = env
        .get("SAND_GATEWAY_BIND_HOST")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1")
        .to_string();
    let port = read_port(env.get("SAND_HOST_PORT"));
    let tls = resolve_tls(env)?;
    let pinned_token = env
        .get("SAND_GATEWAY_TOKEN")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let require_auth = !is_loopback_host(&host)
        || is_truthy_env(env.get("SAND_GATEWAY_REQUIRE_AUTH"))
        || pinned_token.is_some();
    let auth_token = if require_auth {
        Some(pinned_token.unwrap_or_else(generate_token))
    } else {
        None
    };
    Ok(GatewayServerConfig {
        host,
        port,
        auth_token,
        tls,
    })
}

pub fn resolve_gateway_server_config() -> Result<GatewayServerConfig, GatewayConfigError> {
    let env = std::env::vars().collect::<BTreeMap<_, _>>();
    resolve_gateway_server_config_with(&env, default_gateway_token)
}

pub fn gateway_scheme(config: &GatewayServerConfig) -> &'static str {
    if config.tls.is_some() { "https" } else { "http" }
}
