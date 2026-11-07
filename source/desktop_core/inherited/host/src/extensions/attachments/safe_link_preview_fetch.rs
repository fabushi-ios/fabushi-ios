use std::collections::BTreeMap;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_LENGTH, HOST, LOCATION, USER_AGENT,
};
use reqwest::redirect::Policy;
use url::Url;

pub const MAX_URL_LENGTH: usize = 2_048;
pub const MAX_REDIRECTS: usize = 5;
pub const LINK_PREVIEW_USER_AGENT: &str = "Fabushi-LinkPreview/1.0";
pub const LINK_PREVIEW_DNS_DEADLINE_MS: u64 = 3_000;
pub const LINK_PREVIEW_REQUEST_DEADLINE_MS: u64 = 8_000;

const NON_PUBLIC_HOSTNAME_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".internal",
    ".lan",
    ".home",
    ".corp",
    ".cluster",
    ".svc",
    ".arpa",
    ".onion",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandLinkPreviewError {
    #[error("Link preview URL is too long.")]
    UrlTooLong,
    #[error("Link preview URL must be an absolute HTTPS URL.")]
    InvalidUrl,
    #[error("Link previews require HTTPS.")]
    RequiresHttps,
    #[error("Link preview URLs cannot contain credentials.")]
    Credentials,
    #[error("Link preview URLs cannot use a custom port.")]
    CustomPort,
    #[error("Link preview URL has a non-public hostname.")]
    NonPublicHostname,
    #[error("Link preview URL has a non-public IP address.")]
    NonPublicIp,
    #[error("Link preview hostname did not resolve.")]
    HostnameDidNotResolve,
    #[error("Link preview DNS lookup failed: {0}")]
    Dns(String),
    #[error("Link preview request failed: {0}")]
    Request(String),
    #[error("Link preview response exceeded its byte limit.")]
    BodyTooLarge,
    #[error("Too many redirects while fetching link preview.")]
    TooManyRedirects,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeConnectionTarget {
    pub connect_host: String,
    pub host_header: String,
    pub servername: Option<String>,
    pub socket_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreviewResponse {
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub final_url: String,
    pub redirect_urls: Vec<String>,
}

pub fn normalize_hostname(hostname: &str) -> String {
    hostname
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(hostname)
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_string()
}

pub fn has_non_public_hostname_suffix(hostname: &str) -> bool {
    NON_PUBLIC_HOSTNAME_SUFFIXES.iter().any(|suffix| {
        hostname == &suffix[1..] || hostname.ends_with(suffix)
    })
}

pub fn is_blocked_hostname(hostname: &str) -> bool {
    hostname.is_empty()
        || !hostname.contains('.')
        || hostname == "localhost"
        || has_non_public_hostname_suffix(hostname)
}

fn ipv4_in_subnet(address: Ipv4Addr, network: Ipv4Addr, prefix: u32) -> bool {
    let address = u32::from(address);
    let network = u32::from(network);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    address & mask == network & mask
}

fn ipv6_in_subnet(address: Ipv6Addr, network: Ipv6Addr, prefix: u32) -> bool {
    let address = u128::from(address);
    let network = u128::from(network);
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    address & mask == network & mask
}

pub fn is_blocked_link_preview_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => [
            (Ipv4Addr::new(0, 0, 0, 0), 8),
            (Ipv4Addr::new(10, 0, 0, 0), 8),
            (Ipv4Addr::new(100, 64, 0, 0), 10),
            (Ipv4Addr::new(127, 0, 0, 0), 8),
            (Ipv4Addr::new(169, 254, 0, 0), 16),
            (Ipv4Addr::new(172, 16, 0, 0), 12),
            (Ipv4Addr::new(192, 0, 0, 0), 24),
            (Ipv4Addr::new(192, 0, 2, 0), 24),
            (Ipv4Addr::new(192, 88, 99, 0), 24),
            (Ipv4Addr::new(192, 168, 0, 0), 16),
            (Ipv4Addr::new(198, 18, 0, 0), 15),
            (Ipv4Addr::new(198, 51, 100, 0), 24),
            (Ipv4Addr::new(203, 0, 113, 0), 24),
            (Ipv4Addr::new(224, 0, 0, 0), 4),
            (Ipv4Addr::new(240, 0, 0, 0), 4),
        ]
        .into_iter()
        .any(|(network, prefix)| ipv4_in_subnet(ip, network, prefix)),
        IpAddr::V6(ip) => [
            (Ipv6Addr::UNSPECIFIED, 127),
            ("64:ff9b::".parse::<Ipv6Addr>().expect("valid subnet"), 96),
            ("64:ff9b:1::".parse::<Ipv6Addr>().expect("valid subnet"), 48),
            ("100::".parse::<Ipv6Addr>().expect("valid subnet"), 64),
            ("2001::".parse::<Ipv6Addr>().expect("valid subnet"), 32),
            ("2001:db8::".parse::<Ipv6Addr>().expect("valid subnet"), 32),
            ("2002::".parse::<Ipv6Addr>().expect("valid subnet"), 16),
            ("fc00::".parse::<Ipv6Addr>().expect("valid subnet"), 7),
            ("fe80::".parse::<Ipv6Addr>().expect("valid subnet"), 10),
            ("fec0::".parse::<Ipv6Addr>().expect("valid subnet"), 10),
            ("ff00::".parse::<Ipv6Addr>().expect("valid subnet"), 8),
        ]
        .into_iter()
        .any(|(network, prefix)| ipv6_in_subnet(ip, network, prefix)),
    }
}

