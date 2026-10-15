use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::env;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;
use uuid::Uuid;

use super::privacy_mode::{PrivacyLookupError, PrivacyMode, TelemetryAuth};

const DEFAULT_CURSOR_BACKEND_URL: &str = "https://api2.cursor.sh";
const DASHBOARD_GET_USER_PRIVACY_MODE_PATH: &str =
    "/aiserver.v1.DashboardService/GetUserPrivacyMode";
const PRIVACY_MODE_FETCH_TIMEOUT: Duration = Duration::from_secs(3);
const SAND_CLIENT_TYPE: &str = "sand";
const SAND_CLIENT_FALLBACK_BASE_VERSION: &str = "0.1.0";

fn configured_backend_url_from(
    sand_backend_url: Option<String>,
    cursor_api_base_url: Option<String>,
) -> String {
    sand_backend_url
        .filter(|value| !value.trim().is_empty())
        .or_else(|| cursor_api_base_url.filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_CURSOR_BACKEND_URL.to_string())
}

pub fn configured_backend_url() -> String {
    configured_backend_url_from(
        env::var("SAND_BACKEND_URL").ok(),
        env::var("CURSOR_API_BASE_URL").ok(),
    )
}

pub(crate) fn sand_namespace() -> &'static str {
    match env::var("SAND_BOX_OWNER_NAMESPACE").ok().as_deref() {
        Some("dev") => "dev",
        Some("lab") => "lab",
        _ => match env::var("SAND_VARIANT").ok().as_deref() {
            Some("dev") => "dev",
            Some("lab") => "lab",
            _ => "prod",
        },
    }
}

pub(crate) fn sand_client_version() -> String {
    let base = env::var("SAND_CLIENT_APP_VERSION")
        .ok()
        .and_then(|raw| {
            let candidate = raw.trim().split('-').next()?.trim();
            let valid = candidate.split('.').count() == 3
                && candidate.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
            valid.then(|| candidate.to_string())
        })
        .unwrap_or_else(|| SAND_CLIENT_FALLBACK_BASE_VERSION.to_string());
    match sand_namespace() {
        "dev" => format!("{base}-dev"),
        "lab" => format!("{base}-lab"),
        _ => base,
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn create_cursor_checksum(machine_id: &str, now_ms: u64) -> String {
    let kilo_seconds = now_ms / 1_000_000;
    let mut bytes = [
        ((kilo_seconds >> 40) & 0xff) as u8,
        ((kilo_seconds >> 32) & 0xff) as u8,
        ((kilo_seconds >> 24) & 0xff) as u8,
        ((kilo_seconds >> 16) & 0xff) as u8,
        ((kilo_seconds >> 8) & 0xff) as u8,
        (kilo_seconds & 0xff) as u8,
    ];
    let mut last = 165_u8;
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = ((*byte ^ last).wrapping_add((index % 256) as u8)) & 0xff;
        last = *byte;
    }
    format!("{}{}", URL_SAFE_NO_PAD.encode(bytes), machine_id)
}

fn decode_varint(input: &[u8], cursor: &mut usize) -> Result<u64, PrivacyLookupError> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *cursor < input.len() && shift < 64 {
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(PrivacyLookupError::new("privacy response contained malformed varint", false))
}

fn decode_privacy_mode(input: &[u8]) -> Result<PrivacyMode, PrivacyLookupError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        match (field, wire) {
            (1, 0) => {
                let value = decode_varint(input, &mut cursor)?;
                return Ok(PrivacyMode::from_proto(value).unwrap_or(PrivacyMode::Unspecified));
            }
            (_, 0) => { let _ = decode_varint(input, &mut cursor)?; }
            (_, 1) => cursor = cursor.saturating_add(8),
            (_, 2) => {
                let len = usize::try_from(decode_varint(input, &mut cursor)?)
                    .map_err(|_| PrivacyLookupError::new("privacy response length overflow", false))?;
                cursor = cursor.saturating_add(len);
            }
            (_, 5) => cursor = cursor.saturating_add(4),
            _ => return Err(PrivacyLookupError::new("privacy response used unsupported protobuf wire type", false)),
        }
        if cursor > input.len() {
            return Err(PrivacyLookupError::new("privacy response was truncated", false));
        }
    }
    Ok(PrivacyMode::Unspecified)
}

pub fn fetch_privacy_mode(auth: TelemetryAuth) -> Result<PrivacyMode, PrivacyLookupError> {
    let base = configured_backend_url();
    let base = Url::parse(&base)
        .map_err(|error| PrivacyLookupError::new(format!("invalid Cursor backend URL: {error}"), false))?;
    let url = base.join(DASHBOARD_GET_USER_PRIVACY_MODE_PATH)
        .map_err(|error| PrivacyLookupError::new(format!("invalid Cursor privacy URL: {error}"), false))?;
    let client = Client::builder()
        .connect_timeout(PRIVACY_MODE_FETCH_TIMEOUT)
        .timeout(PRIVACY_MODE_FETCH_TIMEOUT)
        .build()
        .map_err(|error| PrivacyLookupError::new(format!("privacy HTTP client: {error}"), false))?;
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/proto")
        .header("connect-protocol-version", "1")
        .header(AUTHORIZATION, format!("Bearer {}", auth.auth_token))
        .header("x-cursor-checksum", create_cursor_checksum(&auth.machine_id, system_now_ms()))
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_namespace())
        .header("x-ghost-mode", "true")
        .header("x-request-id", Uuid::new_v4().to_string())
        .body(vec![0x08, PrivacyMode::NoStorage as u8])
        .send()
        .map_err(|error| {
            let retryable = error.is_timeout() || error.is_connect();
            PrivacyLookupError::new(format!("privacy transport failed: {error}"), retryable)
        })?;
    let status = response.status();
    if !status.is_success() {
        let retryable = status.as_u16() == 503 || status.as_u16() == 504;
        return Err(PrivacyLookupError::new(
            format!("privacy backend returned HTTP {}", status.as_u16()),
            retryable,
        ));
    }
    let bytes = response.bytes()
        .map_err(|error| PrivacyLookupError::new(format!("privacy response body failed: {error}"), true))?;
    decode_privacy_mode(bytes.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_backend_matches_ios_production_default_and_overrides() {
        assert_eq!(
            configured_backend_url_from(None, None),
            DEFAULT_CURSOR_BACKEND_URL
        );
        assert_eq!(
            configured_backend_url_from(None, Some("https://cursor.example.test".into())),
            "https://cursor.example.test"
        );
        assert_eq!(
            configured_backend_url_from(
                Some("https://sand.example.test".into()),
                Some("https://cursor.example.test".into()),
            ),
            "https://sand.example.test"
        );
    }

    #[test]
    fn protobuf_privacy_mode_decode_is_fail_closed_for_missing_and_unknown_values() {
        assert_eq!(decode_privacy_mode(&[]).unwrap(), PrivacyMode::Unspecified);
        assert_eq!(decode_privacy_mode(&[0x08, 4]).unwrap(), PrivacyMode::UsageCodebaseTrainingAllowed);
        assert_eq!(decode_privacy_mode(&[0x08, 99]).unwrap(), PrivacyMode::Unspecified);
    }
}
