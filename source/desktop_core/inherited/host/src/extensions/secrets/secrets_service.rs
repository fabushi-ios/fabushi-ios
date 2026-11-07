use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::host_paths::get_sand_root_dir;
use crate::r#box::box_env::BoxEnvironmentUpdate;

pub const BOX_SECRET_REDACTION_NAMES_ENV_VAR: &str = "CLOUD_AGENT_INJECTED_SECRET_NAMES";
pub const BOX_SECRETS_FILENAME: &str = "box-secrets.json";
pub const MAX_BOX_SECRET_COUNT: usize = 100;
pub const MAX_BOX_SECRET_VALUE_LENGTH: usize = 32 * 1024;
pub const MAX_BOX_SECRETS_TOTAL_LENGTH: usize = 96 * 1024;
pub const SECRETS_APPLY_WAIT_MS: u64 = 5_000;
pub const SECRETS_RETRY_INITIAL_MS: u64 = 1_000;
pub const SECRETS_RETRY_MAX_MS: u64 = 30_000;

const RESERVED_EXACT_BOX_SECRET_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "TERM",
    "PWD",
    "DISPLAY",
    BOX_SECRET_REDACTION_NAMES_ENV_VAR,
];
const RESERVED_BOX_SECRET_PREFIXES: &[&str] = &["SAND_", "__CURSOR", "LD_"];

pub fn get_box_secrets_store_path() -> PathBuf {
    get_sand_root_dir().join(BOX_SECRETS_FILENAME)
}

fn js_len(value: &str) -> usize {
    value.encode_utf16().count()
}

pub fn validate_box_secret_key(key: &str) -> Option<String> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Some("\"\" is not a valid environment variable name".into());
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || chars.any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'))
    {
        return Some(format!("\"{key}\" is not a valid environment variable name"));
    }
    if RESERVED_EXACT_BOX_SECRET_NAMES.contains(&key) {
        return Some(format!("{key} is reserved by the box runtime"));
    }
    for prefix in RESERVED_BOX_SECRET_PREFIXES {
        if key.starts_with(prefix) {
            return Some(format!(
                "Names starting with {prefix} are reserved by the box runtime"
            ));
        }
    }
    if key.to_ascii_lowercase().contains("cursor_sandbox") {
        return Some(format!("{key} is reserved by the box runtime"));
    }
    None
}

pub fn validate_box_secrets(secrets: &BTreeMap<String, String>) -> Option<String> {
    if secrets.len() > MAX_BOX_SECRET_COUNT {
        return Some(format!("Too many secrets (max {MAX_BOX_SECRET_COUNT})"));
    }
    let mut total = 0usize;
    for (key, value) in secrets {
        if let Some(error) = validate_box_secret_key(key) {
            return Some(error);
        }
        let value_len = js_len(value);
        if value_len > MAX_BOX_SECRET_VALUE_LENGTH {
            return Some(format!(
                "The value of {key} is too large (max 32,768 characters)"
            ));
        }
        total = total.saturating_add(js_len(key)).saturating_add(value_len);
    }
    (total > MAX_BOX_SECRETS_TOTAL_LENGTH)
        .then(|| "The combined size of all secrets is too large".to_string())
}

