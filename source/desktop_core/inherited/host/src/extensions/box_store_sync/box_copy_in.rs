use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;

use super::agent_store_sand_files::{AgentStoreClientDependencies, normalize_rel_path};
use super::box_object_store::{
    AgentStoreObjectStoreProvider, BoxObjectStore, BoxObjectStoreProvider,
    BoxObjectStoreProviderDependencies, resolve_box_object_store_provider,
};
use super::box_store_hydration::{
    BOX_STORE_HYDRATION_HANDOFF_FILE_NAME, HydrationEvidence,
    INCOMPLETE_LEGACY_HYDRATE_REASON, is_box_store_fully_hydrated,
    remove_hydration_handoff_marker, write_hydration_handoff_marker,
};
use super::box_store_download::{
    BoxStoreDownloadOptions, BoxStoreDownloadProgress, BoxStoreDownloadTrace, download_manifest,
};
use super::box_store_manifest::{
    count_store_db_manifest_entries, get_store_db_manifest_agent_ids,
};
use super::box_store_manifest_format::{
    BOX_STORE_MANIFEST_REL_PATH, BOX_STORE_MANIFEST_VERSION, BoxStoreManifestEntry,
    parse_box_store_manifest,
};
use crate::extensions::auth::auth_service::HostAuthService;
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::telemetry::host_telemetry_service::{
    HostStructuredLogTelemetry, HostTelemetryService,
};
pub use crate::extensions::telemetry::box_copy_in_telemetry::BOX_COPY_IN_EVENT as BOX_COPY_IN_TELEMETRY_EVENT;
use crate::host_paths::{SAND_BOX_HOME_DIR, get_sand_root_dir};
use crate::ports::telemetry::resolve_sand_box_identity_tags;
use crate::r#box::box_store_backend_policy::{
    BoxStoreBackendKind, is_box_store_copy_in_enabled, resolve_box_store_backend_policy,
};