pub fn parse_safe_link_preview_url(raw_url: &str) -> Result<Url, SandLinkPreviewError> {
    if raw_url.len() > MAX_URL_LENGTH {
        return Err(SandLinkPreviewError::UrlTooLong);
    }
    let parsed = Url::parse(raw_url).map_err(|_| SandLinkPreviewError::InvalidUrl)?;
    if parsed.scheme() != "https" {
        return Err(SandLinkPreviewError::RequiresHttps);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SandLinkPreviewError::Credentials);
    }
    if parsed.port().is_some_and(|port| port != 443) {
        return Err(SandLinkPreviewError::CustomPort);
    }

    let hostname = normalize_hostname(parsed.host_str().unwrap_or_default());
    match hostname.parse::<IpAddr>() {
        Ok(address) if is_blocked_link_preview_ip(address) => {
            return Err(SandLinkPreviewError::NonPublicIp);
        }
        Ok(_) => {}
        Err(_) if is_blocked_hostname(&hostname) => {
            return Err(SandLinkPreviewError::NonPublicHostname);
        }
        Err(_) => {}
    }
    Ok(parsed)
}

fn host_header_for(url: &Url) -> Result<String, SandLinkPreviewError> {
    let hostname = normalize_hostname(url.host_str().ok_or(SandLinkPreviewError::InvalidUrl)?);
    let formatted = match hostname.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{hostname}]"),
        _ => hostname,
    };
    Ok(match url.port() {
        Some(port) => format!("{formatted}:{port}"),
        None => formatted,
    })
}

fn resolve_dns_with_deadline(hostname: &str) -> Result<Vec<IpAddr>, SandLinkPreviewError> {
    let hostname = hostname.to_string();
    let (tx, rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("link-preview-dns-lookup".into())
        .spawn(move || {
            let result = (hostname.as_str(), 443)
                .to_socket_addrs()
                .map(|iter| iter.map(|address| address.ip()).collect::<Vec<_>>())
                .map_err(|error| error.to_string());
            let _ = tx.send(result);
        })
        .map_err(|error| SandLinkPreviewError::Dns(error.to_string()))?;

    match rx.recv_timeout(Duration::from_millis(LINK_PREVIEW_DNS_DEADLINE_MS)) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(error)) => Err(SandLinkPreviewError::Dns(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(SandLinkPreviewError::Dns("lookup deadline exceeded".into()))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(SandLinkPreviewError::Dns("lookup worker disconnected".into()))
        }
    }
}

pub fn get_safe_link_preview_connection_target_with<F>(
    url: &Url,
    resolver: F,
) -> Result<SafeConnectionTarget, SandLinkPreviewError>
where
    F: FnOnce(&str) -> Result<Vec<IpAddr>, SandLinkPreviewError>,
{
    let parsed = parse_safe_link_preview_url(url.as_str())?;
    let hostname = normalize_hostname(parsed.host_str().ok_or(SandLinkPreviewError::InvalidUrl)?);
    if let Ok(address) = hostname.parse::<IpAddr>() {
        return Ok(SafeConnectionTarget {
            connect_host: hostname,
            host_header: host_header_for(&parsed)?,
            servername: None,
            socket_addr: SocketAddr::new(address, 443),
        });
    }

    let addresses = resolver(&hostname)?;
    if addresses.is_empty() {
        return Err(SandLinkPreviewError::HostnameDidNotResolve);
    }
    if addresses.iter().copied().any(is_blocked_link_preview_ip) {
        return Err(SandLinkPreviewError::NonPublicIp);
    }
    let pinned = addresses
        .iter()
        .copied()
        .find(IpAddr::is_ipv4)
        .or_else(|| addresses.iter().copied().find(IpAddr::is_ipv6))
        .ok_or(SandLinkPreviewError::HostnameDidNotResolve)?;

    Ok(SafeConnectionTarget {
        connect_host: pinned.to_string(),
        host_header: host_header_for(&parsed)?,
        servername: Some(hostname),
        socket_addr: SocketAddr::new(pinned, 443),
    })
}