pub fn build_box_secrets_env(
    secrets: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    if secrets.is_empty() {
        return BTreeMap::new();
    }
    let mut env = secrets.clone();
    env.insert(
        BOX_SECRET_REDACTION_NAMES_ENV_VAR.into(),
        secrets.keys().cloned().collect::<Vec<_>>().join(","),
    );
    env
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSecretsStatus {
    pub keys: Vec<String>,
    pub is_applied: bool,
    pub last_applied_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BoxSecretsApplyError {
    #[error("box environment sync is unsupported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Retryable(String),
}

#[derive(Debug, thiserror::Error)]
pub enum BoxSecretsSetError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type BoxSecretsApply =
    Arc<dyn Fn(&BoxEnvironmentUpdate) -> Result<(), BoxSecretsApplyError> + Send + Sync>;
pub type BoxSecretsLog = Arc<dyn Fn(&str) + Send + Sync>;

pub struct BoxSecretsApplierOptions {
    pub apply_to_box: BoxSecretsApply,
    pub store_path: PathBuf,
    pub log: BoxSecretsLog,
    pub retry_initial: Duration,
    pub retry_max: Duration,
    pub apply_wait: Duration,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl BoxSecretsApplierOptions {
    pub fn new(apply_to_box: BoxSecretsApply) -> Self {
        Self {
            apply_to_box,
            store_path: get_box_secrets_store_path(),
            log: Arc::new(|message| eprintln!("[sand-host] {message}")),
            retry_initial: Duration::from_millis(SECRETS_RETRY_INITIAL_MS),
            retry_max: Duration::from_millis(SECRETS_RETRY_MAX_MS),
            apply_wait: Duration::from_millis(SECRETS_APPLY_WAIT_MS),
            now_ms: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            }),
        }
    }
}

#[derive(Default)]
struct ApplyState {
    desired: BTreeMap<String, String>,
    desired_generation: u64,
    applied_generation: u64,
    last_applied_at_ms: Option<u64>,
    stopped: bool,
}

struct Shared {
    state: Mutex<ApplyState>,
    wake: Condvar,
    apply_to_box: BoxSecretsApply,
    log: BoxSecretsLog,
    retry_initial: Duration,
    retry_max: Duration,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

pub struct BoxSecretsApplier {
    shared: Arc<Shared>,
    store_path: PathBuf,
    apply_wait: Duration,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl BoxSecretsApplier {
    pub fn new(options: BoxSecretsApplierOptions) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(ApplyState::default()),
            wake: Condvar::new(),
            apply_to_box: options.apply_to_box,
            log: options.log,
            retry_initial: options.retry_initial,
            retry_max: options.retry_max,
            now_ms: options.now_ms,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("sand-box-secrets-apply".into())
            .spawn(move || run_apply_loop(worker_shared))
            .expect("start box secrets apply worker");
        Self {
            shared,
            store_path: options.store_path,
            apply_wait: options.apply_wait,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn set_secrets(
        &self,
        secrets: BTreeMap<String, String>,
    ) -> Result<BoxSecretsStatus, BoxSecretsSetError> {
        if let Some(error) = validate_box_secrets(&secrets) {
            return Err(BoxSecretsSetError::Validation(error));
        }
        persist_secrets(&self.store_path, &secrets)?;
        let generation = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.desired = secrets;
            state.desired_generation = state.desired_generation.saturating_add(1);
            let generation = state.desired_generation;
            self.shared.wake.notify_all();
            generation
        };
        self.wait_for_generation(generation);
        Ok(self.get_status())
    }

    pub fn apply_persisted(&self) -> Result<BoxSecretsStatus, BoxSecretsSetError> {
        {
            let state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.desired_generation > 0 {
                return Ok(status_of(&state));
            }
        }
        let Some(secrets) = load_persisted(&self.store_path)? else {
            return Ok(self.get_status());
        };
        if let Some(error) = validate_box_secrets(&secrets) {
            (self.shared.log)(&format!(
                "box secrets: persisted secrets ignored after validation failure: {error}"
            ));
            return Ok(self.get_status());
        }
        let generation = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.desired_generation > 0 {
                return Ok(status_of(&state));
            }
            state.desired = secrets;
            state.desired_generation = 1;
            (self.shared.log)(&format!(
                "applying {} persisted box secret(s) at startup",
                state.desired.len()
            ));
            self.shared.wake.notify_all();
            1
        };
        self.wait_for_generation(generation);
        Ok(self.get_status())
    }

    pub fn get_status(&self) -> BoxSecretsStatus {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status_of(&state)
    }

    pub fn stop(&self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.stopped = true;
            self.shared.wake.notify_all();
        }
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }

    fn wait_for_generation(&self, generation: u64) {
        let deadline = Instant::now() + self.apply_wait;
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.applied_generation < generation && !state.stopped {
            let now = Instant::now();
            if now >= deadline {
                (self.shared.log)(
                    "box secrets: apply wait ended unconfirmed (deadline)",
                );
                break;
            }
            let timeout = deadline.saturating_duration_since(now);
            let (next, timed_out) = self
                .shared
                .wake
                .wait_timeout(state, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if timed_out.timed_out() {
                (self.shared.log)(
                    "box secrets: apply wait ended unconfirmed (deadline)",
                );
                break;
            }
        }
    }
}

impl Drop for BoxSecretsApplier {
    fn drop(&mut self) {
        self.stop();
    }
}

fn status_of(state: &ApplyState) -> BoxSecretsStatus {
    BoxSecretsStatus {
        keys: state.desired.keys().cloned().collect(),
        is_applied: state.desired_generation > 0
            && state.applied_generation == state.desired_generation,
        last_applied_at_ms: state.last_applied_at_ms,
    }
}

fn retry_delay(initial: Duration, max: Duration, attempt: u32) -> Duration {
    let multiplier = 1u32.checked_shl(attempt.saturating_sub(1).min(30)).unwrap_or(u32::MAX);
    initial.saturating_mul(multiplier).min(max)
}

fn run_apply_loop(shared: Arc<Shared>) {
    let mut attempt = 0u32;
    loop {
        let (generation, desired) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !state.stopped && state.applied_generation >= state.desired_generation {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.stopped {
                return;
            }
            (state.desired_generation, state.desired.clone())
        };

        let update = BoxEnvironmentUpdate {
            env: build_box_secrets_env(&desired),
            replace: true,
        };
        match (shared.apply_to_box)(&update) {
            Ok(()) => {
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.applied_generation = state.applied_generation.max(generation);
                state.last_applied_at_ms = Some((shared.now_ms)());
                attempt = 0;
                shared.wake.notify_all();
            }
            Err(BoxSecretsApplyError::Unsupported(error)) => {
                (shared.log)(&format!(
                    "box secrets: this box has no environment sync; giving up ({error})"
                ));
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.stopped = true;
                shared.wake.notify_all();
                return;
            }
            Err(BoxSecretsApplyError::Retryable(error)) => {
                attempt = attempt.saturating_add(1);
                (shared.log)(&format!(
                    "box secrets: apply failed (attempt {attempt}); retrying: {error}"
                ));
                let delay = retry_delay(shared.retry_initial, shared.retry_max, attempt);
                let mut state = shared
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let failed_generation = generation;
                let (next, _) = shared
                    .wake
                    .wait_timeout_while(state, delay, |state| {
                        !state.stopped && state.desired_generation == failed_generation
                    })
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state = next;
                if state.stopped {
                    return;
                }
                if state.desired_generation != failed_generation {
                    attempt = 0;
                }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedSecrets {
    version: u8,
    secrets: BTreeMap<String, String>,
}

fn persist_secrets(path: &Path, secrets: &BTreeMap<String, String>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = PathBuf::from(format!(
        "{}.{}.{}.tmp",
        path.to_string_lossy(),
        std::process::id(),
        Uuid::new_v4()
    ));
    let payload = serde_json::to_vec(&PersistedSecrets {
        version: 1,
        secrets: secrets.clone(),
    })
    .map_err(|error| io::Error::other(error.to_string()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    Ok(())
}

fn load_persisted(path: &Path) -> io::Result<Option<BTreeMap<String, String>>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let parsed = match serde_json::from_str::<PersistedSecrets>(&text) {
        Ok(parsed) if parsed.version == 1 => parsed,
        _ => return Ok(None),
    };
    Ok(Some(parsed.secrets))
}
