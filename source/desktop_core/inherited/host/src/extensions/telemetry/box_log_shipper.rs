use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::ports::telemetry::{
    SAND_BOX_BOOT_FAILURE_REASONS, SAND_BOX_BOOT_FAILURE_STAGES, SAND_BOX_BOOT_STAGES,
    SAND_COOKIE_PERSIST_OUTCOMES, SAND_COOKIE_PERSIST_PHASES, SAND_EGRESS_TUNNEL_OUTCOMES,
    SAND_EXEC_DAEMON_RESTART_CAUSES, SAND_HOST_BOOT_FETCH_OUTCOMES,
    SAND_HOST_BOOT_FETCH_REASONS, SAND_PROCESS_CRASH_BINARIES, SAND_PROCESS_CRASH_SIGNALS,
    SAND_SUPERVISOR_RESTART_CAUSES,
};

use super::box_log_ship_telemetry::BoxLogShipReport;
use super::lifecycle_telemetry::BoxInfrastructureEvent;

pub const DEFAULT_LOG_DIR: &str = "/tmp";
pub const SUPERVISOR_LOG_PATH: &str = "/tmp/sand-supervisor.log";
pub const BOX_LOG_SHIP_INTERVAL_MS: u64 = 2_000;
pub const BOX_LOG_SHIP_PROGRESS_INTERVAL_MS: u64 = 5 * 60_000;
pub const DEFAULT_MAX_BYTES_PER_READ: u64 = 256 * 1024;
pub const DEFAULT_MAX_LINES_PER_POLL: usize = 1_000;
pub const DEFAULT_MAX_LAG_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_LINE_BYTES: usize = 64 * 1024;
pub const OFFSETS_FILE_NAME: &str = "sand-log-shipper.offsets.json";
pub const LOG_SUFFIX: &str = ".log";
pub const BOX_TELEMETRY_SOURCE: &str = "sand-box-telemetry";
pub const NEWLINE: u8 = 10;
pub const DEFAULT_SUBDIR_PREFIX: &str = "sand-window-";
pub const DEFAULT_EXCLUDED_SOURCE_PREFIXES: [&str; 1] = ["sand-notify-injector"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverySettlement {
    Delivered,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxTelemetryRecord {
    Log { source: String, line: String },
    Infrastructure { event: BoxInfrastructureEvent },
}

pub type BoxLogBatchReporter =
    Arc<dyn Fn(&[BoxTelemetryRecord]) -> Vec<DeliverySettlement> + Send + Sync>;
pub type BoxLogShipReporter =
    Arc<dyn Fn(&BoxLogShipReport) -> DeliverySettlement + Send + Sync>;

#[derive(Debug, Clone)]
pub struct BoxLogShipperConfig {
    pub log_dir: PathBuf,
    pub skip_paths: Vec<PathBuf>,
    pub exclude_source_prefixes: Vec<String>,
    pub max_bytes_per_read_per_file: u64,
    pub max_lines_per_poll: usize,
    pub max_lag_bytes: u64,
    pub max_line_bytes: usize,
    pub offsets_path: Option<PathBuf>,
    pub poll_interval: Duration,
    pub progress_interval: Duration,
}

impl Default for BoxLogShipperConfig {
    fn default() -> Self {
        Self {
            log_dir: PathBuf::from(DEFAULT_LOG_DIR),
            skip_paths: Vec::new(),
            exclude_source_prefixes: DEFAULT_EXCLUDED_SOURCE_PREFIXES
                .into_iter()
                .map(str::to_string)
                .collect(),
            max_bytes_per_read_per_file: DEFAULT_MAX_BYTES_PER_READ,
            max_lines_per_poll: DEFAULT_MAX_LINES_PER_POLL,
            max_lag_bytes: DEFAULT_MAX_LAG_BYTES,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            offsets_path: None,
            poll_interval: Duration::from_millis(BOX_LOG_SHIP_INTERVAL_MS),
            progress_interval: Duration::from_millis(BOX_LOG_SHIP_PROGRESS_INTERVAL_MS),
        }
    }
}

#[derive(Debug)]
struct LogFile {
    path: PathBuf,
    source: String,
    size: u64,
}

#[derive(Debug)]
struct SaveFailure {
    error_class: String,
    failure_count: u64,
    retry_at: Instant,
}

#[derive(Debug, Default)]
struct ShipperState {
    offsets: BTreeMap<PathBuf, u64>,
    offsets_dirty: bool,
    last_progress_report_at: Option<Instant>,
    offset_save_failure: Option<SaveFailure>,
}

struct BoxLogShipperCore {
    config: BoxLogShipperConfig,
    offsets_path: PathBuf,
    state: Mutex<ShipperState>,
    report_batch: BoxLogBatchReporter,
    report_ship: BoxLogShipReporter,
}

pub struct BoxLogShipper {
    core: Arc<BoxLogShipperCore>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl BoxLogShipper {
    pub fn new(
        config: BoxLogShipperConfig,
        report_batch: BoxLogBatchReporter,
        report_ship: BoxLogShipReporter,
    ) -> Self {
        let offsets_path = config
            .offsets_path
            .clone()
            .unwrap_or_else(|| config.log_dir.join(OFFSETS_FILE_NAME));
        let state = ShipperState {
            offsets: load_offsets(&offsets_path),
            ..ShipperState::default()
        };
        Self {
            core: Arc::new(BoxLogShipperCore {
                config,
                offsets_path,
                state: Mutex::new(state),
                report_batch,
                report_ship,
            }),
            stop: Mutex::new(None),
            worker: Mutex::new(None),
        }
    }

    pub fn start(&self) -> io::Result<()> {
        self.poll_once()?;
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| io::Error::other("box log shipper worker mutex poisoned"))?;
        if worker.is_some() {
            return Ok(());
        }
        let (stop_tx, stop_rx) = mpsc::channel();
        *self
            .stop
            .lock()
            .map_err(|_| io::Error::other("box log shipper stop mutex poisoned"))? =
            Some(stop_tx);
        let core = Arc::clone(&self.core);
        let interval = core.config.poll_interval;
        *worker = Some(thread::spawn(move || {
            loop {
                match stop_rx.recv_timeout(interval) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let _ = poll_once_core(&core);
                    }
                }
            }
        }));
        Ok(())
    }

    pub fn poll_once(&self) -> io::Result<usize> {
        poll_once_core(&self.core)
    }

    pub fn checkpoint_offsets(&self) -> io::Result<()> {
        let mut state = self
            .core
            .state
            .lock()
            .map_err(|_| io::Error::other("box log shipper state mutex poisoned"))?;
        save_offsets(&self.core, &mut state, true)
    }

    pub fn offset_for(&self, path: &Path) -> Option<u64> {
        self.core
            .state
            .lock()
            .ok()
            .and_then(|state| state.offsets.get(path).copied())
    }

    pub fn stop_polling(&self) {
        if let Ok(mut stop) = self.stop.lock() {
            if let Some(sender) = stop.take() {
                let _ = sender.send(());
            }
        }
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for BoxLogShipper {
    fn drop(&mut self) {
        self.stop_polling();
        let _ = self.checkpoint_offsets();
    }
}

fn poll_once_core(core: &Arc<BoxLogShipperCore>) -> io::Result<usize> {
    let listing = list_log_files(&core.config);
    let mut state = core
        .state
        .lock()
        .map_err(|_| io::Error::other("box log shipper state mutex poisoned"))?;

    for file in &listing.files {
        if state
            .offsets
            .get(&file.path)
            .is_some_and(|offset| file.size < *offset)
        {
            set_offset(&mut state, file.path.clone(), 0);
        }
    }

    let mut budget = core.config.max_lines_per_poll;
    let mut processed = 0usize;
    for file in &listing.files {
        if budget == 0 {
            break;
        }
        match pump_file(core, &mut state, file, budget) {
            Ok(count) => {
                processed = processed.saturating_add(count);
                budget = budget.saturating_sub(count);
            }
            Err(_) => {
                // Frozen shipper treats one unreadable log as a local failure and
                // continues shipping the remaining files.
            }
        }
    }

    if state.offsets_dirty {
        save_offsets(core, &mut state, false)?;
    }
    if listing.complete {
        maybe_report_progress(core, &mut state, &listing.files);
    }
    maybe_retry_offset_save_failure(core, &mut state);
    Ok(processed)
}

struct LogListing {
    files: Vec<LogFile>,
    complete: bool,
}

fn list_log_files(config: &BoxLogShipperConfig) -> LogListing {
    let mut files = Vec::new();
    let mut complete = true;
    let entries = match fs::read_dir(&config.log_dir) {
        Ok(entries) => entries,
        Err(_) => {
            return LogListing {
                files,
                complete: false,
            };
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            complete = false;
            continue;
        };
        if file_type.is_file() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !name.ends_with(LOG_SUFFIX) {
                continue;
            }
            let source = to_source_name(&name);
            if is_skipped(config, &path, source) {
                continue;
            }
            match entry.metadata() {
                Ok(metadata) if metadata.is_file() => files.push(LogFile {
                    path,
                    source: source.to_string(),
                    size: metadata.len(),
                }),
                Ok(_) => {}
                Err(_) => complete = false,
            }
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        let Some(dir_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !dir_name.starts_with(DEFAULT_SUBDIR_PREFIX) {
            continue;
        }
        let sub_entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for sub in sub_entries.flatten() {
            let sub_path = sub.path();
            let Ok(file_type) = sub.file_type() else {
                complete = false;
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = sub.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !name.ends_with(LOG_SUFFIX) {
                continue;
            }
            let leaf = to_source_name(&name);
            if is_skipped(config, &sub_path, leaf) {
                continue;
            }
            match sub.metadata() {
                Ok(metadata) if metadata.is_file() => files.push(LogFile {
                    path: sub_path,
                    source: format!("{dir_name}/{leaf}"),
                    size: metadata.len(),
                }),
                Ok(_) => {}
                Err(_) => complete = false,
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    LogListing { files, complete }
}

fn is_skipped(config: &BoxLogShipperConfig, path: &Path, source_leaf: &str) -> bool {
    config.skip_paths.iter().any(|skip| skip == path)
        || config
            .exclude_source_prefixes
            .iter()
            .any(|prefix| source_leaf.starts_with(prefix))
}

fn pump_file(
    core: &BoxLogShipperCore,
    state: &mut ShipperState,
    file: &LogFile,
    budget: usize,
) -> io::Result<usize> {
    let mut from = state.offsets.get(&file.path).copied().unwrap_or(0);
    if file.size <= from {
        return Ok(0);
    }

    let mut processed = 0usize;
    let mut candidate_end_offset = from;
    let mut records = Vec::new();
    if file.size.saturating_sub(from) > core.config.max_lag_bytes {
        let jumped = file
            .size
            .saturating_sub(core.config.max_bytes_per_read_per_file);
        records.push(BoxTelemetryRecord::Log {
            source: file.source.clone(),
            line: format!(
                "[sand-log-shipper] skipped {} bytes (too far behind)",
                jumped.saturating_sub(from)
            ),
        });
        processed = processed.saturating_add(1);
        from = jumped;
        candidate_end_offset = jumped;
    }

    let length = file
        .size
        .saturating_sub(from)
        .min(core.config.max_bytes_per_read_per_file);
    if length == 0 {
        settle_delivery_window(core, state, &file.path, candidate_end_offset, records);
        return Ok(processed);
    }

    let mut handle = File::open(&file.path)?;
    handle.seek(SeekFrom::Start(from))?;
    let mut buffer = vec![0u8; length as usize];
    let bytes_read = handle.read(&mut buffer)?;
    buffer.truncate(bytes_read);
    if bytes_read == 0 {
        settle_delivery_window(core, state, &file.path, candidate_end_offset, records);
        return Ok(processed);
    }

    let last_newline = buffer.iter().rposition(|byte| *byte == NEWLINE);
    if last_newline.is_none() {
        if from.saturating_add(bytes_read as u64) < file.size && processed < budget {
            let line = lossy_line(&buffer, core.config.max_line_bytes);
            if let Some(record) = to_telemetry_record(&file.source, &line) {
                records.push(record);
            }
            candidate_end_offset = from.saturating_add(bytes_read as u64);
            processed = processed.saturating_add(1);
        }
        settle_delivery_window(core, state, &file.path, candidate_end_offset, records);
        return Ok(processed);
    }

    let last_newline = last_newline.unwrap_or_default();
    let mut cursor = 0usize;
    while processed < budget {
        let Some(relative) = buffer[cursor..].iter().position(|byte| *byte == NEWLINE) else {
            break;
        };
        let newline = cursor + relative;
        if newline > last_newline {
            break;
        }
        let mut line_bytes = &buffer[cursor..newline];
        cursor = newline.saturating_add(1);
        if line_bytes.last() == Some(&b'\r') {
            line_bytes = &line_bytes[..line_bytes.len().saturating_sub(1)];
        }
        if line_bytes.is_empty() {
            continue;
        }
        let line = lossy_line(line_bytes, core.config.max_line_bytes);
        if let Some(record) = to_telemetry_record(&file.source, &line) {
            records.push(record);
        }
        processed = processed.saturating_add(1);
    }
    candidate_end_offset = from.saturating_add(cursor as u64);
    settle_delivery_window(core, state, &file.path, candidate_end_offset, records);
    Ok(processed)
}

fn lossy_line(bytes: &[u8], max_line_bytes: usize) -> String {
    let end = bytes.len().min(max_line_bytes);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn settle_delivery_window(
    core: &BoxLogShipperCore,
    state: &mut ShipperState,
    path: &Path,
    candidate_end_offset: u64,
    records: Vec<BoxTelemetryRecord>,
) {
    if records.is_empty() {
        set_offset(state, path.to_path_buf(), candidate_end_offset);
        return;
    }
    let settlements = (core.report_batch)(&records);
    let all_delivered = settlements.len() == records.len()
        && settlements
            .iter()
            .all(|settlement| *settlement == DeliverySettlement::Delivered);
    if all_delivered {
        set_offset(state, path.to_path_buf(), candidate_end_offset);
    }
}

fn to_telemetry_record(source: &str, line: &str) -> Option<BoxTelemetryRecord> {
    if source != BOX_TELEMETRY_SOURCE {
        return Some(BoxTelemetryRecord::Log {
            source: source.to_string(),
            line: line.to_string(),
        });
    }
    let value: Value = serde_json::from_str(line).ok()?;
    parse_box_infrastructure_event(&value)
        .map(|event| BoxTelemetryRecord::Infrastructure { event })
}

pub fn parse_box_infrastructure_event(value: &Value) -> Option<BoxInfrastructureEvent> {
    let object = value.as_object()?;
    let kind = object.get("kind")?.as_str()?;
    match kind {
        "boot_stage" => Some(BoxInfrastructureEvent::BootStage {
            stage: enum_string(object, "stage", SAND_BOX_BOOT_STAGES)?,
            duration_ms: nonnegative_u64(object, "durationMs")?,
        }),
        "boot_failure" => Some(BoxInfrastructureEvent::BootFailure {
            stage: enum_string(object, "stage", SAND_BOX_BOOT_FAILURE_STAGES)?,
            reason: enum_string(object, "reason", SAND_BOX_BOOT_FAILURE_REASONS)?,
            duration_ms: nonnegative_u64(object, "durationMs")?,
        }),
        "egress_tunnel" => Some(BoxInfrastructureEvent::EgressTunnel {
            outcome: enum_string(object, "outcome", SAND_EGRESS_TUNNEL_OUTCOMES)?,
            attempt: positive_u64(object, "attempt")?,
            exit_status: optional_i64(object, "exitStatus")?,
            runtime_s: optional_u64(object, "runtimeS")?,
        }),
        "host_boot_fetch" => {
            let from_version = optional_string(object, "fromVersion")?;
            let to_version = optional_string(object, "toVersion")?;
            if from_version.as_deref().is_some_and(|value| !is_git_hex(value))
                || to_version.as_deref().is_some_and(|value| !is_git_hex(value))
            {
                return None;
            }
            Some(BoxInfrastructureEvent::HostBootFetch {
                outcome: enum_string(object, "outcome", SAND_HOST_BOOT_FETCH_OUTCOMES)?,
                reason: Some(enum_string(
                    object,
                    "reason",
                    SAND_HOST_BOOT_FETCH_REASONS,
                )?),
                duration_ms: nonnegative_u64(object, "durationMs")?,
                swap_ms: optional_u64(object, "swapMs")?,
                from_version,
                to_version,
            })
        }
        "exec_daemon_restart" => Some(BoxInfrastructureEvent::ExecDaemonRestart {
            restart_attempt: positive_u64(object, "restartAttempt")?,
            runtime_s: nonnegative_u64(object, "runtimeS")?,
            cause: enum_string(object, "cause", SAND_EXEC_DAEMON_RESTART_CAUSES)?,
            exit_status: required_i64(object, "exitStatus")?,
        }),
        "supervisor_restart" => Some(BoxInfrastructureEvent::SupervisorRestart {
            restart_attempt: positive_u64(object, "restartAttempt")?,
            runtime_s: nonnegative_u64(object, "runtimeS")?,
            cause: enum_string(object, "cause", SAND_SUPERVISOR_RESTART_CAUSES)?,
            exit_status: required_i64(object, "exitStatus")?,
        }),
        "cookie_persist" => Some(BoxInfrastructureEvent::CookiePersist {
            phase: enum_string(object, "phase", SAND_COOKIE_PERSIST_PHASES)?,
            outcome: enum_string(object, "outcome", SAND_COOKIE_PERSIST_OUTCOMES)?,
            seed_cookies: nonnegative_u64(object, "seedCookies")?,
            injected: optional_u64(object, "injected")?,
            missing_after: optional_u64(object, "missingAfter")?,
            attempts: optional_u64(object, "attempts")?,
        }),
        "process_crash" => {
            let count = positive_u64(object, "count")?;
            if count > 10_000 {
                return None;
            }
            Some(BoxInfrastructureEvent::ProcessCrash {
                binary: enum_string(object, "binary", SAND_PROCESS_CRASH_BINARIES)?,
                signal: enum_string(object, "signal", SAND_PROCESS_CRASH_SIGNALS)?,
                count,
            })
        }
        _ => None,
    }
}

fn enum_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Option<String> {
    let value = object.get(key)?.as_str()?;
    allowed
        .contains(&value)
        .then(|| value.to_string())
}

fn nonnegative_u64(object: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key)?.as_u64()
}

fn positive_u64(object: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    nonnegative_u64(object, key).filter(|value| *value > 0)
}

fn required_i64(object: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key)?.as_i64()
}

fn optional_u64(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<Option<u64>> {
    match object.get(key) {
        None => Some(None),
        Some(value) => value.as_u64().map(Some),
    }
}

fn optional_i64(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<Option<i64>> {
    match object.get(key) {
        None => Some(None),
        Some(value) => value.as_i64().map(Some),
    }
}

fn optional_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<Option<String>> {
    match object.get(key) {
        None => Some(None),
        Some(value) => value.as_str().map(|value| Some(value.to_string())),
    }
}

fn is_git_hex(value: &str) -> bool {
    (7..=40).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn set_offset(state: &mut ShipperState, path: PathBuf, offset: u64) {
    if state.offsets.get(&path).copied() == Some(offset) {
        return;
    }
    state.offsets.insert(path, offset);
    state.offsets_dirty = true;
}

fn load_offsets(path: &Path) -> BTreeMap<PathBuf, u64> {
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return BTreeMap::new();
    };
    let Some(object) = value.as_object() else {
        return BTreeMap::new();
    };
    let mut offsets = BTreeMap::new();
    for (path, value) in object {
        let Some(number) = value.as_f64().filter(|number| *number >= 0.0) else {
            return BTreeMap::new();
        };
        offsets.insert(PathBuf::from(path), to_nonnegative_safe_integer(number));
    }
    offsets
}

fn save_offsets(
    core: &BoxLogShipperCore,
    state: &mut ShipperState,
    force: bool,
) -> io::Result<()> {
    if !force && !state.offsets_dirty {
        return Ok(());
    }
    let record = state
        .offsets
        .iter()
        .map(|(path, offset)| (path.to_string_lossy().into_owned(), *offset))
        .collect::<BTreeMap<_, _>>();
    let data = serde_json::to_vec(&record).map_err(io::Error::other)?;
    match write_file_atomic(&core.offsets_path, &data) {
        Ok(()) => {
            state.offsets_dirty = false;
            if let Some(failure) = state.offset_save_failure.take() {
                let _ = (core.report_ship)(&BoxLogShipReport::SaveRecovered {
                    error_class: failure.error_class,
                    failure_count: to_report_i64(failure.failure_count),
                });
            }
            Ok(())
        }
        Err(error) => {
            state.offsets_dirty = true;
            let error_class = classify_offset_save_error(&error).to_string();
            if let Some(failure) = state.offset_save_failure.as_mut() {
                failure.error_class = error_class;
                failure.failure_count = failure.failure_count.saturating_add(1);
            } else {
                let failure = SaveFailure {
                    error_class,
                    failure_count: 1,
                    retry_at: Instant::now() + core.config.progress_interval,
                };
                let _ = (core.report_ship)(&BoxLogShipReport::SaveFailed {
                    error_class: failure.error_class.clone(),
                    failure_count: to_report_i64(failure.failure_count),
                });
                state.offset_save_failure = Some(failure);
            }
            Ok(())
        }
    }
}

fn write_file_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(OFFSETS_FILE_NAME);
    let temp = path.with_file_name(format!(".{file_name}.part-{}-{suffix}", std::process::id()));
    {
        let mut file = File::create(&temp)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn maybe_report_progress(
    core: &BoxLogShipperCore,
    state: &mut ShipperState,
    files: &[LogFile],
) {
    let now = Instant::now();
    if state
        .last_progress_report_at
        .is_some_and(|last| now.duration_since(last) < core.config.progress_interval)
    {
        return;
    }
    let mut bytes_written = 0u64;
    let mut bytes_delivered = 0u64;
    for file in files {
        bytes_written = saturating_add(bytes_written, file.size);
        bytes_delivered = saturating_add(
            bytes_delivered,
            file.size
                .min(state.offsets.get(&file.path).copied().unwrap_or(0)),
        );
    }
    state.last_progress_report_at = Some(now);
    let _ = (core.report_ship)(&BoxLogShipReport::Progress {
        bytes_written: to_report_i64(bytes_written),
        bytes_delivered: to_report_i64(bytes_delivered),
        // The Rust JSONL transport settles synchronously; therefore no
        // delivery window survives a poll boundary.
        pending_window_count: 0,
        oldest_pending_window_age_ms: 0,
    });
}

fn maybe_retry_offset_save_failure(core: &BoxLogShipperCore, state: &mut ShipperState) {
    let Some(failure) = state.offset_save_failure.as_mut() else {
        return;
    };
    let now = Instant::now();
    if now < failure.retry_at {
        return;
    }
    let settlement = (core.report_ship)(&BoxLogShipReport::SaveFailed {
        error_class: failure.error_class.clone(),
        failure_count: to_report_i64(failure.failure_count),
    });
    failure.retry_at = now + core.config.progress_interval;
    if settlement == DeliverySettlement::Delivered {
        // The failure remains active until a subsequent offset write succeeds;
        // only the diagnostic retry has settled.
    }
}

fn to_report_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn classify_offset_save_error(error: &io::Error) -> &'static str {
    match error.raw_os_error() {
        Some(28 | 122) => "no_space",
        Some(13 | 1) => "permission_denied",
        Some(30) => "read_only",
        Some(2) => "missing_parent",
        None => "unknown",
        Some(_) => "io",
    }
}

pub fn is_box_log_shipping_enabled(host_in_box: Option<&str>, disabled: Option<&str>) -> bool {
    if host_in_box != Some("1") {
        return false;
    }
    !matches!(
        disabled.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes")
    )
}

pub fn to_source_name(file_name: &str) -> &str {
    file_name.strip_suffix(LOG_SUFFIX).unwrap_or(file_name)
}

pub fn classify_offset_save_errno(errno: Option<&str>) -> &'static str {
    match errno {
        Some("ENOSPC" | "EDQUOT") => "no_space",
        Some("EACCES" | "EPERM") => "permission_denied",
        Some("EROFS") => "read_only",
        Some("ENOENT") => "missing_parent",
        None => "unknown",
        Some(_) => "io",
    }
}

pub fn saturating_add(total: u64, value: u64) -> u64 {
    total.saturating_add(value)
}

pub fn to_nonnegative_safe_integer(value: f64) -> u64 {
    const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
    if !value.is_finite() {
        return MAX_SAFE_INTEGER;
    }
    value.round().clamp(0.0, MAX_SAFE_INTEGER as f64) as u64
}
