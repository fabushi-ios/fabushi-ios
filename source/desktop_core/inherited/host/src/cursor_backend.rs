use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Client as AsyncClient;
use reqwest::blocking::Client as BlockingClient;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version,
};

pub const DASHBOARD_GET_ME_PATH: &str = "/aiserver.v1.DashboardService/GetMe";
pub const DASHBOARD_GET_USER_PRIVACY_MODE_PATH: &str =
    "/aiserver.v1.DashboardService/GetUserPrivacyMode";
pub const PRIVACY_MODE_CACHE_MAX_AGE_MS: u64 = 5 * 60_000;
pub const PRIVACY_MODE_FALLBACK_CACHE_MAX_AGE_MS: u64 = 10_000;
pub const PRIVACY_MODE_FETCH_TIMEOUT_MS: u64 = 3_000;
pub const GET_ME_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SandPrivacyMode {
    Unspecified = 0,
    NoStorage = 1,
    NoTraining = 2,
    UsageDataTrainingAllowed = 3,
    UsageCodebaseTrainingAllowed = 4,
}

impl SandPrivacyMode {
    fn from_proto(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            1 => Some(Self::NoStorage),
            2 => Some(Self::NoTraining),
            3 => Some(Self::UsageDataTrainingAllowed),
            4 => Some(Self::UsageCodebaseTrainingAllowed),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum CursorBackendError {
    #[error("invalid Cursor backend URL: {0}")]
    InvalidBackendUrl(String),
    #[error("Cursor backend transport failed: {0}")]
    Transport(String),
    #[error("Cursor backend request timed out: {0}")]
    Timeout(String),
    #[error("Cursor backend request cancelled: {0}")]
    Cancelled(String),
    #[error("Cursor backend returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("Cursor backend protobuf response was invalid: {0}")]
    InvalidProto(String),
}

#[derive(Debug, Clone)]
struct PrivacyCacheEntry {
    backend_url: String,
    account_scope: String,
    value: Option<SandPrivacyMode>,
    expires_at: Instant,
}

static PRIVACY_MODE_CACHE: OnceLock<Mutex<Option<PrivacyCacheEntry>>> = OnceLock::new();

fn privacy_cache() -> &'static Mutex<Option<PrivacyCacheEntry>> {
    PRIVACY_MODE_CACHE.get_or_init(|| Mutex::new(None))
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn create_cursor_checksum(machine_id: &str, now_ms: u64) -> String {
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

fn jwt_subject(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let parsed: Value = serde_json::from_slice(&decoded).ok()?;
    parsed
        .get("sub")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn account_cache_scope(access_token: &str) -> String {
    let seed = jwt_subject(access_token).unwrap_or_else(|| access_token.to_string());
    format!("{:x}", Sha256::digest(seed.as_bytes()))
}

pub(crate) fn send_cursor_unary(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
    path: &str,
    body: &[u8],
    timeout_ms: u64,
    ghost_mode: &str,
) -> Result<Vec<u8>, CursorBackendError> {
    let request_id = Uuid::new_v4().to_string();
    send_cursor_unary_with_request_id(
        backend_url,
        access_token,
        machine_id,
        path,
        body,
        Some(timeout_ms),
        ghost_mode,
        &request_id,
    )
}



pub(crate) fn send_cursor_unary_with_headers(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
    path: &str,
    body: &[u8],
    timeout_ms: u64,
    ghost_mode: &str,
    extra_headers: &[(String, String)],
) -> Result<Vec<u8>, CursorBackendError> {
    let base = Url::parse(backend_url)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let url = base
        .join(path)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let client = BlockingClient::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
    let mut request = client
        .post(url)
        .header(CONTENT_TYPE, "application/proto")
        .header("connect-protocol-version", "1")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(
            "x-cursor-checksum",
            create_cursor_checksum(machine_id, system_now_ms()),
        )
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_box_namespace())
        .header("x-ghost-mode", ghost_mode)
        .header("x-request-id", Uuid::new_v4().to_string());

    for (name, value) in extra_headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
        request = request.header(name, value);
    }

    let response = request
        .body(body.to_vec())
        .send()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?
        .to_vec();
    if !status.is_success() {
        return Err(CursorBackendError::HttpStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).chars().take(512).collect(),
        });
    }
    Ok(bytes)
}

pub(crate) type CursorRequestCancellation =
    Arc<dyn Fn() -> bool + Send + Sync + 'static>;