pub const BOX_COPY_IN_ARG: &str = "--box-copy-in";
pub const BOX_COPY_IN_EXIT_NOOP: i32 = 0;
pub const BOX_COPY_IN_EXIT_FAILED: i32 = 1;
pub const BOX_COPY_IN_EXIT_HYDRATED: i32 = 10;
pub const SAND_BOX_COPY_IN_STATUS_PATH: &str = "/tmp/sand-copy-in-status.json";
pub const COPY_IN_STATUS_THROTTLE_MS: u64 = 750;
pub const COPY_IN_HYDRATE_ATTEMPTS: usize = 8;
pub const COPY_IN_STUCK_THRESHOLD_MS: u64 = 5 * 60_000;
const COPY_IN_RETRY_BASE_MS: u64 = 1_000;
const COPY_IN_RETRY_MAX_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyInOutcome {
    Hydrated,
    Noop,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyInResult {
    pub outcome: CopyInOutcome,
    pub reason: String,
    pub manifest_entries: usize,
    pub store_db_entries: usize,
    pub restored_store_db_entries: Option<usize>,
    pub files: usize,
    pub bytes: u64,
    pub verified: usize,
    pub failures: Vec<String>,
    pub hydrate_source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyInMeteredOutcome {
    Hydrated,
    Empty,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyInProgress {
    pub total: usize,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyInTraceStatus {
    pub copy_stage: Option<String>,
    pub file_entries: Option<usize>,
    pub symlink_entries: Option<usize>,
    pub symlinks_started: Option<usize>,
    pub symlinks_completed: Option<usize>,
    pub symlinks_in_flight: Option<usize>,
    pub active_symlink_steps: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyInWatchdogEvent {
    pub level: &'static str,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
pub struct CopyInRuntimeOptions {
    pub download_owner: Option<(u32, u32)>,
    pub download_concurrency: Option<usize>,
    pub on_progress: Option<Arc<dyn Fn(CopyInProgress) + Send + Sync>>,
    pub on_trace: Option<Arc<dyn Fn(CopyInTraceStatus) + Send + Sync>>,
}

#[derive(Debug, Clone, Default)]
struct CopyInWatchState {
    progress: CopyInProgress,
    progress_events: u64,
    trace: CopyInTraceStatus,
}

pub fn resolve_copy_in_concurrency(raw: Option<&str>) -> Option<usize> {
    parse_positive_usize(raw)
}

pub fn resolve_copy_in_attempts(raw: Option<&str>) -> usize {
    parse_positive_usize(raw).unwrap_or(COPY_IN_HYDRATE_ATTEMPTS)
}

pub fn resolve_copy_in_stuck_threshold_ms(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(COPY_IN_STUCK_THRESHOLD_MS)
}

fn parse_positive_usize(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub fn classify_copy_in_metered_outcome(result: &CopyInResult) -> CopyInMeteredOutcome {
    match result.outcome {
        CopyInOutcome::Hydrated => CopyInMeteredOutcome::Hydrated,
        CopyInOutcome::Noop => CopyInMeteredOutcome::Empty,
        CopyInOutcome::Failed
            if result.reason.starts_with("partial hydrate")
                || result.reason.starts_with(INCOMPLETE_LEGACY_HYDRATE_REASON) =>
        {
            CopyInMeteredOutcome::Partial
        }
        CopyInOutcome::Failed => CopyInMeteredOutcome::Failed,
    }
}

pub fn outcome_to_exit_code(outcome: CopyInOutcome) -> i32 {
    match outcome {
        CopyInOutcome::Hydrated => BOX_COPY_IN_EXIT_HYDRATED,
        CopyInOutcome::Noop => BOX_COPY_IN_EXIT_NOOP,
        CopyInOutcome::Failed => BOX_COPY_IN_EXIT_FAILED,
    }
}

pub fn is_transient_copy_in_failure(result: &CopyInResult) -> bool {
    if result.outcome != CopyInOutcome::Failed {
        return false;
    }
    if result.reason.starts_with("lock-held")
        || result.reason.contains("primary V2 store.db coverage is below")
        || result.reason.contains("no store.db identities to validate V2")
        || result.reason.starts_with("legacy source manifest is empty")
    {
        return false;
    }
    if result.reason.starts_with("partial hydrate")
        || result.reason.starts_with(INCOMPLETE_LEGACY_HYDRATE_REASON)
    {
        return true;
    }
    let text = result
        .failures
        .first()
        .map(String::as_str)
        .unwrap_or(&result.reason)
        .to_ascii_lowercase();
    if text.contains("no inference credential")
        || text.contains("401")
        || text.contains("403")
        || text.contains("forbidden")
        || text.contains("access denied")
        || text.contains("unauthorized")
        || text.contains("unauthenticated")
        || text.contains("not authorized")
        || text.contains("access is not enabled")
    {
        return false;
    }
    true
}

pub fn redact_copy_in_error_for_telemetry(raw: &str) -> String {
    raw.split_whitespace()
        .map(|token| {
            if token.starts_with("http://") || token.starts_with("https://") {
                "<url>"
            } else if token.to_ascii_lowercase().contains("token=")
                || token.to_ascii_lowercase().contains("authorization:")
            {
                "<redacted>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}

pub fn empty(reason: impl Into<String>) -> CopyInResult {
    CopyInResult {
        outcome: CopyInOutcome::Noop,
        reason: reason.into(),
        manifest_entries: 0,
        store_db_entries: 0,
        restored_store_db_entries: None,
        files: 0,
        bytes: 0,
        verified: 0,
        failures: Vec::new(),
        hydrate_source: None,
    }
}

#[derive(Debug)]
struct CopyInStoreLock {
    file: File,
}

impl Drop for CopyInStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn try_acquire_copy_in_lock(path: &Path) -> io::Result<Option<CopyInStoreLock>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(CopyInStoreLock { file })),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn build_copy_in_status_from_result(result: &CopyInResult) -> serde_json::Value {
    serde_json::json!({
        "phase": if result.outcome == CopyInOutcome::Failed { "failed" } else { "done" },
        "restored": result.files,
        "total": result.manifest_entries,
        "bytes": result.bytes,
        "outcome": match classify_copy_in_metered_outcome(result) {
            CopyInMeteredOutcome::Hydrated => "hydrated",
            CopyInMeteredOutcome::Empty => "empty",
            CopyInMeteredOutcome::Partial => "partial",
            CopyInMeteredOutcome::Failed => "failed",
        },
        "storeDbEntries": result.store_db_entries,
    })
}

pub fn write_copy_in_status_atomic(path: &Path, status: &serde_json::Value) -> io::Result<()> {
    let temp_path = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temp_path, serde_json::to_vec(status).map_err(io::Error::other)?)?;
    fs::rename(temp_path, path)
}

pub fn write_copy_in_status_throttled(
    path: &Path,
    status: &serde_json::Value,
    now_ms: u64,
    last_write_ms: &mut Option<u64>,
    force: bool,
) -> io::Result<bool> {
    if !force
        && last_write_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < COPY_IN_STATUS_THROTTLE_MS)
    {
        return Ok(false);
    }
    write_copy_in_status_atomic(path, status)?;
    *last_write_ms = Some(now_ms);
    Ok(true)
}

fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

pub fn build_copy_in_watchdog_event(
    progressed_since_last_tick: bool,
    progress: CopyInProgress,
    elapsed_ms: u64,
    threshold_ms: u64,
    trace: Option<&CopyInTraceStatus>,
) -> CopyInWatchdogEvent {
    let mut metadata = BTreeMap::from([
        (
            "outcome".into(),
            if progressed_since_last_tick { "slow" } else { "stuck" }.into(),
        ),
        (
            "reason".into(),
            if progressed_since_last_tick {
                "moving-data-in-slow-but-advancing"
            } else {
                "moving-data-in-exceeded-threshold"
            }
            .into(),
        ),
        ("manifest_entries".into(), progress.total.to_string()),
        ("files".into(), progress.files.to_string()),
        ("bytes".into(), progress.bytes.to_string()),
        ("duration_ms".into(), elapsed_ms.to_string()),
        ("threshold_ms".into(), threshold_ms.to_string()),
    ]);
    if let Some(trace) = trace {
        if let Some(stage) = trace.copy_stage.as_ref() {
            metadata.insert("stage".into(), stage.clone());
        }
        if let Some(value) = trace.file_entries {
            metadata.insert("file_entries".into(), value.to_string());
        }
        if let Some(value) = trace.symlink_entries {
            metadata.insert("symlink_entries".into(), value.to_string());
        }
        if let Some(value) = trace.symlinks_started {
            metadata.insert("symlinks_started".into(), value.to_string());
        }
        if let Some(value) = trace.symlinks_completed {
            metadata.insert("symlinks_completed".into(), value.to_string());
        }
        if let Some(value) = trace.symlinks_in_flight {
            metadata.insert("symlinks_in_flight".into(), value.to_string());
        }
        let active = trace
            .active_symlink_steps
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(step, count)| format!("{step}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        if !active.is_empty() {
            metadata.insert("active_symlink_steps".into(), active);
        }
    }
    CopyInWatchdogEvent {
        level: if progressed_since_last_tick { "info" } else { "warn" },
        metadata,
    }
}

pub fn bucket_copy_in_reason(result: &CopyInResult) -> &'static str {
    if result.outcome == CopyInOutcome::Hydrated {
        return "hydrated";
    }
    if result.outcome == CopyInOutcome::Noop {
        return "noop";
    }
    if result.reason.starts_with(INCOMPLETE_LEGACY_HYDRATE_REASON) {
        return INCOMPLETE_LEGACY_HYDRATE_REASON;
    }
    if result.reason.starts_with("partial hydrate") {
        return "partial-hydrate";
    }
    if result.reason.starts_with("store manifest unreadable")
        || result.reason.starts_with("legacy store manifest unreadable")
    {
        return "manifest-unreadable";
    }
    if result.reason.starts_with("download threw") {
        return "download-error";
    }
    "error"
}

pub fn classify_copy_in_failure(result: &CopyInResult) -> &'static str {
    if result.outcome != CopyInOutcome::Failed {
        return "";
    }
    if result.reason.starts_with("lock-held") {
        return "lock-held";
    }
    let text = result
        .failures
        .first()
        .map(String::as_str)
        .unwrap_or(&result.reason)
        .to_ascii_lowercase();
    if text.contains("no inference credential") {
        return "no-credential";
    }
    if text.contains("401")
        || text.contains("403")
        || text.contains("forbidden")
        || text.contains("access denied")
        || text.contains("unauthorized")
        || text.contains("unauthenticated")
        || text.contains("notloggedin")
        || text.contains("does not have access")
        || text.contains("not authorized")
        || text.contains("access is not enabled")
    {
        return "auth";
    }
    if text.contains("timeout") || text.contains("timed out") || text.contains("etimedout")
        || text.contains("deadline")
    {
        return "timeout";
    }
    if text.contains("enotfound")
        || text.contains("econnrefused")
        || text.contains("econnreset")
        || text.contains("fetch failed")
        || text.contains("network")
        || text.contains("socket")
        || text.contains("getaddrinfo")
        || text.contains("dns")
    {
        return "network";
    }
    if text.contains("nosuchkey")
        || text.contains("presign")
        || text.contains("amazonaws")
        || text.contains("s3")
        || text.contains("bucket")
    {
        return "s3";
    }
    "unknown"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyInTelemetryEvent {
    pub level: &'static str,
    pub metadata: BTreeMap<String, String>,
}

pub fn build_copy_in_started_telemetry() -> CopyInTelemetryEvent {
    CopyInTelemetryEvent {
        level: "info",
        metadata: BTreeMap::from([
            ("outcome".into(), "started".into()),
            ("reason".into(), "hydrate-started".into()),
            ("duration_ms".into(), "0".into()),
        ]),
    }
}

pub fn build_copy_in_result_telemetry(
    result: &CopyInResult,
    duration_ms: u64,
) -> CopyInTelemetryEvent {
    let error_summary = if result.outcome == CopyInOutcome::Failed {
        redact_copy_in_error_for_telemetry(
            result
                .failures
                .first()
                .map(String::as_str)
                .unwrap_or(&result.reason),
        )
    } else {
        String::new()
    };
    let mut metadata = BTreeMap::from([
        ("outcome".into(), format!("{:?}", result.outcome).to_ascii_lowercase()),
        ("reason".into(), bucket_copy_in_reason(result).into()),
        ("error_class".into(), classify_copy_in_failure(result).into()),
        ("error_summary".into(), error_summary),
        ("manifest_entries".into(), result.manifest_entries.to_string()),
        ("store_db_entries".into(), result.store_db_entries.to_string()),
        ("files".into(), result.files.to_string()),
        ("bytes".into(), result.bytes.to_string()),
        ("verified".into(), result.verified.to_string()),
        ("failures".into(), result.failures.len().to_string()),
        ("duration_ms".into(), duration_ms.to_string()),
    ]);
    if let Some(value) = result.hydrate_source.as_ref() {
        metadata.insert("hydrate_source".into(), value.clone());
    }
    if let Some(value) = result.restored_store_db_entries {
        metadata.insert("restored_store_db_entries".into(), value.to_string());
    }
    CopyInTelemetryEvent {
        level: if result.outcome == CopyInOutcome::Failed {
            "error"
        } else {
            "info"
        },
        metadata,
    }
}

fn report_copy_in_telemetry(
    telemetry: Option<&HostStructuredLogTelemetry>,
    result: &CopyInResult,
    duration_ms: u64,
) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let event = build_copy_in_result_telemetry(result, duration_ms);
    let _ = telemetry.report_box_copy_in(event.level, event.metadata);
}

pub fn resolve_copy_in_download_owner_values(
    effective_uid: u32,
    home_uid: u32,
    home_gid: u32,
) -> Option<(u32, u32)> {
    (effective_uid == 0 && home_uid > 0).then_some((home_uid, home_gid))
}

fn resolve_copy_in_download_owner() -> Option<(u32, u32)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let effective_uid = unsafe { libc::geteuid() };
        let metadata = fs::metadata(SAND_BOX_HOME_DIR).ok()?;
        return resolve_copy_in_download_owner_values(effective_uid, metadata.uid(), metadata.gid());
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn apply_download_owner(path: &Path, owner: Option<(u32, u32)>) -> Result<(), String> {
    let Some((uid, gid)) = owner else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|error| error.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, uid, gid);
        Ok(())
    }
}

fn store_id_from_environment(environment: &BTreeMap<String, String>) -> Result<Option<String>, String> {
    let Some(store_id) = environment
        .get("SAND_BOX_STORE_ID")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    normalize_rel_path(store_id).map(Some)
}

fn run_copy_in_with_providers(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
    provider: &dyn BoxObjectStoreProvider,
    legacy_provider: Option<&dyn BoxObjectStoreProvider>,
    hydration_marker_path: Option<&Path>,
    options: CopyInRuntimeOptions,
) -> CopyInResult {
    let store_id = match store_id_from_environment(environment) {
        Ok(Some(value)) => value,
        Ok(None) => return empty("no SAND_BOX_STORE_ID; booting fresh"),
        Err(error) => return failed_copy_in(format!("invalid store id: {error}")),
    };
    let store = provider.for_store(&store_id);
    let legacy_store = legacy_provider.map(|provider| provider.for_store(&store_id));
    run_box_copy_in_with_sources(
        store.as_ref(),
        legacy_store.as_deref(),
        target_root,
        hydration_marker_path,
        options,
    )
}

fn report_copy_in_result(result: &CopyInResult) -> i32 {
    match result {
        CopyInResult { outcome: CopyInOutcome::Hydrated | CopyInOutcome::Noop, .. } => {
            eprintln!(
                "[box-copy-in] outcome={:?} files={} bytes={} verified={} reason={}",
                result.outcome, result.files, result.bytes, result.verified, result.reason
            );
        }
        _ => {
            eprintln!(
                "[box-copy-in] failed files={} verified={} failures={} reason={}",
                result.files,
                result.verified,
                result.failures.len(),
                result.reason
            );
        }
    }
    outcome_to_exit_code(result.outcome)
}

fn run_copy_in_with_retry(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
    provider: &dyn BoxObjectStoreProvider,
    legacy_provider: Option<&dyn BoxObjectStoreProvider>,
    hydration_marker_path: Option<&Path>,
    options: CopyInRuntimeOptions,
) -> CopyInResult {
    let attempts = resolve_copy_in_attempts(
        environment
            .get("SAND_BOX_COPY_IN_ATTEMPTS")
            .map(String::as_str),
    )
    .max(1);
    let mut last = failed_copy_in("copy-in did not run".into());
    for attempt in 1..=attempts {
        last = run_copy_in_with_providers(
            environment,
            target_root,
            provider,
            legacy_provider,
            hydration_marker_path,
            options.clone(),
        );
        if last.outcome != CopyInOutcome::Failed
            || !is_transient_copy_in_failure(&last)
            || attempt == attempts
        {
            return last;
        }
        let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX).min(20);
        let backoff = COPY_IN_RETRY_BASE_MS
            .saturating_mul(1_u64 << exponent)
            .min(COPY_IN_RETRY_MAX_MS);
        eprintln!(
            "[box-copy-in] transient failure (attempt {attempt}/{attempts}): {}; retrying in {backoff}ms",
            last.reason
        );
        thread::sleep(Duration::from_millis(backoff));
    }
    last
}

/// Dependency-injected copy-in entrypoint. Without remote dependencies it is
/// appropriate for LocalFS/tests; the shipping CLI uses
/// execute_production_box_copy_in_from_env so AgentStore/V2 reuse Host auth.
pub fn execute_box_copy_in_from_env(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
) -> i32 {
    execute_box_copy_in_from_env_with_provider_dependencies(
        environment,
        target_root,
        BoxObjectStoreProviderDependencies::default(),
    )
}

/// Shipping copy-in entrypoint. LocalFS requires no authentication; remote
/// AgentStore/SandBoxStoreV2 reuse the canonical Host auth and machine-id
/// owners plus the same BoxObjectStore provider resolver used by the long-lived
/// production sync service.
pub fn execute_production_box_copy_in_from_env(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
) -> i32 {
    if !is_box_store_copy_in_enabled(environment) {
        return BOX_COPY_IN_EXIT_NOOP;
    }
    let started_at = Instant::now();
    let sand_root = get_sand_root_dir();
    let policy = resolve_box_store_backend_policy(environment);
    let store_backend = match policy.kind {
        BoxStoreBackendKind::Native => "fabushi-native",
        BoxStoreBackendKind::LocalFs => "local-fs",
        BoxStoreBackendKind::AgentStore => "agent-store",
        BoxStoreBackendKind::SandBoxStoreV2 => "sand-box-store-v2",
    };

    let mut telemetry_identity_tags = resolve_sand_box_identity_tags();
    telemetry_identity_tags.insert("store_backend".into(), store_backend.into());
    let telemetry = HostTelemetryService::open_with_identity_tags(
        sand_root.join("telemetry").join("host-events.jsonl"),
        telemetry_identity_tags,
    )
    .ok()
    .map(|service| service.logs.clone());
    if let Some(telemetry) = telemetry.as_ref() {
        let event = build_copy_in_started_telemetry();
        let _ = telemetry.report_box_copy_in(event.level, event.metadata);
    }

    let deps = if policy.local_dir.is_some() {
        BoxObjectStoreProviderDependencies::default()
    } else {
        let backend_url = match get_configured_backend_url() {
            Ok(url) => url,
            Err(error) => {
                let result = failed_copy_in(format!("invalid production backend URL: {error}"));
                report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                return report_copy_in_result(&result);
            }
        };
        let auth = match HostAuthService::production(|message| {
            eprintln!("[box-copy-in][auth] {message}");
        }) {
            Ok(auth) => Arc::new(auth),
            Err(error) => {
                let result = failed_copy_in(format!("failed to initialize production auth: {error}"));
                report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                return report_copy_in_result(&result);
            }
        };
        let token_auth = Arc::clone(&auth);
        let machine_auth = Arc::clone(&auth);
        BoxObjectStoreProviderDependencies {
            native_credentials: Some(auth.clone()),
            backend_url: Some(backend_url),
            get_access_token: Some(Arc::new(move || {
                token_auth.get_access_token().map_err(|error| error.to_string())
            })),
            get_machine_id: Some(Arc::new(move || {
                machine_auth.get_machine_id().map_err(|error| error.to_string())
            })),
        }
    };

    let legacy_provider: Option<Arc<dyn BoxObjectStoreProvider>> =
        if policy.kind == BoxStoreBackendKind::SandBoxStoreV2 {
            let legacy_deps = AgentStoreClientDependencies {
                backend_url: match deps.backend_url.clone() {
                    Some(value) => value,
                    None => {
                        let result = failed_copy_in(
                            "legacy AgentStore backend URL is not configured".into(),
                        );
                        report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                        return report_copy_in_result(&result);
                    }
                },
                get_access_token: match deps.get_access_token.clone() {
                    Some(value) => value,
                    None => {
                        let result = failed_copy_in(
                            "legacy AgentStore auth token resolver is not configured".into(),
                        );
                        report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                        return report_copy_in_result(&result);
                    }
                },
                get_machine_id: match deps.get_machine_id.clone() {
                    Some(value) => value,
                    None => {
                        let result = failed_copy_in(
                            "legacy AgentStore machine id resolver is not configured".into(),
                        );
                        report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                        return report_copy_in_result(&result);
                    }
                },
            };
            match AgentStoreObjectStoreProvider::new(legacy_deps) {
                Ok(provider) => Some(Arc::new(provider)),
                Err(error) => {
                    let result =
                        failed_copy_in(format!("legacy AgentStore provider unavailable: {error}"));
                    report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
                    return report_copy_in_result(&result);
                }
            }
        } else {
            None
        };

    let provider = match resolve_box_object_store_provider(environment, deps) {
        Ok(provider) => provider,
        Err(error) => {
            let result = failed_copy_in(format!("object-store provider unavailable: {error}"));
            report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
            return report_copy_in_result(&result);
        }
    };

    let lock_path = sand_root.join("box-store-sync.lock");
    let _lock = match try_acquire_copy_in_lock(&lock_path) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            let result = failed_copy_in("lock-held".into());
            let _ = write_copy_in_status_atomic(
                Path::new(SAND_BOX_COPY_IN_STATUS_PATH),
                &build_copy_in_status_from_result(&result),
            );
            report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
            eprintln!("[box-copy-in] could not acquire box-store lock; failing closed");
            return BOX_COPY_IN_EXIT_FAILED;
        }
        Err(error) => {
            let result = failed_copy_in(format!("store lock error: {error}"));
            let _ = write_copy_in_status_atomic(
                Path::new(SAND_BOX_COPY_IN_STATUS_PATH),
                &build_copy_in_status_from_result(&result),
            );
            report_copy_in_telemetry(
                    telemetry.as_ref(),
                    &result,
                    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
                );
            return report_copy_in_result(&result);
        }
    };

    let status_last_write = Arc::new(Mutex::new(None::<u64>));
    if let Ok(mut last) = status_last_write.lock() {
        let _ = write_copy_in_status_throttled(
            Path::new(SAND_BOX_COPY_IN_STATUS_PATH),
            &serde_json::json!({"phase":"copying","restored":0,"total":0,"bytes":0}),
            wall_clock_now_ms(),
            &mut last,
            true,
        );
    }

    let watch_state = Arc::new(Mutex::new(CopyInWatchState::default()));
    let progress_state = Arc::clone(&watch_state);
    let progress_last_write = Arc::clone(&status_last_write);
    let on_progress: Arc<dyn Fn(CopyInProgress) + Send + Sync> = Arc::new(move |progress| {
        if let Ok(mut state) = progress_state.lock() {
            state.progress = progress;
            state.progress_events = state.progress_events.saturating_add(1);
            let trace = state.trace.clone();
            drop(state);
            if let Ok(mut last) = progress_last_write.lock() {
                let mut status = serde_json::json!({
                    "phase":"copying",
                    "restored":progress.files,
                    "total":progress.total,
                    "bytes":progress.bytes,
                });
                if let Some(map) = status.as_object_mut() {
                    if let Some(stage) = trace.copy_stage {
                        map.insert("copyStage".into(), serde_json::Value::String(stage));
                    }
                    if let Some(value) = trace.file_entries {
                        map.insert("fileEntries".into(), value.into());
                    }
                    if let Some(value) = trace.symlink_entries {
                        map.insert("symlinkEntries".into(), value.into());
                    }
                    if let Some(value) = trace.symlinks_started {
                        map.insert("symlinksStarted".into(), value.into());
                    }
                    if let Some(value) = trace.symlinks_completed {
                        map.insert("symlinksCompleted".into(), value.into());
                    }
                    if let Some(value) = trace.symlinks_in_flight {
                        map.insert("symlinksInFlight".into(), value.into());
                    }
                }
                let _ = write_copy_in_status_throttled(
                    Path::new(SAND_BOX_COPY_IN_STATUS_PATH),
                    &status,
                    wall_clock_now_ms(),
                    &mut last,
                    false,
                );
            }
        }
    });
    let trace_state = Arc::clone(&watch_state);
    let on_trace: Arc<dyn Fn(CopyInTraceStatus) + Send + Sync> = Arc::new(move |trace| {
        if let Ok(mut state) = trace_state.lock() {
            state.trace = trace;
        }
    });

    let stuck_threshold_ms = resolve_copy_in_stuck_threshold_ms(
        environment
            .get("SAND_BOX_COPY_IN_STUCK_MS")
            .map(String::as_str),
    );
    let (watchdog_stop_tx, watchdog_stop_rx) = mpsc::channel::<()>();
    let watchdog_state = Arc::clone(&watch_state);
    let watchdog_telemetry = telemetry.clone();
    let watchdog_started = started_at;
    let watchdog = thread::spawn(move || {
        let mut progress_events_at_last_tick = 0_u64;
        loop {
            match watchdog_stop_rx.recv_timeout(Duration::from_millis(stuck_threshold_ms)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let snapshot = watchdog_state
                        .lock()
                        .map(|state| state.clone())
                        .unwrap_or_default();
                    let progressed = snapshot.progress_events > progress_events_at_last_tick;
                    progress_events_at_last_tick = snapshot.progress_events;
                    let mut event = build_copy_in_watchdog_event(
                        progressed,
                        snapshot.progress,
                        watchdog_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                        stuck_threshold_ms,
                        Some(&snapshot.trace),
                    );
                    eprintln!(
                        "[box-copy-in] STILL copying files={}/{} bytes={} outcome={}",
                        snapshot.progress.files,
                        snapshot.progress.total,
                        snapshot.progress.bytes,
                        event.metadata.get("outcome").map(String::as_str).unwrap_or("stuck")
                    );
                    if let Some(telemetry) = watchdog_telemetry.as_ref() {
                        let _ = telemetry.report_box_copy_in(event.level, event.metadata);
                    }
                }
            }
        }
    });

    let marker_path = sand_root.join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME);
    let result = run_copy_in_with_retry(
        environment,
        target_root,
        provider.as_ref(),
        legacy_provider.as_deref(),
        Some(&marker_path),
        CopyInRuntimeOptions {
            download_owner: resolve_copy_in_download_owner(),
            download_concurrency: resolve_copy_in_concurrency(
                environment
                    .get("SAND_BOX_STORE_COPY_IN_CONCURRENCY")
                    .map(String::as_str),
            ),
            on_progress: Some(on_progress),
            on_trace: Some(on_trace),
        },
    );

    let _ = watchdog_stop_tx.send(());
    let _ = watchdog.join();
    if let Ok(mut last) = status_last_write.lock() {
        let _ = write_copy_in_status_throttled(
            Path::new(SAND_BOX_COPY_IN_STATUS_PATH),
            &build_copy_in_status_from_result(&result),
            wall_clock_now_ms(),
            &mut last,
            true,
        );
    }
    report_copy_in_telemetry(
        telemetry.as_ref(),
        &result,
        started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
    );
    report_copy_in_result(&result)
}

