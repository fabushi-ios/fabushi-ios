use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::extensions::auth::auth_service::{
    FabushiCoreCredential, FabushiCoreCredentialProvider,
};
use crate::network_worker_process::{
    NETWORK_WORKER_START_TIMEOUT_MS, NETWORK_WORKER_STOP_TIMEOUT_MS, NetworkWorkerProcessOptions,
    OwnedNetworkWorker, default_network_worker_executable, start_network_worker,
};
use crate::remote_computer_target::RemoteComputerDeviceSecretProvider;

const HTTP_TIMEOUT_SECS: u64 = 15;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const SUPERVISOR_POLL_MS: u64 = 5_000;
const SERVICE_SUPERVISOR_POLL_MS: u64 = 50;
const STATE_VERSION: u32 = 1;

pub const FABUSHI_API_BASE_URL_ENV: &str = "FABUSHI_API_BASE_URL";
pub const DEFAULT_FABUSHI_API_BASE_URL: &str = "https://api.ombhrum.com";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NetworkEnrollmentError {
    #[error("Fabushi network authority is unavailable.")]
    AuthorityUnavailable,
    #[error("Fabushi network enrollment request transport failed.")]
    RequestFailed,
    #[error("Fabushi network enrollment request was explicitly rejected.")]
    RequestRejected,
    #[error("Fabushi network enrollment response failed validation.")]
    InvalidResponse,
    #[error("Fabushi network worker execution outcome is unknown.")]
    OutcomeUnknown,
    #[error("Fabushi network enrollment state failed.")]
    StateFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollmentState {
    version: u32,
    user_id: String,
    device_id: String,
    request_id: String,
    state_key_b64: String,
    phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cell_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enrollment_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    control_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
}

impl EnrollmentState {
    fn new(credential: &FabushiCoreCredential) -> Result<Self, NetworkEnrollmentError> {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).map_err(|_| NetworkEnrollmentError::StateFailed)?;
        Ok(Self {
            version: STATE_VERSION,
            user_id: credential.user_id.clone(),
            device_id: credential.device_id.clone(),
            request_id: format!("network-enroll-request-{}", Uuid::new_v4().simple()),
            state_key_b64: BASE64_STANDARD.encode(key),
            phase: "grant_pending".into(),
            grant_id: None,
            grant_secret: None,
            cell_id: None,
            generation: None,
            enrollment_expires_at: None,
            control_url: None,
            auth_key: None,
            node_id: None,
        })
    }

    fn identity_matches(&self, credential: &FabushiCoreCredential) -> bool {
        self.version == STATE_VERSION
            && self.user_id == credential.user_id
            && self.device_id == credential.device_id
            && self.request_id.len() >= 8
            && BASE64_STANDARD
                .decode(&self.state_key_b64)
                .is_ok_and(|value| value.len() == 32)
    }

    fn clear_one_time_secrets(&mut self) {
        self.grant_secret = None;
        self.auth_key = None;
    }

    fn reset_expired_uncertain_start(&mut self, now: i64) -> bool {
        if self.phase != "worker_start_outcome_unknown"
            || self.enrollment_expires_at.is_none_or(|expires_at| expires_at > now)
        {
            return false;
        }
        self.request_id = format!("network-enroll-request-{}", Uuid::new_v4().simple());
        self.phase = "grant_pending".into();
        self.grant_id = None;
        self.grant_secret = None;
        self.cell_id = None;
        self.generation = None;
        self.enrollment_expires_at = None;
        self.control_url = None;
        self.auth_key = None;
        self.node_id = None;
        true
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoreGrant {
    grant_id: String,
    grant_secret: String,
    cell_id: String,
    generation: i64,
    expires_at: i64,
    request_id: String,
}

fn validate_base_url(raw: &str) -> Result<Url, NetworkEnrollmentError> {
    let mut url = Url::parse(raw.trim()).map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?;
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(NetworkEnrollmentError::AuthorityUnavailable);
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(loopback && url.scheme() == "http") {
        return Err(NetworkEnrollmentError::AuthorityUnavailable);
    }
    url.set_path("/");
    Ok(url)
}

fn validate_control_url(raw: &str) -> Result<String, NetworkEnrollmentError> {
    let url = Url::parse(raw.trim()).map_err(|_| NetworkEnrollmentError::InvalidResponse)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(NetworkEnrollmentError::InvalidResponse);
    }
    Ok(url.origin().ascii_serialization())
}

fn endpoint(base: &Url, segments: &[&str]) -> Result<Url, NetworkEnrollmentError> {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?;
        path.clear();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

fn identity_scope(credential: &FabushiCoreCredential) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"fabushi-fdn-host-state-v1\0");
    hasher.update(credential.user_id.as_bytes());
    hasher.update([0]);
    hasher.update(credential.device_id.as_bytes());
    let digest = hasher.finalize();
    digest[..20].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn private_write(path: &Path, bytes: &[u8]) -> Result<(), NetworkEnrollmentError> {
    let parent = path.parent().ok_or(NetworkEnrollmentError::StateFailed)?;
    fs::create_dir_all(parent).map_err(|_| NetworkEnrollmentError::StateFailed)?;
    let temp = parent.join(format!(".fdn-state-{}-{}.tmp", std::process::id(), Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|_| NetworkEnrollmentError::StateFailed)?;
    file.write_all(bytes).map_err(|_| NetworkEnrollmentError::StateFailed)?;
    file.sync_all().map_err(|_| NetworkEnrollmentError::StateFailed)?;
    fs::rename(&temp, path).map_err(|_| {
        let _ = fs::remove_file(&temp);
        NetworkEnrollmentError::StateFailed
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn read_state(path: &Path) -> Result<Option<EnrollmentState>, NetworkEnrollmentError> {
    match fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .map(Some)
            .map_err(|_| NetworkEnrollmentError::StateFailed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(NetworkEnrollmentError::StateFailed),
    }
}

fn write_state(path: &Path, state: &EnrollmentState) -> Result<(), NetworkEnrollmentError> {
    let bytes = serde_json::to_vec(state).map_err(|_| NetworkEnrollmentError::StateFailed)?;
    private_write(path, &bytes)
}

pub struct NetworkEnrollmentOwner {
    base_url: Url,
    credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
    device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
    state_root: PathBuf,
    executable_path: PathBuf,
    http: Client,
}

impl std::fmt::Debug for NetworkEnrollmentOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkEnrollmentOwner")
            .field("base_url", &self.base_url)
            .field("credential_provider", &"<first-party>")
            .field("device_secret_provider", &"<redacted>")
            .field("state_root", &self.state_root)
            .field("executable_path", &self.executable_path)
            .finish_non_exhaustive()
    }
}

impl NetworkEnrollmentOwner {
    pub fn from_environment(
        credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
        device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
        state_root: impl Into<PathBuf>,
    ) -> Result<Self, NetworkEnrollmentError> {
        let raw = std::env::var(FABUSHI_API_BASE_URL_ENV)
            .unwrap_or_else(|_| DEFAULT_FABUSHI_API_BASE_URL.into());
        Self::new(
            &raw,
            credential_provider,
            device_secret_provider,
            state_root,
            default_network_worker_executable().map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?,
        )
    }

    pub fn new(
        base_url: &str,
        credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
        device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
        state_root: impl Into<PathBuf>,
        executable_path: impl Into<PathBuf>,
    ) -> Result<Self, NetworkEnrollmentError> {
        Ok(Self {
            base_url: validate_base_url(base_url)?,
            credential_provider,
            device_secret_provider,
            state_root: state_root.into(),
            executable_path: executable_path.into(),
            http: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?,
        })
    }

    pub fn current_identity(&self) -> Result<String, NetworkEnrollmentError> {
        let credential = self.credential()?;
        Ok(format!("{}\0{}", credential.user_id, credential.device_id))
    }

    fn credential(&self) -> Result<FabushiCoreCredential, NetworkEnrollmentError> {
        let credential = self.credential_provider
            .get_fabushi_core_credential()
            .map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?;
        credential.authorize_origin(self.base_url.as_str())
            .map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?;
        Ok(credential)
    }

    fn authority(&self) -> Result<(FabushiCoreCredential, String), NetworkEnrollmentError> {
        let credential = self.credential()?;
        let secret = self.device_secret_provider
            .get_device_secret(&credential)
            .map_err(|_| NetworkEnrollmentError::AuthorityUnavailable)?;
        if !(48..=256).contains(&secret.len()) {
            return Err(NetworkEnrollmentError::AuthorityUnavailable);
        }
        Ok((credential, secret))
    }

    fn post(&self, credential: &FabushiCoreCredential, url: Url, body: Value) -> Result<Value, NetworkEnrollmentError> {
        let response = self.http
            .post(url)
            .bearer_auth(credential.access_token())
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .map_err(|_| NetworkEnrollmentError::RequestFailed)?;
        if !response.status().is_success() {
            return Err(NetworkEnrollmentError::RequestRejected);
        }
        let bytes = response.bytes().map_err(|_| NetworkEnrollmentError::RequestFailed)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        serde_json::from_slice(&bytes).map_err(|_| NetworkEnrollmentError::InvalidResponse)
    }

    fn identity_paths(&self, credential: &FabushiCoreCredential) -> (PathBuf, PathBuf) {
        let root = self.state_root.join(identity_scope(credential));
        (root.join("enrollment.json"), root.join("tsnet"))
    }

    fn load_or_create_state(
        &self,
        credential: &FabushiCoreCredential,
        path: &Path,
    ) -> Result<EnrollmentState, NetworkEnrollmentError> {
        if let Some(state) = read_state(path)? {
            if !state.identity_matches(credential) {
                return Err(NetworkEnrollmentError::StateFailed);
            }
            return Ok(state);
        }
        let state = EnrollmentState::new(credential)?;
        write_state(path, &state)?;
        Ok(state)
    }

    fn issue_or_reconcile_grant(
        &self,
        credential: &FabushiCoreCredential,
        device_secret: &str,
        state_path: &Path,
        state: &mut EnrollmentState,
    ) -> Result<(), NetworkEnrollmentError> {
        if state.grant_id.is_some() && state.grant_secret.is_some() {
            return Ok(());
        }
        let value = self.post(
            credential,
            endpoint(&self.base_url, &[
                "v1", "computers", &credential.device_id, "network-enrollment-grants",
            ])?,
            json!({
                "deviceSecret": device_secret,
                "requestId": state.request_id,
            }),
        )?;
        let grant: CoreGrant = serde_json::from_value(value)
            .map_err(|_| NetworkEnrollmentError::InvalidResponse)?;
        if grant.request_id != state.request_id
            || grant.cell_id.trim().is_empty()
            || grant.generation < 1
            || grant.grant_id.trim().is_empty()
            || grant.grant_secret.len() < 80
        {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        state.grant_id = Some(grant.grant_id);
        state.grant_secret = Some(grant.grant_secret);
        state.cell_id = Some(grant.cell_id);
        state.generation = Some(grant.generation);
        state.enrollment_expires_at = Some(grant.expires_at);
        state.phase = "grant_ready".into();
        write_state(state_path, state)
    }

    fn start_or_reconcile_enrollment(
        &self,
        credential: &FabushiCoreCredential,
        state_path: &Path,
        state: &mut EnrollmentState,
    ) -> Result<(), NetworkEnrollmentError> {
        if state.control_url.is_some() && state.generation.is_some() {
            return Ok(());
        }
        let grant_id = state.grant_id.clone().ok_or(NetworkEnrollmentError::StateFailed)?;
        let grant_secret = state.grant_secret.clone().ok_or(NetworkEnrollmentError::StateFailed)?;
        let value = self.post(
            credential,
            endpoint(&self.base_url, &["api", "fdn", "network-enrollment", "start"])?,
            json!({"grantId": grant_id, "grantSecret": grant_secret}),
        )?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        let enrollment = value.get("enrollment").and_then(Value::as_object)
            .ok_or(NetworkEnrollmentError::InvalidResponse)?;
        if enrollment.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str())
            || enrollment.get("grantId").and_then(Value::as_str) != state.grant_id.as_deref()
        {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        let generation = enrollment.get("generation").and_then(Value::as_i64)
            .ok_or(NetworkEnrollmentError::InvalidResponse)?;
        if state.generation != Some(generation) {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        state.control_url = Some(validate_control_url(
            enrollment.get("controlUrl").and_then(Value::as_str)
                .ok_or(NetworkEnrollmentError::InvalidResponse)?,
        )?);
        if enrollment.get("completed").and_then(Value::as_bool) == Some(true) {
            state.phase = "completed".into();
            state.node_id = enrollment.get("nodeId").and_then(Value::as_str).map(str::to_string);
            state.clear_one_time_secrets();
        } else {
            let auth_key = enrollment.get("authKey").and_then(Value::as_str)
                .ok_or(NetworkEnrollmentError::InvalidResponse)?;
            if auth_key.is_empty() || auth_key.len() > 512 {
                return Err(NetworkEnrollmentError::InvalidResponse);
            }
            state.auth_key = Some(auth_key.to_string());
            state.phase = "key_ready".into();
        }
        write_state(state_path, state)
    }

    fn complete_enrollment(
        &self,
        credential: &FabushiCoreCredential,
        state_path: &Path,
        state: &mut EnrollmentState,
    ) -> Result<(), NetworkEnrollmentError> {
        let grant_id = state.grant_id.clone().ok_or(NetworkEnrollmentError::StateFailed)?;
        let value = self.post(
            credential,
            endpoint(&self.base_url, &["api", "fdn", "network-enrollment", "complete"])?,
            json!({"grantId": grant_id}),
        ).map_err(|error| {
            state.phase = "completion_outcome_unknown".into();
            let _ = write_state(state_path, state);
            match error {
                NetworkEnrollmentError::RequestFailed => NetworkEnrollmentError::OutcomeUnknown,
                other => other,
            }
        })?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        let enrollment = value.get("enrollment").and_then(Value::as_object)
            .ok_or(NetworkEnrollmentError::InvalidResponse)?;
        if enrollment.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str())
            || enrollment.get("grantId").and_then(Value::as_str) != state.grant_id.as_deref()
            || enrollment.get("generation").and_then(Value::as_i64) != state.generation
            || enrollment.get("completed").and_then(Value::as_bool) != Some(true)
        {
            return Err(NetworkEnrollmentError::InvalidResponse);
        }
        state.node_id = enrollment.get("nodeId").and_then(Value::as_str).map(str::to_string);
        state.phase = "completed".into();
        state.clear_one_time_secrets();
        write_state(state_path, state)
    }

    pub fn reconcile(&self) -> Result<OwnedNetworkWorker, NetworkEnrollmentError> {
        let (credential, device_secret) = self.authority()?;
        let (state_path, tsnet_state_dir) = self.identity_paths(&credential);
        let mut state = self.load_or_create_state(&credential, &state_path)?;

        if state.reset_expired_uncertain_start(unix_seconds()?) {
            write_state(&state_path, &state)?;
        }

        if state.phase == "completion_outcome_unknown" {
            self.complete_enrollment(&credential, &state_path, &mut state)?;
        }

        if state.phase != "completed" {
            self.issue_or_reconcile_grant(
                &credential,
                &device_secret,
                &state_path,
                &mut state,
            )?;
            self.start_or_reconcile_enrollment(&credential, &state_path, &mut state)?;
        }

        let control_url = state.control_url.clone().ok_or(NetworkEnrollmentError::StateFailed)?;
        let auth_key = match state.phase.as_str() {
            "key_ready" => state.auth_key.clone().ok_or(NetworkEnrollmentError::StateFailed)?,
            "worker_start_outcome_unknown" | "completion_outcome_unknown" | "completed" => String::new(),
            _ => String::new(),
        };
        let hostname_suffix: String = credential.device_id
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            .take(96)
            .collect();
        let hostname = format!("fabushi-{hostname_suffix}");
        let mut worker = start_network_worker(NetworkWorkerProcessOptions {
            executable_path: self.executable_path.clone(),
            hostname,
            control_url,
            auth_key,
            state_dir: tsnet_state_dir,
            state_key_b64: state.state_key_b64.clone(),
            start_timeout_ms: NETWORK_WORKER_START_TIMEOUT_MS,
            stop_timeout_ms: NETWORK_WORKER_STOP_TIMEOUT_MS,
        }).map_err(|_| {
            if state.phase == "key_ready" {
                state.phase = "worker_start_outcome_unknown".into();
                state.auth_key = None;
                let _ = write_state(&state_path, &state);
                NetworkEnrollmentError::OutcomeUnknown
            } else {
                NetworkEnrollmentError::RequestFailed
            }
        })?;

        let status = worker.status().map_err(|_| NetworkEnrollmentError::RequestFailed)?;
        if status.backend_state != "Running" || status.tailscale_ips.is_empty() {
            let _ = worker.close();
            return Err(NetworkEnrollmentError::RequestFailed);
        }

        if state.phase != "completed" {
            state.phase = "completion_pending".into();
            write_state(&state_path, &state)?;
            if let Err(error) = self.complete_enrollment(&credential, &state_path, &mut state) {
                let _ = worker.close();
                return Err(error);
            }
        }
        Ok(worker)
    }
}

fn unix_seconds() -> Result<i64, NetworkEnrollmentError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NetworkEnrollmentError::StateFailed)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| NetworkEnrollmentError::StateFailed)
}

pub trait NetworkWorkerService: Send + Sync {
    fn poll(&self, worker: &mut OwnedNetworkWorker) -> Result<(), NetworkEnrollmentError>;
}

pub struct NetworkEnrollmentSupervisor {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    service: Arc<Mutex<Option<Arc<dyn NetworkWorkerService>>>>,
}

impl NetworkEnrollmentSupervisor {
    pub fn start(owner: NetworkEnrollmentOwner) -> Result<Self, NetworkEnrollmentError> {
        let service = Arc::new(Mutex::new(None));
        Self::start_with_shared_service(owner, service)
    }

    pub fn start_with_service(
        owner: NetworkEnrollmentOwner,
        service: Option<Arc<dyn NetworkWorkerService>>,
    ) -> Result<Self, NetworkEnrollmentError> {
        let supervisor = NetworkEnrollmentSupervisor::start(owner)?;
        if let Some(service) = service {
            supervisor.install_service(service)?;
        }
        Ok(supervisor)
    }

    fn install_service(
        &self,
        service: Arc<dyn NetworkWorkerService>,
    ) -> Result<(), NetworkEnrollmentError> {
        let mut slot = self
            .service
            .lock()
            .map_err(|_| NetworkEnrollmentError::StateFailed)?;
        *slot = Some(service);
        Ok(())
    }

    fn start_with_shared_service(
        owner: NetworkEnrollmentOwner,
        service: Arc<Mutex<Option<Arc<dyn NetworkWorkerService>>>>,
    ) -> Result<Self, NetworkEnrollmentError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let service_worker = Arc::clone(&service);
        let join = thread::Builder::new()
            .name("fabushi-fdn-network-owner".into())
            .spawn(move || {
                let mut current_identity: Option<String> = None;
                let mut worker: Option<OwnedNetworkWorker> = None;
                while !stop_worker.load(Ordering::Acquire) {
                    match owner.current_identity() {
                        Ok(identity) => {
                            if current_identity.as_deref() != Some(identity.as_str()) {
                                if let Some(mut existing) = worker.take() {
                                    let _ = existing.close();
                                }
                                current_identity = Some(identity);
                            }
                            let healthy = worker
                                .as_mut()
                                .and_then(|owned| owned.status().ok())
                                .is_some_and(|status| status.backend_state == "Running");
                            if !healthy {
                                if let Some(mut existing) = worker.take() {
                                    let _ = existing.close();
                                }
                                match owner.reconcile() {
                                    Ok(owned) => worker = Some(owned),
                                    Err(NetworkEnrollmentError::AuthorityUnavailable) => {}
                                    Err(NetworkEnrollmentError::OutcomeUnknown) => {}
                                    Err(_) => {}
                                }
                            }
                            let service = service_worker
                                .lock()
                                .ok()
                                .and_then(|slot| slot.clone());
                            let reset_worker = if let (Some(runtime), Some(owned)) =
                                (service.as_ref(), worker.as_mut())
                            {
                                matches!(
                                    runtime.poll(owned),
                                    Err(NetworkEnrollmentError::StateFailed)
                                )
                            } else {
                                false
                            };
                            if reset_worker {
                                if let Some(mut existing) = worker.take() {
                                    let _ = existing.close();
                                }
                            }
                        }
                        Err(NetworkEnrollmentError::AuthorityUnavailable) => {
                            if let Some(mut existing) = worker.take() {
                                let _ = existing.close();
                            }
                            current_identity = None;
                        }
                        Err(_) => {}
                    }
                    let has_service = service_worker
                        .lock()
                        .map(|slot| slot.is_some())
                        .unwrap_or(false);
                    let idle_ms = if has_service {
                        SERVICE_SUPERVISOR_POLL_MS
                    } else {
                        SUPERVISOR_POLL_MS
                    };
                    for _ in 0..50 {
                        if stop_worker.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(idle_ms / 50));
                    }
                }
                if let Some(mut existing) = worker.take() {
                    let _ = existing.close();
                }
            })
            .map_err(|_| NetworkEnrollmentError::StateFailed)?;
        Ok(Self {
            stop,
            join: Some(join),
            service,
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for NetworkEnrollmentSupervisor {
    fn drop(&mut self) {
        self.stop();
    }
}
