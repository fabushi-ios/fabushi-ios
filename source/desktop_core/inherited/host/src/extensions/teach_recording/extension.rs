use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::host_paths::get_sand_root_dir;

pub const CAP_SLACK_MS: u64 = 2_000;
pub const TEACH_QUEUE_KEY_FILENAME: &str = "teach-queue-key.json";
pub const TEACH_RECORDING_EXTENSION_ID: &str = "teach-recording";
pub const TEACH_RECORDING_DEPENDENCIES: &[&str] = &[
    "experiments",
    "forever-box",
    "managed-setup",
    "telemetry",
    "transcript",
];

pub fn parse_teach_queue_key(raw: &str) -> Option<[u8; 32]> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let key_hex = value.get("keyHex").and_then(Value::as_str)?;
    if key_hex.len() != 64 || !key_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0_u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&key_hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn key_hex(key: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in key {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn write_queue_key(path: &Path, key: &[u8; 32]) -> io::Result<()> {
    let payload = json!({
        "version": 1,
        "keyHex": key_hex(key),
    })
    .to_string();

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(payload.as_bytes())?;
    file.flush()
}

pub fn load_teach_queue_key_with(
    sand_root: &Path,
    fill_random: impl FnOnce(&mut [u8]) -> io::Result<()>,
) -> io::Result<[u8; 32]> {
    let path = sand_root.join(TEACH_QUEUE_KEY_FILENAME);
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Some(existing) = parse_teach_queue_key(&raw) {
            return Ok(existing);
        }
    }

    let mut key = [0_u8; 32];
    fill_random(&mut key)?;
    fs::create_dir_all(sand_root)?;
    write_queue_key(&path, &key)?;
    Ok(key)
}

pub fn load_teach_queue_key(sand_root: Option<&Path>) -> io::Result<[u8; 32]> {
    let owned_root: PathBuf;
    let root = match sand_root {
        Some(root) => root,
        None => {
            owned_root = get_sand_root_dir();
            &owned_root
        }
    };
    load_teach_queue_key_with(root, |buffer| {
        getrandom::fill(buffer)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error.to_string()))
    })
}

#[derive(Clone)]
pub struct TeachRecordingServiceDeps {
    pub is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub cap_delay_ms: u64,
    pub send_learning_prompt:
        Arc<dyn Fn(&str, &str, &str, Option<&str>) -> Result<(), String> + Send + Sync>,
    pub list_agent_ids: Arc<dyn Fn() -> Result<Vec<String>, String> + Send + Sync>,
    pub queue_signature_key: Arc<dyn Fn() -> io::Result<[u8; 32]> + Send + Sync>,
    pub ensure_learning_workflow: Arc<dyn Fn() -> Result<bool, String> + Send + Sync>,
    pub track_recording_started: Arc<dyn Fn(Value) + Send + Sync>,
    pub track_recording_stopped: Arc<dyn Fn(Value) + Send + Sync>,
    pub report_cap_stop_failed: Arc<dyn Fn(Value) + Send + Sync>,
    pub report_start_failed: Arc<dyn Fn(Value) + Send + Sync>,
}

pub trait TeachRecordingService: Send + Sync + 'static {
    type Api: Clone + Send + Sync + 'static;

    fn recover_pending(&self) -> Result<(), String>;
    fn api(&self) -> Self::Api;
    fn dispose(&self);
}

pub trait TeachRecordingServiceFactory: Send + Sync {
    type Service: TeachRecordingService;

    fn create(&self, deps: TeachRecordingServiceDeps) -> Self::Service;
}

pub struct TeachRecordingExtension<S: TeachRecordingService> {
    service: Arc<S>,
}

impl<S: TeachRecordingService> TeachRecordingExtension<S> {
    pub fn api(&self) -> S::Api {
        self.service.api()
    }

    pub fn service(&self) -> &Arc<S> {
        &self.service
    }
}

impl<S: TeachRecordingService> Drop for TeachRecordingExtension<S> {
    fn drop(&mut self) {
        self.service.dispose();
    }
}

pub fn create_teach_recording_extension<F>(
    factory: &F,
    deps: TeachRecordingServiceDeps,
) -> TeachRecordingExtension<F::Service>
where
    F: TeachRecordingServiceFactory,
{
    TeachRecordingExtension {
        service: Arc::new(factory.create(deps)),
    }
}

pub fn recover_teach_recording_extension<S>(
    extension: &TeachRecordingExtension<S>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
)
where
    S: TeachRecordingService,
{
    if extension.service.recover_pending().is_err() {
        log("teach-recording: pending delivery recovery failed");
    }
}

pub fn start_teach_recording_extension<F>(
    factory: &F,
    deps: TeachRecordingServiceDeps,
    log: Arc<dyn Fn(&str) + Send + Sync>,
) -> TeachRecordingExtension<F::Service>
where
    F: TeachRecordingServiceFactory,
{
    let extension = create_teach_recording_extension(factory, deps);
    recover_teach_recording_extension(&extension, log);
    extension
}