pub fn execute_box_copy_in_from_env_with_provider_dependencies(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
    deps: BoxObjectStoreProviderDependencies,
) -> i32 {
    if !is_box_store_copy_in_enabled(environment) {
        return BOX_COPY_IN_EXIT_NOOP;
    }
    let provider = match resolve_box_object_store_provider(environment, deps) {
        Ok(provider) => provider,
        Err(error) => return report_copy_in_result(&failed_copy_in(format!(
            "object-store provider unavailable: {error}"
        ))),
    };
    execute_box_copy_in_with_provider(environment, target_root, provider.as_ref())
}

pub fn execute_box_copy_in_with_provider(
    environment: &BTreeMap<String, String>,
    target_root: &Path,
    provider: &dyn BoxObjectStoreProvider,
) -> i32 {
    report_copy_in_result(&run_copy_in_with_providers(
        environment,
        target_root,
        provider,
        None,
        None,
        CopyInRuntimeOptions::default(),
    ))
}

#[derive(Debug, Clone)]
struct ManifestSnapshot {
    present: bool,
    entries: BTreeMap<String, BoxStoreManifestEntry>,
    fully_hydrated: Option<bool>,
}

fn read_manifest_snapshot(
    store: &dyn BoxObjectStore,
    label: &str,
) -> Result<ManifestSnapshot, String> {
    let Some(bytes) = store
        .get(BOX_STORE_MANIFEST_REL_PATH)
        .map_err(|error| format!("{label} store manifest unreadable: {error}"))?
    else {
        return Ok(ManifestSnapshot {
            present: false,
            entries: BTreeMap::new(),
            fully_hydrated: None,
        });
    };
    let raw: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{label} store manifest unreadable: {error}"))?;
    let manifest = parse_box_store_manifest(&raw)
        .ok_or_else(|| format!("{label} store manifest unreadable: invalid manifest"))?;
    Ok(ManifestSnapshot {
        present: true,
        entries: manifest.entries,
        fully_hydrated: manifest.fully_hydrated,
    })
}