pub(crate) fn send_cursor_unary_cancellable(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
    path: &str,
    body: &[u8],
    timeout_ms: u64,
    ghost_mode: &str,
    cancellation: CursorRequestCancellation,
) -> Result<Vec<u8>, CursorBackendError> {
    if cancellation() {
        return Err(CursorBackendError::Cancelled(
            "request was cancelled before dispatch".into(),
        ));
    }

    let backend_url = backend_url.to_string();
    let access_token = access_token.to_string();
    let machine_id = machine_id.to_string();
    let path = path.to_string();
    let body = body.to_vec();
    let ghost_mode = ghost_mode.to_string();
    let request_id = Uuid::new_v4().to_string();

    thread::Builder::new()
        .name("cursor-unary-cancellable".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
            runtime.block_on(async move {
                send_cursor_unary_async_with_request_id(
                    &backend_url,
                    &access_token,
                    &machine_id,
                    &path,
                    &body,
                    timeout_ms,
                    &ghost_mode,
                    &request_id,
                    cancellation,
                )
                .await
            })
        })
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?
        .join()
        .map_err(|_| {
            CursorBackendError::Transport(
                "cancellable Cursor backend worker panicked".into(),
            )
        })?
}

async fn send_cursor_unary_async_with_request_id(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
    path: &str,
    body: &[u8],
    timeout_ms: u64,
    ghost_mode: &str,
    request_id: &str,
    cancellation: CursorRequestCancellation,
) -> Result<Vec<u8>, CursorBackendError> {
    let base = Url::parse(backend_url)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let url = base
        .join(path)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let client = AsyncClient::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
    let request = client
        .post(url)
        .header(CONTENT_TYPE, "application/proto")
        .header("connect-protocol-version", "1")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(
            "x-cursor-checksum",
            create_cursor_checksum(machine_id, system_now_ms()),
        )
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_box_namespace())
        .header("x-ghost-mode", ghost_mode)
        .header("x-request-id", request_id)
        .body(body.to_vec())
        .send();

    tokio::pin!(request);
    let response = tokio::select! {
        result = &mut request => {
            result.map_err(|error| {
                if error.is_timeout() {
                    CursorBackendError::Timeout(error.to_string())
                } else {
                    CursorBackendError::Transport(error.to_string())
                }
            })?
        }
        _ = wait_for_cursor_cancellation(Arc::clone(&cancellation)) => {
            return Err(CursorBackendError::Cancelled(
                "request was cancelled while waiting for response headers".into(),
            ));
        }
    };
    let status = response.status();
    let response_body = response.bytes();
    tokio::pin!(response_body);
    let bytes = tokio::select! {
        result = &mut response_body => {
            result
                .map_err(|error| {
                    if error.is_timeout() {
                        CursorBackendError::Timeout(error.to_string())
                    } else {
                        CursorBackendError::Transport(error.to_string())
                    }
                })?
                .to_vec()
        }
        _ = wait_for_cursor_cancellation(cancellation) => {
            return Err(CursorBackendError::Cancelled(
                "request was cancelled while reading response body".into(),
            ));
        }
    };
    if !status.is_success() {
        return Err(CursorBackendError::HttpStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).chars().take(512).collect(),
        });
    }
    Ok(bytes)
}

async fn wait_for_cursor_cancellation(cancellation: CursorRequestCancellation) {
    while !cancellation() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(crate) fn send_cursor_unary_with_request_id(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
    path: &str,
    body: &[u8],
    timeout_ms: Option<u64>,
    ghost_mode: &str,
    request_id: &str,
) -> Result<Vec<u8>, CursorBackendError> {
    let base = Url::parse(backend_url)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let url = base
        .join(path)
        .map_err(|error| CursorBackendError::InvalidBackendUrl(error.to_string()))?;
    let mut builder = BlockingClient::builder();
    if let Some(timeout_ms) = timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    let client = builder
        .build()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/proto")
        .header("connect-protocol-version", "1")
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(
            "x-cursor-checksum",
            create_cursor_checksum(machine_id, system_now_ms()),
        )
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_box_namespace())
        .header("x-ghost-mode", ghost_mode)
        .header("x-request-id", request_id)
        .body(body.to_vec())
        .send()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|error| CursorBackendError::Transport(error.to_string()))?
        .to_vec();
    if !status.is_success() {
        return Err(CursorBackendError::HttpStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).chars().take(512).collect(),
        });
    }
    Ok(bytes)
}

fn decode_varint(input: &[u8], cursor: &mut usize) -> Result<u64, CursorBackendError> {
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
    Err(CursorBackendError::InvalidProto("malformed varint".into()))
}

