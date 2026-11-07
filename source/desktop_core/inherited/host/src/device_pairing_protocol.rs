//! OpenClaw Pairing Domain Helpers (`src/pairing/`)
//!
//! Mirrors:
//! - `src/pairing/pairing-join-code.ts`
//! - `src/pairing/pairing-request-path.ts`
//! - `src/pairing/pairing-key.ts`
//! - `src/pairing/pairing-reply.ts`
//! - `src/pairing/pairing-url.ts`

pub const DEVICE_PAIRING_JOIN_CODE_BYTES: usize = 16;

/// Validates whether a string is a 22-character base64url device pairing join code.
pub fn is_device_pairing_join_code(value: &str) -> bool {
    if value.len() != 22 {
        return false;
    }
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parses the pairing join request path from a URL pathname.
///
/// Matches OpenClaw `parseDevicePairingJoinRequestPath`:
/// - Terminal `/j` returns `Some("")`
/// - Last `/j/<shortcode>` returns `Some("<shortcode>")`
/// - Otherwise returns `None`
pub fn parse_device_pairing_join_request_path(pathname: &str) -> Option<&str> {
    if pathname.ends_with("/j") {
        return Some("");
    }
    if let Some(pos) = pathname.rfind("/j/") {
        return Some(&pathname[pos + 3..]);
    }
    None
}

/// Sanitizes pairing key tokens (channel or account ID).
fn normalize_pairing_key(value: &str, kind: &str) -> Result<String, String> {
    let trimmed = value.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err(format!("invalid pairing {kind}: expected non-empty string"));
    }
    let sanitized: String = trimmed
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let safe = sanitized.replace("..", "_");
    if safe.is_empty() || safe == "_" {
        return Err(format!("invalid pairing {kind}: sanitized key is empty"));
    }
    Ok(safe)
}

/// Sanitizes channel key for pairing store.
pub fn safe_channel_key(channel: &str) -> Result<String, String> {
    normalize_pairing_key(channel, "channel")
}

/// Sanitizes account ID key for pairing store.
pub fn safe_account_key(account_id: &str) -> Result<String, String> {
    normalize_pairing_key(account_id, "account id")
}

/// Resolves pairing id label shown in approval prompts.
pub fn resolve_pairing_id_label(custom_label: Option<&str>) -> &str {
    custom_label.unwrap_or("userId")
}

/// Formats the user-facing pairing challenge reply text.
pub fn build_pairing_reply(channel: &str, id_line: &str, code: &str) -> String {
    format!(
        "OpenClaw: access not configured.\n\n{}\nPairing code:\n```\n{}\n```\n\nAsk the bot owner to approve with:\n```\nopenclaw pairing approve {} {}\n```",
        id_line, code, channel, code
    )
}

/// Checks whether a hostname belongs to a private LAN or local domain.
pub fn is_private_lan_host(host: &str) -> bool {
    let host = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    let host = host.split('%').next().unwrap_or(&host);
    if host.ends_with(".local") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(v4) => {
                let octets = v4.octets();
                octets[0] == 10
                    || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                    || (octets[0] == 192 && octets[1] == 168)
                    || (octets[0] == 169 && octets[1] == 254) // Link-local
            }
            std::net::IpAddr::V6(v6) => {
                let segs = v6.segments();
                (segs[0] & 0xffc0) == 0xfe80 // Link-local (fe80::/10)
                    || (segs[0] & 0xfe00) == 0xfc00 // ULA (fc00::/7, covers fc00 and fd00)
            }
        }
    } else {
        false
    }
}

/// Checks whether cleartext ws:// is allowed for this mobile pairing host.
pub fn is_mobile_pairing_cleartext_allowed_host(host: &str) -> bool {
    let host = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    let host = host.split('%').next().unwrap_or(&host);
    host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "10.0.2.2" // Android emulator loopback alias
        || is_private_lan_host(host)
}

/// Validates mobile pairing URL security.
/// Rejects cleartext `ws://` on public internet; allows `wss://` and LAN/localhost `ws://`.
pub fn validate_mobile_pairing_url(url: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(url).map_err(|_| "Resolved mobile pairing URL is invalid.".to_string())?;
    let scheme = parsed.scheme().to_lowercase();
    if scheme == "wss" || scheme == "https" {
        return Ok(());
    }
    if scheme == "ws" || scheme == "http" {
        if let Some(host) = parsed.host_str() {
            if is_mobile_pairing_cleartext_allowed_host(host) {
                return Ok(());
            }
        }
        return Err(
            "Tailscale and public mobile pairing require a secure gateway URL (wss://) or Tailscale Serve/Funnel. ws:// is only valid for localhost, private LAN addresses, .local hosts, or the Android emulator.".to_string()
        );
    }
    Err("Invalid mobile pairing URL protocol; expected wss:// or ws://".to_string())
}
