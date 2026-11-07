use std::env;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::blocking::Client;
use serde_json::Value;
use thiserror::Error;
use url::Url;

pub const SAND_INFERENCE_RENEWAL_CREDENTIAL_ENV: &str = "SAND_INFERENCE_RENEWAL_CREDENTIAL";
pub const SAND_DEV_INFERENCE_TOKEN_FILE_ENV: &str = "SAND_DEV_INFERENCE_TOKEN_FILE";
pub const REFRESH_LEEWAY_MS: u64 = 2 * 60 * 1_000;
pub const MIN_REFRESH_INTERVAL_MS: u64 = 30 * 1_000;
pub const MAX_REFRESH_INTERVAL_MS: u64 = 30 * 60 * 1_000;
pub const CREDENTIAL_RETRY_BASE_DELAY_MS: u64 = 5 * 1_000;
pub const CREDENTIAL_RETRY_MAX_DELAY_MS: u64 = 5 * 60 * 1_000;
pub const DEFAULT_TTL_MS: u64 = 10 * 60 * 1_000;
pub const RENEWAL_PATH: &str = "/sand-box/inference-credential";
// Historical symbol retained for source compatibility; no foreign service fallback.
pub const DEFAULT_CURSOR_BACKEND_URL: &str = "https://api.ombhrum.com";
pub const SAND_CLIENT_TYPE: &str = "sand";
pub const SAND_CLIENT_FALLBACK_BASE_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceCredential {
    pub access_token: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenewalOutcome {
    Renewed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalResult {
    pub outcome: RenewalOutcome,
    pub consecutive_failures: u32,
    pub duration_ms: u64,
    pub error_summary: Option<String>,
}

#[derive(Debug, Error)]
pub enum SandCredentialRenewalError {
    #[error("{0}")]
    InvalidPayload(String),
    #[error("invalid backend URL: {0}")]
    InvalidBackendUrl(String),
    #[error("Sand inference-credential renewal request failed: {0}")]
    Transport(String),
    #[error("Sand inference-credential renewal failed (HTTP {0}).")]
    HttpStatus(u16),
    #[error("read inference credential file: {0}")]
    ReadFile(#[from] std::io::Error),
    #[error("parse inference credential JSON: {0}")]
    ParseJson(#[from] serde_json::Error),
}

pub fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn get_access_token_expiry_ms(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    if payload.is_empty() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let parsed: Value = serde_json::from_slice(&decoded).ok()?;
    let exp = parsed.as_object()?.get("exp")?.as_f64()?;
    if !exp.is_finite() || exp < 0.0 {
        return None;
    }
    let millis = exp * 1_000.0;
    (millis <= u64::MAX as f64).then_some(millis as u64)
}

fn credential_from_payload(
    parsed: Value,
    absent_message: &str,
    observed_now_ms: u64,
) -> Result<InferenceCredential, SandCredentialRenewalError> {
    let object = parsed
        .as_object()
        .ok_or_else(|| SandCredentialRenewalError::InvalidPayload(absent_message.into()))?;
    let access_token = object
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access_token.is_empty() {
        return Err(SandCredentialRenewalError::InvalidPayload(absent_message.into()));
    }
    let expires_at_ms = object
        .get("expiresAtMs")
        .and_then(Value::as_u64)
        .or_else(|| get_access_token_expiry_ms(&access_token))
        .unwrap_or_else(|| observed_now_ms.saturating_add(DEFAULT_TTL_MS));
    Ok(InferenceCredential {
        access_token,
        expires_at_ms,
    })
}

pub fn read_dev_inference_credential_file(
    path: &Path,
) -> Result<InferenceCredential, SandCredentialRenewalError> {
    let raw = fs::read_to_string(path)?;
    let parsed = serde_json::from_str::<Value>(&raw)?;
    credential_from_payload(
        parsed,
        &format!("Dev inference token file {} has no accessToken yet.", path.display()),
        system_now_ms(),
    )
}

pub fn get_configured_backend_url() -> Result<String, SandCredentialRenewalError> {
    configured_product_backend_url(
        env::var("FABUSHI_API_BASE_URL").ok().as_deref(),
        env::var("SAND_BACKEND_URL").ok().as_deref(),
    )
}

pub fn configured_product_backend_url(
    native: Option<&str>,
    legacy: Option<&str>,
) -> Result<String, SandCredentialRenewalError> {
    let raw = native.or(legacy).unwrap_or(DEFAULT_CURSOR_BACKEND_URL).trim();
    let url = Url::parse(raw)
        .map_err(|_| SandCredentialRenewalError::InvalidBackendUrl("invalid product service origin".into()))?;
    let host = url.host_str().unwrap_or_default();
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    let approved = matches!(url.origin().ascii_serialization().as_str(),
        "https://api.ombhrum.com" | "https://fabushi-ci-isolated-auth.bhrumom.workers.dev");
    if (!url.username().is_empty() || url.password().is_some())
        || url.query().is_some() || url.fragment().is_some()
        || (url.scheme() != "https" && !(loopback && url.scheme() == "http"))
        || (!loopback && (!approved || url.path() != "/" || url.port().is_some()))
    {
        return Err(SandCredentialRenewalError::InvalidBackendUrl(
            "product services require an approved Fabushi origin or loopback".into(),
        ));
    }
    Ok(url.to_string())
}

pub fn sand_box_namespace() -> &'static str {
    match env::var("SAND_BOX_OWNER_NAMESPACE").ok().as_deref() {
        Some("dev") => "dev",
        Some("lab") => "lab",
        _ if env::var("SAND_PACKAGED").ok().as_deref() != Some("1") => "dev",
        _ if env::var("SAND_LAB").ok().as_deref() == Some("1") => "lab",
        _ => "prod",
    }
}

fn sand_client_base_version() -> String {
    let stamped = env::var("SAND_CLIENT_APP_VERSION").unwrap_or_default();
    let base = stamped.split('-').next().unwrap_or_default();
    let valid = {
        let parts = base.split('.').collect::<Vec<_>>();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    if valid {
        base.to_string()
    } else {
        SAND_CLIENT_FALLBACK_BASE_VERSION.into()
    }
}

pub fn sand_client_version() -> String {
    let base = sand_client_base_version();
    match sand_box_namespace() {
        "dev" => format!("{base}-dev"),
        "lab" => format!("{base}-lab"),
        _ => base,
    }
}

pub fn renew_sand_box_inference_credential(
    backend_url: &str,
    credential: &str,
) -> Result<InferenceCredential, SandCredentialRenewalError> {
    let backend = Url::parse(backend_url)
        .map_err(|error| SandCredentialRenewalError::InvalidBackendUrl(error.to_string()))?;
    let url = backend
        .join(RENEWAL_PATH)
        .map_err(|error| SandCredentialRenewalError::InvalidBackendUrl(error.to_string()))?;
    let response = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| SandCredentialRenewalError::Transport(error.to_string()))?
        .post(url)
        .header("content-type", "application/json")
        .header("x-cursor-client-type", SAND_CLIENT_TYPE)
        .header("x-cursor-client-version", sand_client_version())
        .header("x-sand-box-namespace", sand_box_namespace())
        .json(&serde_json::json!({ "credential": credential }))
        .send()
        .map_err(|error| SandCredentialRenewalError::Transport(error.to_string()))?;

    if !response.status().is_success() {
        return Err(SandCredentialRenewalError::HttpStatus(response.status().as_u16()));
    }
    let parsed = response
        .json::<Value>()
        .map_err(|error| SandCredentialRenewalError::Transport(error.to_string()))?;
    credential_from_payload(
        parsed,
        "Sand inference-credential renewal returned no token.",
        system_now_ms(),
    )
}

pub fn refresh_delay_ms(expires_at_ms: u64, observed_now_ms: u64) -> u64 {
    expires_at_ms
        .saturating_sub(observed_now_ms)
        .saturating_sub(REFRESH_LEEWAY_MS)
        .clamp(MIN_REFRESH_INTERVAL_MS, MAX_REFRESH_INTERVAL_MS)
}

pub fn retry_delay_ms(attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1).min(31);
    let factor = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    CREDENTIAL_RETRY_BASE_DELAY_MS
        .saturating_mul(factor)
        .min(CREDENTIAL_RETRY_MAX_DELAY_MS)
}

pub fn redact_renewal_error_for_report(raw: &str) -> String {
    let mut words = Vec::new();
    for word in raw.split_whitespace() {
        let redacted = if word.starts_with("http://") || word.starts_with("https://") {
            "<url>".to_string()
        } else if word.starts_with('/') {
            "<path>".to_string()
        } else if word.len() >= 24
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            "<id>".to_string()
        } else {
            word.to_string()
        };
        words.push(redacted);
    }
    words.join(" ").chars().take(160).collect()
}

pub trait CredentialRenewalBackend: Send + Sync + 'static {
    fn renew(&self, credential: &str) -> Result<InferenceCredential, SandCredentialRenewalError>;
}