fn mark_primary_legacy_hydration_incomplete(
    primary: &dyn BoxObjectStore,
) -> Result<(), String> {
    let baseline = primary.get(BOX_STORE_MANIFEST_REL_PATH)?;
    let mut value = match baseline.as_deref() {
        Some(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|error| format!("primary manifest unreadable before legacy hydrate: {error}"))?;
            parse_box_store_manifest(&value)
                .ok_or_else(|| "primary manifest unreadable before legacy hydrate: invalid manifest".to_string())?;
            value
        }
        None => serde_json::json!({
            "version": BOX_STORE_MANIFEST_VERSION,
            "updatedAtMs": wall_clock_now_ms(),
            "fullyHydrated": false,
            "entries": {}
        }),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| "primary manifest unreadable before legacy hydrate: expected object".to_string())?;
    object.insert(
        "updatedAtMs".into(),
        serde_json::Value::from(wall_clock_now_ms()),
    );
    object.insert("fullyHydrated".into(), serde_json::Value::Bool(false));
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| format!("failed to serialize primary legacy-hydration manifest: {error}"))?;
    if primary.put_if_unchanged(
        BOX_STORE_MANIFEST_REL_PATH,
        baseline.as_deref(),
        &encoded,
    )? {
        Ok(())
    } else {
        Err("primary manifest changed while marking legacy hydration incomplete".into())
    }
}

