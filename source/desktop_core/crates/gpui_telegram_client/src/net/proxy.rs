use crate::net::http_client::HttpClientError;
use crate::session_persistence::ProxyEndpointRecord;
use base64::{engine::general_purpose::STANDARD as BASE64_STD, Engine as _};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ClientProxyType {
    #[default]
    Direct,
    Http,
    Socks5,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientProxyConfig {
    pub proxy_type: ClientProxyType,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub fallback_to_direct: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyHealthReport {
    pub reachable: bool,
    pub latency_ms: u32,
    pub detail: String,
}

impl Default for ClientProxyConfig {
    fn default() -> Self {
        Self::direct()
    }
}

impl ClientProxyConfig {
    pub fn direct() -> Self {
        Self {
            proxy_type: ClientProxyType::Direct,
            host: String::new(),
            port: 0,
            username: None,
            password: None,
            fallback_to_direct: true,
        }
    }

    pub fn http(host: impl Into<String>, port: u16) -> Self {
        Self {
            proxy_type: ClientProxyType::Http,
            host: host.into().trim().to_string(),
            port,
            username: None,
            password: None,
            fallback_to_direct: false,
        }
    }

    pub fn socks5(host: impl Into<String>, port: u16) -> Self {
        Self {
            proxy_type: ClientProxyType::Socks5,
            host: host.into().trim().to_string(),
            port,
            username: None,
            password: None,
            fallback_to_direct: false,
        }
    }

    pub fn with_auth(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        let u = username.into();
        let p = password.into();
        self.username = if u.is_empty() { None } else { Some(u) };
        self.password = if p.is_empty() { None } else { Some(p) };
        self
    }

    pub fn is_direct(&self) -> bool {
        self.proxy_type == ClientProxyType::Direct
    }

    pub fn from_url(raw: &str) -> Result<Self, HttpClientError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("direct") || trimmed.starts_with("direct://") {
            return Ok(Self::direct());
        }
        let parsed = reqwest::Url::parse(trimmed)
            .map_err(|e| HttpClientError::Proxy(format!("Invalid proxy URL '{trimmed}': {e}")))?;
        let proxy_type = match parsed.scheme().to_ascii_lowercase().as_str() {
            "http" | "https" => ClientProxyType::Http,
            "socks5" | "socks5h" => ClientProxyType::Socks5,
            "direct" => return Ok(Self::direct()),
            other => {
                return Err(HttpClientError::Proxy(format!(
                    "Unsupported proxy scheme '{other}'"
                )));
            }
        };
        let host = parsed
            .host_str()
            .map(|h| h.trim_matches(|c| c == '[' || c == ']').to_string())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| HttpClientError::Proxy("Proxy URL missing host".into()))?;
        let port = parsed
            .port_or_known_default()
            .filter(|&p| p > 0)
            .ok_or_else(|| HttpClientError::Proxy("Proxy URL missing valid port".into()))?;
        let username = (!parsed.username().is_empty()).then(|| parsed.username().to_string());
        let password = parsed.password().map(|p| p.to_string());
        let cfg = Self {
            proxy_type,
            host,
            port,
            username,
            password,
            fallback_to_direct: false,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_record(rec: &ProxyEndpointRecord) -> Option<Self> {
        let kind_lower = rec.kind.trim().to_ascii_lowercase();
        let proxy_type = match kind_lower.as_str() {
            "direct" | "none" => return Some(Self::direct()),
            "http" | "https" => ClientProxyType::Http,
            "socks5" | "socks5h" => ClientProxyType::Socks5,
            _ => return None,
        };
        let username = rec
            .username
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                let s = rec.secret_or_user.trim();
                (!s.is_empty()).then(|| s.to_string())
            });
        let password = rec.password.clone().filter(|s| !s.is_empty());
        let cfg = Self {
            proxy_type,
            host: rec.host.trim().to_string(),
            port: rec.port,
            username,
            password,
            fallback_to_direct: false,
        };
        cfg.validate().ok().map(|_| cfg)
    }

    pub fn validate(&self) -> Result<(), HttpClientError> {
        if self.proxy_type == ClientProxyType::Direct {
            return Ok(());
        }
        if self.host.trim().is_empty() {
            return Err(HttpClientError::Proxy("Proxy host cannot be empty".into()));
        }
        if self
            .host
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
        {
            return Err(HttpClientError::Proxy(
                "Proxy host contains invalid control or whitespace characters".into(),
            ));
        }
        if self.port == 0 {
            return Err(HttpClientError::Proxy("Proxy port must be > 0".into()));
        }
        if self.password.as_deref().is_some_and(|p| !p.is_empty())
            && self.username.as_deref().unwrap_or("").is_empty()
        {
            return Err(HttpClientError::Proxy(
                "Proxy password specified without username".into(),
            ));
        }
        if let Some(u) = &self.username {
            if u.len() > 255 {
                return Err(HttpClientError::Proxy("Proxy username exceeds 255 bytes".into()));
            }
            if u.chars().any(|c| c.is_ascii_control() || c == ':') {
                return Err(HttpClientError::Proxy(
                    "Proxy username contains invalid control characters or ':'".into(),
                ));
            }
        }
        if let Some(p) = &self.password {
            if p.len() > 255 {
                return Err(HttpClientError::Proxy("Proxy password exceeds 255 bytes".into()));
            }
            if p.chars().any(|c| c.is_ascii_control()) {
                return Err(HttpClientError::Proxy(
                    "Proxy password contains invalid control characters".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn display_endpoint(&self) -> String {
        if self.is_direct() {
            "Direct".to_string()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn to_reqwest_proxy(&self) -> Result<Option<reqwest::Proxy>, HttpClientError> {
        self.validate()?;
        let scheme = match self.proxy_type {
            ClientProxyType::Direct => return Ok(None),
            ClientProxyType::Http => "http",
            ClientProxyType::Socks5 => "socks5h",
        };
        let url = format!("{}://{}:{}", scheme, format_host_for_url(&self.host), self.port);
        let mut proxy = reqwest::Proxy::all(&url)
            .map_err(|e| HttpClientError::Proxy(format!("Failed to build reqwest proxy: {e}")))?;
        if let Some(ref user) = self.username {
            let pass = self.password.as_deref().unwrap_or("");
            proxy = proxy.basic_auth(user, pass);
        }
        Ok(Some(proxy))
    }

    pub async fn probe_health(
        &self,
        target_host: &str,
        target_port: u16,
        timeout: Duration,
    ) -> Result<ProxyHealthReport, HttpClientError> {
        self.validate()?;
        let started = Instant::now();
        let fut = open_tunneled_tcp(target_host, target_port, self);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(_stream)) => {
                let latency_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
                Ok(ProxyHealthReport {
                    reachable: true,
                    latency_ms: latency_ms.max(1),
                    detail: format!("Connected via {}", self.display_endpoint()),
                })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(HttpClientError::Proxy(format!(
                "Proxy probe timed out after {}ms ({})",
                timeout.as_millis(),
                self.display_endpoint()
            ))),
        }
    }
}

fn format_host_for_url(host: &str) -> String {
    let trimmed = host.trim();
    if trimmed.contains(':') && !trimmed.starts_with('[') {
        format!("[{trimmed}]")
    } else {
        trimmed.to_string()
    }
}

pub(crate) async fn open_tunneled_tcp(
    target_host: &str,
    target_port: u16,
    proxy: &ClientProxyConfig,
) -> Result<TcpStream, HttpClientError> {
    proxy.validate()?;
    if target_host.trim().is_empty()
        || target_host
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
        || target_port == 0
    {
        return Err(HttpClientError::Proxy(
            "Invalid proxy tunnel target host or port".into(),
        ));
    }
    match proxy.proxy_type {
        ClientProxyType::Direct => TcpStream::connect((target_host, target_port))
            .await
            .map_err(|e| HttpClientError::Proxy(format!("Direct TCP connect failed: {e}"))),
        ClientProxyType::Http => open_http_connect_tunnel(target_host, target_port, proxy).await,
        ClientProxyType::Socks5 => open_socks5_tunnel(target_host, target_port, proxy).await,
    }
}

async fn open_http_connect_tunnel(
    target_host: &str,
    target_port: u16,
    proxy: &ClientProxyConfig,
) -> Result<TcpStream, HttpClientError> {
    proxy.validate()?;
    if target_host.trim().is_empty()
        || target_host
            .chars()
            .any(|c| c.is_ascii_control() || c.is_whitespace())
        || target_port == 0
    {
        return Err(HttpClientError::Proxy(
            "Invalid HTTP CONNECT target host or port".into(),
        ));
    }
    let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|e| {
            HttpClientError::Proxy(format!(
                "Failed to connect to HTTP proxy {}:{}: {e}",
                proxy.host, proxy.port
            ))
        })?;

    let authority = format!("{}:{}", format_host_for_url(target_host), target_port);
    let mut req = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\n"
    );
    if let Some(ref user) = proxy.username {
        let creds = format!("{user}:{}", proxy.password.as_deref().unwrap_or(""));
        let encoded = BASE64_STD.encode(creds.as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| HttpClientError::Proxy(format!("HTTP CONNECT write failed: {e}")))?;

    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if buf.len() >= 16 * 1024 {
            return Err(HttpClientError::Proxy(
                "HTTP proxy CONNECT response header exceeded 16 KiB".into(),
            ));
        }
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| HttpClientError::Proxy(format!("HTTP CONNECT read failed: {e}")))?;
        if n == 0 {
            return Err(HttpClientError::Proxy(
                "HTTP proxy closed connection during CONNECT handshake".into(),
            ));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let header_str = String::from_utf8_lossy(&buf);
    let status_line = header_str.lines().next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if status_code == 407 {
        return Err(HttpClientError::Proxy(
            "HTTP proxy authentication required (407 Proxy Authentication Required)".into(),
        ));
    }
    if !(200..300).contains(&status_code) {
        return Err(HttpClientError::Proxy(format!(
            "HTTP proxy CONNECT rejected with status '{status_line}'"
        )));
    }
    Ok(stream)
}

async fn open_socks5_tunnel(
    target_host: &str,
    target_port: u16,
    proxy: &ClientProxyConfig,
) -> Result<TcpStream, HttpClientError> {
    let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|e| {
            HttpClientError::Proxy(format!(
                "Failed to connect to SOCKS5 proxy {}:{}: {e}",
                proxy.host, proxy.port
            ))
        })?;

    let has_auth = proxy.username.is_some();
    let greeting: Vec<u8> = if has_auth {
        vec![0x05, 0x02, 0x00, 0x02]
    } else {
        vec![0x05, 0x01, 0x00]
    };
    stream
        .write_all(&greeting)
        .await
        .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 greeting write failed: {e}")))?;

    let mut method_resp = [0u8; 2];
    stream
        .read_exact(&mut method_resp)
        .await
        .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 greeting read failed: {e}")))?;
    if method_resp[0] != 0x05 {
        return Err(HttpClientError::Proxy(format!(
            "Invalid SOCKS5 version byte: 0x{:02x}",
            method_resp[0]
        )));
    }
    match method_resp[1] {
        0x00 => {}
        0x02 => {
            let user = proxy.username.as_deref().unwrap_or("");
            let pass = proxy.password.as_deref().unwrap_or("");
            let ubytes = user.as_bytes();
            let pbytes = pass.as_bytes();
            if ubytes.len() > 255 || pbytes.len() > 255 {
                return Err(HttpClientError::Proxy(
                    "SOCKS5 username/password exceeds 255 bytes".into(),
                ));
            }
            let mut auth_pkt = Vec::with_capacity(3 + ubytes.len() + pbytes.len());
            auth_pkt.push(0x01);
            auth_pkt.push(ubytes.len() as u8);
            auth_pkt.extend_from_slice(ubytes);
            auth_pkt.push(pbytes.len() as u8);
            auth_pkt.extend_from_slice(pbytes);
            stream
                .write_all(&auth_pkt)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 auth write failed: {e}")))?;

            let mut auth_resp = [0u8; 2];
            stream
                .read_exact(&mut auth_resp)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 auth read failed: {e}")))?;
            if auth_resp[1] != 0x00 {
                return Err(HttpClientError::Proxy(
                    "SOCKS5 username/password authentication rejected".into(),
                ));
            }
        }
        0xFF => {
            return Err(HttpClientError::Proxy(
                "SOCKS5 proxy rejected all offered authentication methods".into(),
            ));
        }
        other => {
            return Err(HttpClientError::Proxy(format!(
                "SOCKS5 proxy selected unsupported auth method 0x{other:02x}"
            )));
        }
    }

    // Build SOCKS5 CONNECT request
    let mut req = Vec::with_capacity(64);
    req.push(0x05); // VER
    req.push(0x01); // CMD = CONNECT
    req.push(0x00); // RSV
    if let Ok(ip) = target_host.parse::<IpAddr>() {
        match ip {
            IpAddr::V4(v4) => {
                req.push(0x01);
                req.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                req.push(0x04);
                req.extend_from_slice(&v6.octets());
            }
        }
    } else {
        let host_bytes = target_host.as_bytes();
        if host_bytes.is_empty() || host_bytes.len() > 255 {
            return Err(HttpClientError::Proxy(
                "Target hostname length invalid for SOCKS5".into(),
            ));
        }
        req.push(0x03);
        req.push(host_bytes.len() as u8);
        req.extend_from_slice(host_bytes);
    }
    req.extend_from_slice(&target_port.to_be_bytes());

    stream
        .write_all(&req)
        .await
        .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 CONNECT write failed: {e}")))?;

    let mut hdr = [0u8; 4];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 CONNECT read failed: {e}")))?;
    if hdr[0] != 0x05 {
        return Err(HttpClientError::Proxy(format!(
            "Invalid SOCKS5 CONNECT response version: 0x{:02x}",
            hdr[0]
        )));
    }
    if hdr[1] != 0x00 {
        return Err(HttpClientError::Proxy(format!(
            "SOCKS5 CONNECT failed with reply code 0x{:02x}",
            hdr[1]
        )));
    }
    match hdr[3] {
        0x01 => {
            let mut skip = [0u8; 4 + 2];
            stream
                .read_exact(&mut skip)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 bind read failed: {e}")))?;
            let _ = Ipv4Addr::new(skip[0], skip[1], skip[2], skip[3]);
        }
        0x04 => {
            let mut skip = [0u8; 16 + 2];
            stream
                .read_exact(&mut skip)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 bind read failed: {e}")))?;
            let mut addr = [0u8; 16];
            addr.copy_from_slice(&skip[..16]);
            let _ = Ipv6Addr::from(addr);
        }
        0x03 => {
            let mut len_buf = [0u8; 1];
            stream
                .read_exact(&mut len_buf)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 domain len failed: {e}")))?;
            let mut skip = vec![0u8; len_buf[0] as usize + 2];
            stream
                .read_exact(&mut skip)
                .await
                .map_err(|e| HttpClientError::Proxy(format!("SOCKS5 domain read failed: {e}")))?;
        }
        atyp => {
            return Err(HttpClientError::Proxy(format!(
                "Unknown SOCKS5 ATYP 0x{atyp:02x}"
            )));
        }
    }

    Ok(stream)
}