fn skip_field(
    input: &[u8],
    cursor: &mut usize,
    wire_type: u8,
) -> Result<(), CursorBackendError> {
    match wire_type {
        0 => {
            let _ = decode_varint(input, cursor)?;
        }
        1 => {
            *cursor = cursor.saturating_add(8);
        }
        2 => {
            let length = usize::try_from(decode_varint(input, cursor)?)
                .map_err(|_| CursorBackendError::InvalidProto("length overflow".into()))?;
            *cursor = cursor.saturating_add(length);
        }
        5 => {
            *cursor = cursor.saturating_add(4);
        }
        other => {
            return Err(CursorBackendError::InvalidProto(format!(
                "unsupported wire type {other}"
            )));
        }
    }
    if *cursor > input.len() {
        return Err(CursorBackendError::InvalidProto(
            "truncated protobuf field".into(),
        ));
    }
    Ok(())
}

fn decode_optional_varint_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<u64>, CursorBackendError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 0 {
            return Ok(Some(decode_varint(input, &mut cursor)?));
        }
        skip_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn decode_optional_string_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<String>, CursorBackendError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 2 {
            let length = usize::try_from(decode_varint(input, &mut cursor)?)
                .map_err(|_| CursorBackendError::InvalidProto("length overflow".into()))?;
            let end = cursor.saturating_add(length);
            if end > input.len() {
                return Err(CursorBackendError::InvalidProto(
                    "truncated string field".into(),
                ));
            }
            let value = std::str::from_utf8(&input[cursor..end])
                .map_err(|_| CursorBackendError::InvalidProto("string field is not UTF-8".into()))?
                .trim()
                .to_string();
            return Ok((!value.is_empty()).then_some(value));
        }
        skip_field(input, &mut cursor, wire)?;
    }
    Ok(None)
}

pub fn get_sand_ghost_mode_header_from_privacy_mode(
    privacy_mode: Option<SandPrivacyMode>,
) -> &'static str {
    match privacy_mode {
        Some(SandPrivacyMode::UsageDataTrainingAllowed)
        | Some(SandPrivacyMode::UsageCodebaseTrainingAllowed) => "false",
        _ => "true",
    }
}

pub fn fetch_sand_privacy_mode(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
) -> Result<Option<SandPrivacyMode>, CursorBackendError> {
    let bytes = send_cursor_unary(
        backend_url,
        access_token,
        machine_id,
        DASHBOARD_GET_USER_PRIVACY_MODE_PATH,
        &[0x08, SandPrivacyMode::NoStorage as u8],
        PRIVACY_MODE_FETCH_TIMEOUT_MS,
        "true",
    )?;
    Ok(decode_optional_varint_field(&bytes, 1)?.and_then(SandPrivacyMode::from_proto))
}

pub fn clear_sand_privacy_mode_cache_for_testing() {
    *privacy_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

pub fn resolve_sand_privacy_mode(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
) -> Option<SandPrivacyMode> {
    let scope = account_cache_scope(access_token);
    let mut cache = privacy_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    if let Some(entry) = cache.as_ref() {
        if entry.backend_url == backend_url
            && entry.account_scope == scope
            && now <= entry.expires_at
        {
            return entry.value;
        }
    }

    let value = match fetch_sand_privacy_mode(backend_url, access_token, machine_id) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "[sand:privacy] privacy-mode lookup failed, using privacy-safe fallback backend={backend_url} error={}",
                std::any::type_name_of_val(&error)
            );
            None
        }
    };
    let ttl = if value.is_some() {
        PRIVACY_MODE_CACHE_MAX_AGE_MS
    } else {
        PRIVACY_MODE_FALLBACK_CACHE_MAX_AGE_MS
    };
    *cache = Some(PrivacyCacheEntry {
        backend_url: backend_url.to_string(),
        account_scope: scope,
        value,
        expires_at: now + Duration::from_millis(ttl),
    });
    value
}

pub fn resolve_sand_ghost_mode_header(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
) -> &'static str {
    get_sand_ghost_mode_header_from_privacy_mode(resolve_sand_privacy_mode(
        backend_url,
        access_token,
        machine_id,
    ))
}

pub fn fetch_sand_user_full_name(
    backend_url: &str,
    access_token: &str,
    machine_id: &str,
) -> Result<Option<String>, CursorBackendError> {
    let ghost_mode = resolve_sand_ghost_mode_header(backend_url, access_token, machine_id);
    let bytes = send_cursor_unary(
        backend_url,
        access_token,
        machine_id,
        DASHBOARD_GET_ME_PATH,
        &[],
        GET_ME_TIMEOUT_MS,
        ghost_mode,
    )?;
    let first_name = decode_optional_string_field(&bytes, 4)?;
    let last_name = decode_optional_string_field(&bytes, 5)?;
    let parts = [first_name, last_name]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>();
    Ok((!parts.is_empty()).then(|| parts.join(" ")))
}