fn failure_with_source(
    reason: impl Into<String>,
    manifest_entries: usize,
    store_db_entries: usize,
    restored_store_db_entries: Option<usize>,
    hydrate_source: Option<String>,
) -> CopyInResult {
    let reason = reason.into();
    CopyInResult {
        outcome: CopyInOutcome::Failed,
        reason: reason.clone(),
        manifest_entries,
        store_db_entries,
        restored_store_db_entries,
        files: 0,
        bytes: 0,
        verified: 0,
        failures: vec![reason],
        hydrate_source,
    }
}

fn emit_trace(options: &CopyInRuntimeOptions, trace: &CopyInTraceStatus) {
    if let Some(callback) = options.on_trace.as_ref() {
        callback(trace.clone());
    }
}

fn emit_progress(
    options: &CopyInRuntimeOptions,
    manifest_entries: usize,
    files: usize,
    bytes: u64,
) {
    if let Some(callback) = options.on_progress.as_ref() {
        callback(CopyInProgress {
            total: manifest_entries,
            files,
            bytes,
        });
    }
}

fn restore_manifest(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    manifest: &BTreeMap<String, BoxStoreManifestEntry>,
    hydrate_source: Option<String>,
    downloaded_from_legacy: bool,
    authoritative_store_db_entries: Option<usize>,
    authoritative_store_db_agent_ids: Option<&HashSet<String>>,
    options: &CopyInRuntimeOptions,
) -> CopyInResult {
    if manifest.is_empty() {
        return empty("store empty; first boot");
    }

    let manifest_entries = manifest.len();
    let store_db_entries =
        authoritative_store_db_entries.unwrap_or_else(|| count_store_db_manifest_entries(Some(manifest)));

    let progress_options = options.clone();
    let trace_options = options.clone();
    let download = download_manifest(
        store,
        target_root,
        manifest,
        BoxStoreDownloadOptions {
            download_concurrency: options
                .download_concurrency
                .unwrap_or(super::box_store_sync::DEFAULT_COPY_IN_CONCURRENCY),
            owner: options.download_owner,
            on_progress: Some(Arc::new(move |progress: BoxStoreDownloadProgress| {
                emit_progress(
                    &progress_options,
                    progress.total,
                    progress.files,
                    progress.bytes,
                );
            })),
            on_trace: Some(Arc::new(move |trace: BoxStoreDownloadTrace| {
                let copy_stage = if trace.event.starts_with("symlink") {
                    "symlink"
                } else {
                    "manifest"
                };
                emit_trace(
                    &trace_options,
                    &CopyInTraceStatus {
                        copy_stage: Some(copy_stage.into()),
                        file_entries: Some(trace.file_entries),
                        symlink_entries: Some(trace.symlink_entries),
                        symlinks_started: Some(trace.symlinks_started),
                        symlinks_completed: Some(trace.symlinks_completed),
                        symlinks_in_flight: Some(trace.symlinks_in_flight),
                        active_symlink_steps: trace.active_symlink_steps,
                    },
                );
            })),
            on_log: Some(Arc::new(|message| {
                eprintln!("[box-copy-in] {message}");
            })),
            ..BoxStoreDownloadOptions::default()
        },
    );

    let files = download.files;
    let bytes = download.bytes;
    let verified = download.verified;
    let failures = download.failures;

    let restored_store_db_entries = if let Some(authoritative_ids) = authoritative_store_db_agent_ids
    {
        authoritative_ids
            .iter()
            .filter(|agent_id| {
                target_root
                    .join("home/box/sand-data/agents")
                    .join(agent_id.as_str())
                    .join("store.db")
                    .is_file()
            })
            .count()
    } else {
        manifest
            .iter()
            .filter(|(rel_path, entry)| {
                !entry.is_symlink()
                    && rel_path.starts_with("home/box/sand-data/agents/")
                    && rel_path.ends_with("/store.db")
                    && target_root.join(rel_path).is_file()
            })
            .count()
    };

    let evidence = HydrationEvidence {
        failures: Some(failures.clone()),
        manifest_entries: Some(manifest_entries as u64),
        files: Some(files as u64),
        verified: Some(verified as u64),
        hydrate_source: downloaded_from_legacy.then(|| "legacy".into()),
        authoritative_store_db_entries: Some(store_db_entries as u64),
        restored_store_db_entries: Some(restored_store_db_entries as u64),
    };
    let fully_hydrated = is_box_store_fully_hydrated(Some(&evidence))
        && (hydrate_source.as_deref() != Some("legacy")
            || restored_store_db_entries >= store_db_entries);

    if fully_hydrated {
        CopyInResult {
            outcome: CopyInOutcome::Hydrated,
            reason: if hydrate_source.as_deref() == Some("legacy") {
                "store hydrated from legacy (v2 migration)".into()
            } else {
                "store hydrated".into()
            },
            manifest_entries,
            store_db_entries,
            restored_store_db_entries: Some(restored_store_db_entries),
            files,
            bytes,
            verified,
            failures: Vec::new(),
            hydrate_source,
        }
    } else {
        let reason = if hydrate_source.as_deref() == Some("legacy")
            && restored_store_db_entries < store_db_entries
        {
            format!(
                "{INCOMPLETE_LEGACY_HYDRATE_REASON} advertised_store_db={store_db_entries} restored_store_db={restored_store_db_entries} advisory_advertised_files={manifest_entries} advisory_restored_files={files}"
            )
        } else {
            format!(
                "partial hydrate ({files}/{manifest_entries} files, {} failures)",
                failures.len()
            )
        };
        CopyInResult {
            outcome: CopyInOutcome::Failed,
            reason,
            manifest_entries,
            store_db_entries,
            restored_store_db_entries: Some(restored_store_db_entries),
            files,
            bytes,
            verified,
            failures,
            hydrate_source,
        }
    }
}