#[derive(Debug, Clone)]
pub struct HttpCredentialRenewalBackend {
    backend_url: String,
}

impl HttpCredentialRenewalBackend {
    pub fn new(backend_url: impl Into<String>) -> Self {
        Self {
            backend_url: backend_url.into(),
        }
    }
}

impl CredentialRenewalBackend for HttpCredentialRenewalBackend {
    fn renew(&self, credential: &str) -> Result<InferenceCredential, SandCredentialRenewalError> {
        renew_sand_box_inference_credential(&self.backend_url, credential)
    }
}

pub struct CredentialRenewerHooks {
    pub get_credential: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub set_credential: Arc<dyn Fn(InferenceCredential) + Send + Sync>,
    pub on_result: Option<Arc<dyn Fn(RenewalResult) + Send + Sync>>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl CredentialRenewerHooks {
    pub fn production(
        get_credential: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        set_credential: Arc<dyn Fn(InferenceCredential) + Send + Sync>,
        on_result: Option<Arc<dyn Fn(RenewalResult) + Send + Sync>>,
    ) -> Self {
        Self {
            get_credential,
            set_credential,
            on_result,
            now_ms: Arc::new(system_now_ms),
        }
    }
}

#[derive(Debug)]
struct RenewerState {
    closed: bool,
    waiting: bool,
    requested_epoch: u64,
    completed_epoch: u64,
}

pub struct SandInferenceCredentialRenewer {
    state: Arc<(Mutex<RenewerState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    backend: Arc<dyn CredentialRenewalBackend>,
    hooks: Arc<CredentialRenewerHooks>,
}

impl SandInferenceCredentialRenewer {
    pub fn new(
        backend: Arc<dyn CredentialRenewalBackend>,
        hooks: CredentialRenewerHooks,
    ) -> Self {
        Self {
            state: Arc::new((
                Mutex::new(RenewerState {
                    closed: false,
                    waiting: false,
                    requested_epoch: 0,
                    completed_epoch: 0,
                }),
                Condvar::new(),
            )),
            worker: None,
            backend,
            hooks: Arc::new(hooks),
        }
    }