pub fn get_safe_link_preview_connection_target(
    url: &Url,
) -> Result<SafeConnectionTarget, SandLinkPreviewError> {
    get_safe_link_preview_connection_target_with(url, resolve_dns_with_deadline)
}

pub fn resolve_safe_link_preview_redirect(
    current_url: &Url,
    location: &str,
) -> Result<Url, SandLinkPreviewError> {
    let resolved = current_url
        .join(location)
        .map_err(|_| SandLinkPreviewError::InvalidUrl)?;
    parse_safe_link_preview_url(resolved.as_str())
}

pub fn read_limited_body(
    reader: &mut impl Read,
    max_bytes: usize,
    truncate_at_byte_limit: bool,
) -> Result<Vec<u8>, SandLinkPreviewError> {
    let read_limit = max_bytes.saturating_add(1);
    let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(read_limit as u64)
        .read_to_end(&mut body)
        .map_err(|error| SandLinkPreviewError::Request(error.to_string()))?;
    if body.len() > max_bytes {
        if !truncate_at_byte_limit {
            return Err(SandLinkPreviewError::BodyTooLarge);
        }
        body.truncate(max_bytes);
    }
    Ok(body)
}

fn response_headers(response: &Response) -> BTreeMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn request_once(
    url: &Url,
    max_bytes: usize,
    accept: &str,
    truncate_at_byte_limit: bool,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>), SandLinkPreviewError> {
    let target = get_safe_link_preview_connection_target(url)?;
    let hostname = normalize_hostname(url.host_str().ok_or(SandLinkPreviewError::InvalidUrl)?);
    let mut builder = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_millis(LINK_PREVIEW_DNS_DEADLINE_MS))
        .timeout(Duration::from_millis(LINK_PREVIEW_REQUEST_DEADLINE_MS))
        .no_proxy();

    if hostname.parse::<IpAddr>().is_err() {
        builder = builder.resolve(&hostname, target.socket_addr);
    }

    let client = builder
        .build()
        .map_err(|error| SandLinkPreviewError::Request(error.to_string()))?;
    let mut response = client
        .get(url.clone())
        .header(ACCEPT, accept)
        .header(ACCEPT_ENCODING, "identity")
        .header(CONNECTION, "close")
        .header(HOST, target.host_header)
        .header(USER_AGENT, LINK_PREVIEW_USER_AGENT)
        .send()
        .map_err(|error| SandLinkPreviewError::Request(error.to_string()))?;

    let status = response.status().as_u16();
    let headers = response_headers(&response);
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok((status, headers, Vec::new()));
    }

    if !truncate_at_byte_limit {
        let declared = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        if declared.is_some_and(|length| length > max_bytes) {
            return Err(SandLinkPreviewError::BodyTooLarge);
        }
    }

    let body = read_limited_body(&mut response, max_bytes, truncate_at_byte_limit)?;
    Ok((status, headers, body))
}

pub fn fetch_safe_link_preview_resource(
    raw_url: &str,
    max_bytes: usize,
    accept: &str,
    truncate_at_byte_limit: bool,
) -> Result<LinkPreviewResponse, SandLinkPreviewError> {
    let mut current_url = parse_safe_link_preview_url(raw_url)?;
    let mut redirect_urls = Vec::new();

    for count in 0.. {
        if count > MAX_REDIRECTS {
            return Err(SandLinkPreviewError::TooManyRedirects);
        }
        let (status_code, headers, body) =
            request_once(&current_url, max_bytes, accept, truncate_at_byte_limit)?;
        if !matches!(status_code, 301 | 302 | 303 | 307 | 308) {
            return Ok(LinkPreviewResponse {
                status_code,
                headers,
                body,
                final_url: current_url.to_string(),
                redirect_urls,
            });
        }

        let Some(location) = headers.get(LOCATION.as_str()) else {
            return Ok(LinkPreviewResponse {
                status_code,
                headers,
                body,
                final_url: current_url.to_string(),
                redirect_urls,
            });
        };
        current_url = resolve_safe_link_preview_redirect(&current_url, location)?;
        redirect_urls.push(current_url.to_string());
    }
    unreachable!("redirect loop is bounded")
}