pub fn run_box_copy_in_with_sources(
    primary: &dyn BoxObjectStore,
    legacy: Option<&dyn BoxObjectStore>,
    target_root: &Path,
    hydration_marker_path: Option<&Path>,
    options: CopyInRuntimeOptions,
) -> CopyInResult {
    let primary_snapshot = match read_manifest_snapshot(primary, "") {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return failed_copy_in(error.trim_start().to_string());
        }
    };
    let mut manifest = primary_snapshot.entries;
    let mut download_store = primary;
    let mut hydrate_source = None::<String>;
    let mut downloaded_from_legacy = false;
    let mut authoritative_store_db_entries = None::<usize>;
    let mut authoritative_store_db_agent_ids = None::<HashSet<String>>;

    if (!primary_snapshot.present || primary_snapshot.fully_hydrated == Some(false))
        && legacy.is_some()
    {
        let legacy = legacy.expect("legacy source checked");
        let legacy_snapshot = match read_manifest_snapshot(legacy, "legacy") {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return failure_with_source(
                    error,
                    0,
                    0,
                    Some(0),
                    Some("legacy".into()),
                );
            }
        };
        if !legacy_snapshot.entries.is_empty() {
            let legacy_store_db_entries =
                count_store_db_manifest_entries(Some(&legacy_snapshot.entries));
            let legacy_store_db_ids =
                get_store_db_manifest_agent_ids(Some(&legacy_snapshot.entries));
            authoritative_store_db_entries = Some(legacy_store_db_entries);
            authoritative_store_db_agent_ids = Some(legacy_store_db_ids.clone());
            hydrate_source = Some("legacy".into());

            if primary_snapshot.fully_hydrated == Some(false) && !manifest.is_empty() {
                let primary_ids = get_store_db_manifest_agent_ids(Some(&manifest));
                if legacy_store_db_ids.is_empty() {
                    return failure_with_source(
                        format!(
                            "{INCOMPLETE_LEGACY_HYDRATE_REASON} legacy source has no store.db identities to validate V2"
                        ),
                        manifest.len(),
                        0,
                        Some(0),
                        hydrate_source,
                    );
                }
                let covered = legacy_store_db_ids
                    .iter()
                    .all(|agent_id| primary_ids.contains(agent_id));
                if !covered {
                    let restored = legacy_store_db_ids
                        .iter()
                        .filter(|agent_id| primary_ids.contains(*agent_id))
                        .count();
                    return failure_with_source(
                        format!(
                            "{INCOMPLETE_LEGACY_HYDRATE_REASON} primary V2 store.db coverage is below the legacy source"
                        ),
                        manifest.len(),
                        legacy_store_db_entries,
                        Some(restored),
                        hydrate_source,
                    );
                }
                eprintln!(
                    "[box-copy-in] primary V2 manifest is not sealed but covers all {legacy_store_db_entries} legacy store.db entries; hydrating the newer V2 store"
                );
            } else {
                eprintln!(
                    "[box-copy-in] own store never seeded; hydrating {} entries from the legacy store (v2 migration)",
                    legacy_snapshot.entries.len()
                );
                manifest = legacy_snapshot.entries;
                download_store = legacy;
                downloaded_from_legacy = true;
            }
        } else if primary_snapshot.fully_hydrated == Some(false) {
            return failure_with_source(
                "legacy source manifest is empty for incomplete primary",
                manifest.len(),
                count_store_db_manifest_entries(Some(&manifest)),
                Some(0),
                Some("legacy".into()),
            );
        }
    }

    if manifest.is_empty() {
        return empty("store empty; first boot");
    }

    if hydrate_source.as_deref() == Some("legacy") {
        let Some(marker_path) = hydration_marker_path else {
            return failure_with_source(
                "failed to mark legacy hydrate incomplete: hydration handoff marker is unavailable",
                manifest.len(),
                count_store_db_manifest_entries(Some(&manifest)),
                Some(0),
                hydrate_source,
            );
        };
        if let Err(error) = remove_hydration_handoff_marker(marker_path) {
            return failure_with_source(
                format!("failed to mark legacy hydrate incomplete: {error}"),
                manifest.len(),
                count_store_db_manifest_entries(Some(&manifest)),
                Some(0),
                hydrate_source,
            );
        }
        if let Err(error) = mark_primary_legacy_hydration_incomplete(primary) {
            return failure_with_source(
                format!("failed to mark legacy hydrate incomplete: {error}"),
                manifest.len(),
                authoritative_store_db_entries
                    .unwrap_or_else(|| count_store_db_manifest_entries(Some(&manifest))),
                Some(0),
                hydrate_source,
            );
        }
    }

    let result = restore_manifest(
        download_store,
        target_root,
        &manifest,
        hydrate_source.clone(),
        downloaded_from_legacy,
        authoritative_store_db_entries,
        authoritative_store_db_agent_ids.as_ref(),
        &options,
    );

    if result.outcome == CopyInOutcome::Hydrated
        && result.hydrate_source.as_deref() == Some("legacy")
    {
        if let Some(marker_path) = hydration_marker_path {
            if let Err(error) = write_hydration_handoff_marker(marker_path) {
                eprintln!(
                    "[box-copy-in] failed to persist legacy hydrate handoff after complete restore: {error}"
                );
            }
        }
    }
    result
}