    pub fn start(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let state = Arc::clone(&self.state);
        let backend = Arc::clone(&self.backend);
        let hooks = Arc::clone(&self.hooks);
        self.worker = Some(thread::spawn(move || {
            let mut consecutive_failures = 0_u32;
            loop {
                {
                    let guard = state.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if guard.closed {
                        break;
                    }
                }

                let started = Instant::now();
                let renewal_credential = (hooks.get_credential)();
                let delay_ms = match renewal_credential.filter(|value| !value.is_empty()) {
                    None => MAX_REFRESH_INTERVAL_MS,
                    Some(renewal_credential) => match backend.renew(&renewal_credential) {
                        Ok(credential) => {
                            let expires_at_ms = credential.expires_at_ms;
                            (hooks.set_credential)(credential);
                            consecutive_failures = 0;
                            if let Some(on_result) = &hooks.on_result {
                                on_result(RenewalResult {
                                    outcome: RenewalOutcome::Renewed,
                                    consecutive_failures,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    error_summary: None,
                                });
                            }
                            refresh_delay_ms(expires_at_ms, (hooks.now_ms)())
                        }
                        Err(error) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            if let Some(on_result) = &hooks.on_result {
                                on_result(RenewalResult {
                                    outcome: RenewalOutcome::Failed,
                                    consecutive_failures,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    error_summary: Some(redact_renewal_error_for_report(
                                        &error.to_string(),
                                    )),
                                });
                            }
                            retry_delay_ms(consecutive_failures)
                        }
                    },
                };

                let (lock, wake) = &*state;
                let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                guard.completed_epoch = guard.requested_epoch;
                wake.notify_all();
                if guard.closed {
                    break;
                }
                guard.waiting = true;
                let requested = guard.requested_epoch;
                let duration = Duration::from_millis(delay_ms);
                let (next_guard, _) = wake
                    .wait_timeout_while(guard, duration, |current| {
                        !current.closed && current.requested_epoch == requested
                    })
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                guard = next_guard;
                guard.waiting = false;
                if guard.closed {
                    break;
                }
            }
        }));
    }

    pub fn request_immediate_renewal(&self) -> bool {
        let (lock, wake) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.closed || !guard.waiting {
            return false;
        }
        guard.requested_epoch = guard.requested_epoch.saturating_add(1);
        let requested = guard.requested_epoch;
        wake.notify_all();
        while !guard.closed && guard.completed_epoch < requested {
            guard = wake
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        !guard.closed
    }

    pub fn close(&mut self) {
        let (lock, wake) = &*self.state;
        {
            let mut guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.closed = true;
            wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SandInferenceCredentialRenewer {
    fn drop(&mut self) {
        self.close();
    }
}