/// Canonical local-store copy-in orchestration used by focused contract tests.
/// The shipping `--box-copy-in` path calls run_box_copy_in_with_sources so V2
/// can coordinate with the legacy AgentStore source before hydration.
pub fn run_local_box_copy_in(store: &dyn BoxObjectStore, target_root: &Path) -> CopyInResult {
    let snapshot = match read_manifest_snapshot(store, "") {
        Ok(snapshot) => snapshot,
        Err(error) => return failed_copy_in(error.trim_start().to_string()),
    };
    if !snapshot.present || snapshot.entries.is_empty() {
        return empty("store empty; first boot");
    }
    if snapshot.fully_hydrated == Some(false) {
        return failed_copy_in(format!(
            "{INCOMPLETE_LEGACY_HYDRATE_REASON}: local manifest is not fully hydrated"
        ));
    }
    restore_manifest(
        store,
        target_root,
        &snapshot.entries,
        None,
        false,
        None,
        None,
        &CopyInRuntimeOptions::default(),
    )
}


fn failed_copy_in(reason: String) -> CopyInResult {
    CopyInResult {
        outcome: CopyInOutcome::Failed,
        reason: reason.clone(),
        manifest_entries: 0,
        store_db_entries: 0,
        restored_store_db_entries: None,
        files: 0,
        bytes: 0,
        verified: 0,
        failures: vec![reason],
        hydrate_source: None,
    }
}
