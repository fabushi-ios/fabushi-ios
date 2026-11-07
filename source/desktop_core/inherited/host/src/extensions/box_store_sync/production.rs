use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex, TryLockError,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::durable_file_policy::BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES;
use crate::extensions::box_store_sync::agent_store_sand_files::AgentStoreClientDependencies;
use crate::extensions::box_store_sync::box_object_store::{
    resolve_box_object_store_provider, AgentStoreObjectStoreProvider, BoxObjectStore,
    BoxObjectStoreProvider, BoxObjectStoreProviderDependencies,
};
use crate::extensions::box_store_sync::box_store_hydration::{
    remove_hydration_handoff_marker, BOX_STORE_HYDRATION_HANDOFF_FILE_NAME,
    BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH,
};
use crate::extensions::box_store_sync::box_store_manifest::{
    count_agent_dir_manifest_entries, count_store_db_manifest_entries, load_manifest_for_write,
    prepare_canonical_manifest_reset, read_manifest_strict, serialize_manifest_bytes,
    set_manifest_entry, write_manifest_with_retry, write_manifest_with_retry_and_conflict_reporter,
    BoxManifestMap, BoxStoreManifestConflictInfo, ManifestHydrationUpdate, ManifestSaveOptions,
    AGENT_STORE_DB_BASENAMES,
};
use crate::extensions::box_store_sync::box_store_manifest_format::{
    BoxStoreManifest, BoxStoreManifestEntry, BOX_STORE_BLOBS_PREFIX, BOX_STORE_MANIFEST_REL_PATH,
    BOX_STORE_MANIFEST_VERSION, SAND_MANIFEST_V2_ENV,
};
use crate::extensions::box_store_sync::box_store_pack::{
    build_pack_file, is_box_store_pack_build_enabled, parse_pack_index, parse_pack_retired,
    plan_pack_maintenance, serialize_pack_index, serialize_pack_retired, PackEntry, PackIndex,
    PackSource, BOX_STORE_PACKS_PREFIX, BOX_STORE_PACK_INDEX_KEY, BOX_STORE_PACK_RETIRED_KEY,
    PACK_INDEX_VERSION, PACK_MAX_MEMBER_SIZE_SUM, PACK_MEMBER_MAX_BYTES,
};
use crate::extensions::box_store_sync::box_store_pack_pipeline::{
    PACK_BUILD_MIN_BYTES, PACK_BUILD_MIN_MEMBERS, PACK_TMP_DIR_NAME, PACK_TMP_MAX_AGE_MS,
};
use crate::extensions::box_store_sync::box_store_sync::{
    box_store_sync_cycle_telemetry, BoxStoreCycleSummary, BoxStoreWriterLock,
    BOX_STORE_WRITER_LOCK_FILE_NAME, LARGE_OBJECT_THRESHOLD_BYTES,
};
use crate::extensions::box_store_sync::box_store_sync_service::{
    build_box_home_category, build_box_home_ignore, is_better_cli_home_enabled,
    plan_periodic_cycle, BOX_HOME_DIR, BOX_HOME_PRUNE_GUARDED_FOREIGN_TREES, BOX_HOME_REL_PREFIX,
    BOX_STORE_MANIFEST_RETRY_ATTEMPTS, BOX_STORE_MANIFEST_RETRY_DELAY_MS,
    SAND_STORE_BETTER_CLI_ENV, SAND_USER_NON_ROOT_ENV,
};
use crate::extensions::box_store_sync::box_store_transfer::{
    glob_matches_path, BoxStoreTransfer, CategoryTransferSummary, StagedTransferFile,
    BOX_STORE_SNAPSHOT_TMP_SUFFIX,
};
use crate::extensions::box_store_sync::chrome_session_stage::{
    chrome_session_stage_telemetry, stage_box_chrome_session_with_report,
    CHROME_AUTH_STATE_CACHE_EXCLUDE_NAMES, CHROME_AUTH_STATE_REL_DIRS, CHROME_SESSION_DB_DIR,
    CHROME_SESSION_DB_NAMES, CHROME_SESSION_DB_REL_DIR,
};
use crate::extensions::box_store_sync::chrome_session_watcher::ChromeSessionWatcher;
use crate::extensions::box_store_sync::extension::{
    BoxStoreSyncExtensionDeps, BoxStoreSyncService, BoxStoreSyncServiceFactory,
    BoxStoreSyncTelemetryReporter,
};
use crate::extensions::box_store_sync::store_db_bundle_capture::{
    create_store_db_capture_trace, record_store_db_capture_failure, AgentDbCaptureQueues,
    StoreDbCaptureFailurePhase, StoreDbCaptureTrace,
};
use crate::extensions::box_store_sync::store_db_capture::{
    aggregate_store_db_sweep_outcome, box_store_db_capture_telemetry,
    BoxStoreDbCaptureTelemetrySummary, StoreDbCaptureOutcome,
};
use crate::extensions::box_store_sync::store_db_snapshot_upload::{
    SnapshotUploadOutcome, StoreDbSnapshotRuntimePort, StoreDbSnapshotUpload,
};
use crate::extensions::box_store_sync::workspace_ignore::{
    load_workspace_ignore, WorkspaceIgnore, SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS,
};
use crate::host_paths::get_sand_root_dir;
use crate::r#box::box_store_backend_policy::{
    is_box_store_sync_enabled, resolve_box_store_backend_policy, BoxStoreBackendKind,
};
use crate::storage::store_db::{get_sand_agent_db_write_generation, has_live_sand_agent_db_handle};

const SAND_BOX_STORE_ID_ENV: &str = "SAND_BOX_STORE_ID";
const FABUSHI_OPENCLAW_PORTABLE_BACKUP_ENV: &str = "FABUSHI_OPENCLAW_PORTABLE_BACKUP";
const WORKSPACE_ROOT: &str = "/workspace";
const CLI_CONFIG_ROOT: &str = "/home/box/cli-config";
const SAND_DATA_REL_PREFIX: &str = "home/box/sand-data";
const WORKSPACE_REL_PREFIX: &str = "workspace";
const CLI_CONFIG_REL_PREFIX: &str = "home/box/cli-config";
const POLL_SLEEP_SLICE_MS: u64 = 250;
const DEFAULT_MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const DEFAULT_SNAPSHOT_OUT_CONCURRENCY: usize = 8;

fn openclaw_portable_backup_enabled() -> bool {
    matches!(
        std::env::var(FABUSHI_OPENCLAW_PORTABLE_BACKUP_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

const SAND_DATA_EXCLUDES: &[&str] = &[
    "home/box/sand-data/host.lock",
    "home/box/sand-data/gateway.json",
    "home/box/sand-data/box-store-sync.lock",
    "home/box/sand-data/box-store-hydration-handoff.json",
    BOX_STORE_HYDRATION_HANDOFF_MANIFEST_PATH,
];

const CLI_CONFIG_EXCLUDES: &[&str] = &[
    "home/box/cli-config/.cli-auth-tmp*",
    "home/box/cli-config/*.cli-auth-old",
    "home/box/cli-config/**/*.cli-auth-old",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionBoxStoreSyncMode {
    Disabled,
    Native,
    LocalFs {
        base_dir: PathBuf,
        store_id_override: Option<String>,
    },
    AgentStore,
    SandBoxStoreV2,
    UnsupportedRemote {
        backend: BoxStoreBackendKind,
    },
    InvalidLocalConfiguration {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProductionBoxStoreSyncStatus {
    pub enabled: bool,
    pub backend: String,
    pub last_ok: Option<bool>,
    pub last_reason: Option<String>,
    pub last_manifest_entries: usize,
    pub last_files_uploaded: usize,
    pub last_bytes_uploaded: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProductionBoxStoreDurableStatus {
    pub durable: bool,
    pub fully_hydrated: Option<bool>,
    pub entry_count: usize,
    pub store_db_entries: usize,
    pub agent_dir_entries: usize,
    pub total_bytes: u64,
    pub last_snapshot_at_ms: u64,
}

#[derive(Clone)]
pub struct ProductionBoxStoreSyncApi {
    inner: Arc<ProductionBoxStoreSyncInner>,
}

impl ProductionBoxStoreSyncApi {
    pub fn is_enabled(&self) -> bool {
        self.status().enabled
    }

    pub fn get_store_id(&self) -> Result<String, String> {
        self.inner.resolve_store_id()
    }

    pub fn status(&self) -> ProductionBoxStoreSyncStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn snapshot_local_now(&self) -> Result<ProductionBoxStoreSyncStatus, String> {
        self.inner.flush_waiters.fetch_add(1, Ordering::AcqRel);
        let result = self
            .inner
            .run_local_cycle(false, true, false, false, false, true, true);
        self.inner.flush_waiters.fetch_sub(1, Ordering::AcqRel);
        result?;
        Ok(self.status())
    }

    pub fn get_box_store_status(&self) -> ProductionBoxStoreDurableStatus {
        self.inner.read_store_status()
    }

    pub fn clear_box_store_now(&self) -> Result<(), String> {
        self.inner.clear_store_now()
    }

    pub fn forget_agent(&self, agent_id: &str) -> Result<(), String> {
        self.inner.forget_agent(agent_id)
    }

    pub fn object_store_for(&self, source_id: &str) -> Result<Arc<dyn BoxObjectStore>, String> {
        self.inner.object_store_for_source_id(source_id)
    }

    pub fn schedule_store_db_snapshot(&self, agent_id: &str) -> bool {
        let agent_id = agent_id.trim();
        if agent_id.is_empty()
            || self.inner.stopped.load(Ordering::Acquire)
            || !matches!(
                &self.inner.mode,
                ProductionBoxStoreSyncMode::LocalFs { .. }
                    | ProductionBoxStoreSyncMode::AgentStore
                    | ProductionBoxStoreSyncMode::SandBoxStoreV2
                    | ProductionBoxStoreSyncMode::Native
            )
        {
            return false;
        }
        let delay =
            Duration::from_millis(self.inner.deps.scheduling.store_db_debounce.delay_ms.max(1));
        let mut queue = self
            .inner
            .store_db_debounce
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.schedule(agent_id.to_string(), Instant::now() + delay);
        drop(queue);
        self.inner.store_db_wake.notify_one();
        true
    }
}

pub struct ProductionBoxStoreSyncFactory;

impl BoxStoreSyncServiceFactory for ProductionBoxStoreSyncFactory {
    type Service = ProductionBoxStoreSyncService;

    fn create(&self, deps: BoxStoreSyncExtensionDeps) -> Self::Service {
        ProductionBoxStoreSyncService::new(deps)
    }
}

pub struct ProductionBoxStoreSyncService {
    inner: Arc<ProductionBoxStoreSyncInner>,
    watcher: Mutex<Option<ChromeSessionWatcher>>,
    poller: Mutex<Option<JoinHandle<()>>>,
    store_db_worker: Mutex<Option<JoinHandle<()>>>,
}

impl ProductionBoxStoreSyncService {
    pub fn new(deps: BoxStoreSyncExtensionDeps) -> Self {
        let env = std::env::vars().collect::<BTreeMap<_, _>>();
        let mode = resolve_production_box_store_sync_mode(&env);
        let backend = mode_name(&mode).to_string();
        let enabled = matches!(
            mode,
            ProductionBoxStoreSyncMode::LocalFs { .. }
                | ProductionBoxStoreSyncMode::Native
                | ProductionBoxStoreSyncMode::AgentStore
                | ProductionBoxStoreSyncMode::SandBoxStoreV2
        );
        let transfer_log = Arc::clone(&deps.log);
        let transfer = BoxStoreTransfer::new(
            DEFAULT_MAX_OBJECT_BYTES,
            DEFAULT_SNAPSHOT_OUT_CONCURRENCY,
            LARGE_OBJECT_THRESHOLD_BYTES,
            Some(get_sand_root_dir().join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME)),
            Arc::new(move |message| {
                transfer_log(&format!("[box-store-sync] {message}"));
            }),
        );
        Self {
            inner: Arc::new(ProductionBoxStoreSyncInner {
                deps,
                env,
                mode,
                transfer,
                started: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
                cycle_lock: Mutex::new(()),
                writer_lock: Mutex::new(None),
                store_db_debounce: Mutex::new(StoreDbDebounceQueue::default()),
                store_db_wake: Condvar::new(),
                agent_db_capture_queues: AgentDbCaptureQueues::default(),
                object_store_provider: Mutex::new(None),
                last_idle_only_sync: Mutex::new(Instant::now()),
                flush_waiters: AtomicUsize::new(0),
                status: Mutex::new(ProductionBoxStoreSyncStatus {
                    enabled,
                    backend,
                    ..ProductionBoxStoreSyncStatus::default()
                }),
            }),
            watcher: Mutex::new(None),
            poller: Mutex::new(None),
            store_db_worker: Mutex::new(None),
        }
    }

    pub fn mode(&self) -> &ProductionBoxStoreSyncMode {
        &self.inner.mode
    }
}

impl BoxStoreSyncService for ProductionBoxStoreSyncService {
    type Api = ProductionBoxStoreSyncApi;

    fn api(&self) -> Self::Api {
        ProductionBoxStoreSyncApi {
            inner: Arc::clone(&self.inner),
        }
    }

    fn start(&self) {
        if self.inner.stopped.load(Ordering::Acquire)
            || self.inner.started.swap(true, Ordering::AcqRel)
        {
            return;
        }

        match &self.inner.mode {
            ProductionBoxStoreSyncMode::Disabled => {
                self.inner
                    .log("box-store sync disabled by SAND_BOX_STORE_SYNC");
                self.inner.report_sync_lifecycle("warn", false, "disabled");
                return;
            }
            ProductionBoxStoreSyncMode::UnsupportedRemote { backend } => {
                self.inner.log(&format!(
                    "box-store sync backend {backend:?} is not production-wired in Rust yet; refusing to downgrade"
                ));
                self.inner
                    .diagnostic("startup", "remote-backend-not-wired", false);
                return;
            }
            ProductionBoxStoreSyncMode::InvalidLocalConfiguration { reason } => {
                self.inner.log(&format!(
                    "box-store sync local configuration rejected: {reason}"
                ));
                self.inner
                    .diagnostic("startup", "invalid-local-configuration", false);
                return;
            }
            ProductionBoxStoreSyncMode::LocalFs { .. }
            | ProductionBoxStoreSyncMode::Native
            | ProductionBoxStoreSyncMode::AgentStore
            | ProductionBoxStoreSyncMode::SandBoxStoreV2 => {}
        }

        let chrome_inner = Arc::clone(&self.inner);
        let watcher = ChromeSessionWatcher::with_logger(
            CHROME_SESSION_DB_DIR,
            CHROME_SESSION_DB_NAMES.iter().copied(),
            self.inner.deps.scheduling.chrome_session_debounce.delay_ms,
            Arc::new(move || {
                if let Err(error) =
                    chrome_inner.run_local_cycle(true, false, false, false, false, true, false)
                {
                    chrome_inner.log(&format!("chrome-session snapshot rejected: {error}"));
                    chrome_inner.diagnostic("chrome-session", &error, false);
                }
            }),
            {
                let inner = Arc::clone(&self.inner);
                Arc::new(move |message| inner.log(&message))
            },
        );
        watcher.start();
        *self
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(watcher);

        let store_db_inner = Arc::clone(&self.inner);
        match thread::Builder::new()
            .name("box-store-db-debounce".into())
            .spawn(move || run_store_db_debounce_loop(store_db_inner))
        {
            Ok(handle) => {
                *self
                    .store_db_worker
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
            }
            Err(error) => {
                self.inner.log(&format!(
                    "store.db debounce worker failed to start: {error}"
                ));
                self.inner
                    .diagnostic("startup", "store-db-debounce-worker-start-failed", false);
            }
        }

        let poll_inner = Arc::clone(&self.inner);
        let interval_ms = self.inner.deps.scheduling.polling.interval_ms.max(1);
        let handle = thread::Builder::new()
            .name("box-store-sync-poller".into())
            .spawn(move || {
                let roots = snapshot_transfer_roots(&poll_inner.env);
                let removed = poll_inner.transfer.sweep_leaked_temps(&roots);
                if removed > 0 {
                    poll_inner.log(&format!("swept {removed} leaked box-store temp file(s)"));
                }
                while !poll_inner.stopped.load(Ordering::Acquire) {
                    if !sleep_interruptibly(&poll_inner.stopped, interval_ms) {
                        break;
                    }
                    let idle = (poll_inner.deps.is_idle)();
                    let hydration_handoff_pending = get_sand_root_dir()
                        .join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME)
                        .exists();
                    let plan = plan_periodic_cycle(
                        idle,
                        hydration_handoff_pending,
                        poll_inner.elapsed_since_idle_only_sync_ms(),
                    );
                    if plan.include_idle_only {
                        poll_inner.mark_idle_only_sync();
                    }
                    if let Err(error) = poll_inner.run_local_cycle(
                        false,
                        plan.include_store_dbs,
                        plan.skip_live_handle_store_dbs,
                        plan.include_packs,
                        plan.include_idle_only,
                        false,
                        false,
                    ) {
                        poll_inner.log(&format!("periodic snapshot rejected: {error}"));
                        poll_inner.diagnostic("periodic", &error, false);
                    }
                }
            });
        match handle {
            Ok(handle) => {
                *self
                    .poller
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
                let backend = mode_name(&self.inner.mode);
                self.inner.log(&format!(
                    "box-store sync {backend} production slice enabled"
                ));
                self.inner.report_sync_lifecycle("info", true, "enabled");
            }
            Err(error) => {
                self.inner
                    .log(&format!("box-store poller failed to start: {error}"));
                self.inner
                    .diagnostic("startup", "poller-start-failed", false);
            }
        }
    }

    fn dispose(&self) {
        if self.inner.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.inner.store_db_wake.notify_all();
        if let Some(watcher) = self
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            watcher.stop();
        }
        if let Some(handle) = self
            .poller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
        if let Some(handle) = self
            .store_db_worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = handle.join();
        }
        let _cycle = self
            .inner
            .cycle_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.release_writer_lock();
    }
}

#[derive(Default)]
struct StoreDbDebounceQueue {
    pending: BTreeMap<String, Instant>,
}

impl StoreDbDebounceQueue {
    fn schedule(&mut self, agent_id: String, deadline: Instant) {
        self.pending.insert(agent_id, deadline);
    }

    fn take_due(&mut self, now: Instant) -> Vec<String> {
        let due = self
            .pending
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(agent_id, _)| agent_id.clone())
            .collect::<Vec<_>>();
        for agent_id in &due {
            self.pending.remove(agent_id);
        }
        due
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().min().copied()
    }
}

fn run_store_db_debounce_loop(inner: Arc<ProductionBoxStoreSyncInner>) {
    loop {
        let due = {
            let mut queue = inner
                .store_db_debounce
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            loop {
                if inner.stopped.load(Ordering::Acquire) {
                    return;
                }
                let now = Instant::now();
                let due = queue.take_due(now);
                if !due.is_empty() {
                    break due;
                }
                queue = if let Some(deadline) = queue.next_deadline() {
                    let wait = deadline.saturating_duration_since(now);
                    let (queue, _) = inner
                        .store_db_wake
                        .wait_timeout(queue, wait)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    queue
                } else {
                    inner
                        .store_db_wake
                        .wait(queue)
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                };
            }
        };
        for agent_id in due {
            if inner.stopped.load(Ordering::Acquire) {
                return;
            }
            if let Err(error) = inner.run_local_agent_db_snapshot(&agent_id) {
                inner.log(&format!(
                    "turn-end store.db snapshot rejected for {agent_id}: {error}"
                ));
            }
        }
    }
}

struct ProductionBoxStoreSyncInner {
    deps: BoxStoreSyncExtensionDeps,
    env: BTreeMap<String, String>,
    mode: ProductionBoxStoreSyncMode,
    transfer: BoxStoreTransfer,
    started: AtomicBool,
    stopped: AtomicBool,
    cycle_lock: Mutex<()>,
    writer_lock: Mutex<Option<BoxStoreWriterLock>>,
    store_db_debounce: Mutex<StoreDbDebounceQueue>,
    store_db_wake: Condvar,
    agent_db_capture_queues: AgentDbCaptureQueues,
    object_store_provider: Mutex<Option<Arc<dyn BoxObjectStoreProvider>>>,
    last_idle_only_sync: Mutex<Instant>,
    flush_waiters: AtomicUsize,
    status: Mutex<ProductionBoxStoreSyncStatus>,
}

impl ProductionBoxStoreSyncInner {
    fn is_active_mode(&self) -> bool {
        matches!(
            self.mode,
            ProductionBoxStoreSyncMode::LocalFs { .. }
                | ProductionBoxStoreSyncMode::Native
                | ProductionBoxStoreSyncMode::AgentStore
                | ProductionBoxStoreSyncMode::SandBoxStoreV2
        )
    }

    fn resolve_store_id(&self) -> Result<String, String> {
        if let Some(store_id) = self
            .env
            .get(SAND_BOX_STORE_ID_ENV)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| is_frozen_agent_store_source_id(value))
        {
            return Ok(store_id.to_string());
        }
        (self.deps.resolve_store_id)()
    }

    fn resolve_object_store(&self) -> Result<(String, Arc<dyn BoxObjectStore>), String> {
        if !self.is_active_mode() {
            return Err("box-store backend is not active".into());
        }
        let store_id = self.resolve_store_id()?;
        let store = self.object_store_for_source_id(&store_id)?;
        Ok((store_id, store))
    }

    fn object_store_for_source_id(
        &self,
        source_id: &str,
    ) -> Result<Arc<dyn BoxObjectStore>, String> {
        if !is_frozen_agent_store_source_id(source_id) {
            return Err(format!(
                "resolved BoxStore source id is not a frozen AgentStore source id: {source_id}"
            ));
        }

        let provider = {
            let mut slot = self
                .object_store_provider
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.is_none() {
                *slot = Some(resolve_box_object_store_provider(
                    &self.env,
                    BoxObjectStoreProviderDependencies {
                        native_credentials: self.deps.native_credentials.clone(),
                        backend_url: self.deps.backend_url.clone(),
                        get_access_token: self.deps.get_access_token.clone(),
                        get_machine_id: self.deps.get_machine_id.clone(),
                    },
                )?);
            }
            slot.as_ref()
                .cloned()
                .ok_or_else(|| "BoxStore object-store provider failed to initialize".to_string())?
        };
        Ok(Arc::from(provider.for_store(source_id)))
    }

    fn log(&self, message: &str) {
        (self.deps.log)(&format!("[box-store-sync] {message}"));
    }

    fn diagnostic(&self, phase: &str, reason: &str, ok: bool) {
        let mut diagnostic = Map::new();
        diagnostic.insert("extension".into(), Value::String("box-store-sync".into()));
        diagnostic.insert("phase".into(), Value::String(phase.into()));
        diagnostic.insert("reason".into(), Value::String(reason.into()));
        diagnostic.insert("ok".into(), Value::Bool(ok));
        diagnostic.insert(
            "backend".into(),
            Value::String(mode_name(&self.mode).into()),
        );
        (self.deps.report_host_extension_diagnostic)(&diagnostic);
    }

    fn report_cycle_telemetry(&self, _phase: &str, summary: &BoxStoreCycleSummary) {
        let Some(telemetry) = box_store_sync_cycle_telemetry(summary) else {
            return;
        };
        (self.deps.report_box_store_sync_cycle)(telemetry.level, &telemetry.metadata);
    }

    fn report_manifest_write_conflict(&self, info: &BoxStoreManifestConflictInfo) {
        let mut metadata = BTreeMap::from([
            ("store_id".to_string(), info.store_id.clone()),
            ("attempts".to_string(), info.attempts.to_string()),
            ("accepted".to_string(), info.accepted.to_string()),
            ("covered".to_string(), info.covered.to_string()),
            (
                "canonical_matches_attempt".to_string(),
                info.canonical_matches_attempt.to_string(),
            ),
            (
                "live_view_changed".to_string(),
                info.live_view_changed.to_string(),
            ),
            (
                "attempted_entries".to_string(),
                info.attempted_entries.to_string(),
            ),
            (
                "canonical_readable".to_string(),
                info.canonical_readable.to_string(),
            ),
        ]);
        if let Some(value) = info.last_base_etag.as_ref() {
            metadata.insert("last_base_etag".into(), value.clone());
        }
        if let Some(value) = info.last_baseline_source.as_ref() {
            metadata.insert("last_baseline_source".into(), value.clone());
        }
        if let Some(value) = info.last_conflict_rel_path.as_ref() {
            metadata.insert("last_conflict_rel_path".into(), value.clone());
        }
        if let Some(value) = info.canonical_entry_count {
            metadata.insert("canonical_entry_count".into(), value.to_string());
        }
        if let Some(value) = info.canonical_updated_at_ms {
            metadata.insert("canonical_updated_at_ms".into(), value.to_string());
        }
        if let Some(value) = info.canonical_writer_window_id.as_ref() {
            metadata.insert("canonical_writer_window_id".into(), value.clone());
        }
        if let Some(value) = info.our_window_id.as_ref() {
            metadata.insert("our_window_id".into(), value.clone());
        }
        (self.deps.report_box_store_manifest_conflict)("warn", &metadata);
    }

    fn report_store_db_capture(
        &self,
        trigger: &'static str,
        outcome: StoreDbCaptureOutcome,
        capture: &ProductionStoreDbCaptureResult,
        is_committed: bool,
        store_id: Option<&str>,
        duration_ms: u64,
        queue_duration_ms: u64,
    ) {
        let telemetry = box_store_db_capture_telemetry(&BoxStoreDbCaptureTelemetrySummary {
            outcome,
            trigger,
            failure_phase: capture.capture_trace.failure_phase,
            is_committed,
            agent_count: capture.agent_count,
            files_scanned: capture.summary.files_scanned,
            files_uploaded: capture.summary.files_uploaded,
            bytes: capture.summary.bytes_uploaded,
            duration_ms,
            queue_duration_ms,
            capture_duration_ms: capture.capture_trace.capture_duration_ms,
            blob_upload_duration_ms: capture.capture_trace.blob_upload_duration_ms,
            manifest_commit_duration_ms: capture.capture_trace.manifest_commit_duration_ms,
            store_id: store_id.map(str::to_string),
        });
        (self.deps.report_box_store_db_capture)(telemetry.level, &telemetry.metadata);
    }

    fn report_empty_store_db_capture(
        &self,
        trigger: &'static str,
        outcome: StoreDbCaptureOutcome,
        failure_phase: Option<StoreDbCaptureFailurePhase>,
        agent_count: usize,
        duration_ms: u64,
        queue_duration_ms: u64,
    ) {
        let mut capture_trace = create_store_db_capture_trace();
        capture_trace.failure_phase = failure_phase;
        let capture = ProductionStoreDbCaptureResult {
            summary: CategoryTransferSummary {
                name: "store.db".into(),
                ..CategoryTransferSummary::default()
            },
            agent_count,
            capture_trace,
        };
        self.report_store_db_capture(
            trigger,
            outcome,
            &capture,
            false,
            None,
            duration_ms,
            queue_duration_ms,
        );
    }

    fn report_sync_lifecycle(&self, level: &str, ok: bool, reason: &str) {
        let metadata = BTreeMap::from([
            ("ok".to_string(), ok.to_string()),
            ("reason".to_string(), reason.to_string()),
            ("phase".to_string(), "startup".to_string()),
        ]);
        (self.deps.report_box_store_sync_cycle)(level, &metadata);
    }

    fn elapsed_since_idle_only_sync_ms(&self) -> u64 {
        self.last_idle_only_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    fn mark_idle_only_sync(&self) {
        *self
            .last_idle_only_sync
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    fn ensure_writer_lock(&self) -> Result<bool, String> {
        let mut slot = self
            .writer_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return Ok(true);
        }
        let lock_path = get_sand_root_dir().join(BOX_STORE_WRITER_LOCK_FILE_NAME);
        let window_id = format!("mahayana-host-{}", std::process::id());
        let lock = match BoxStoreWriterLock::try_acquire(&lock_path, &window_id) {
            Ok(Some(lock)) => lock,
            Ok(None) => return Ok(false),
            Err(error) => {
                self.log(&format!("writer lock error: {error}"));
                return Ok(false);
            }
        };
        *slot = Some(lock);
        Ok(true)
    }

    fn release_writer_lock(&self) {
        self.writer_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    fn record_cycle_skip(&self, reason: &str) {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.last_ok = Some(false);
        status.last_reason = Some(reason.to_string());
    }

    fn run_local_cycle(
        &self,
        chrome_only: bool,
        include_store_dbs: bool,
        skip_live_handle_store_dbs: bool,
        include_packs: bool,
        include_idle_only: bool,
        wait_for_in_flight: bool,
        accept_matching_canonical_on_conflict: bool,
    ) -> Result<(), String> {
        let cycle_started_at = Instant::now();
        let _cycle = if wait_for_in_flight {
            self.cycle_lock
                .lock()
                .map_err(|_| "box-store sync cycle lock poisoned".to_string())?
        } else {
            match self.cycle_lock.try_lock() {
                Ok(cycle) => cycle,
                Err(TryLockError::WouldBlock) => {
                    self.log("cycle skipped (in-flight)");
                    return Ok(());
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err("box-store sync cycle lock poisoned".into());
                }
            }
        };
        if self.stopped.load(Ordering::Acquire) {
            self.record_cycle_skip("stopped");
            return Ok(());
        }
        if !self.ensure_writer_lock()? {
            self.log("cycle skipped (locked)");
            self.record_cycle_skip("locked");
            let status = self
                .status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            self.report_cycle_telemetry(
                if chrome_only {
                    "chrome-session"
                } else {
                    "periodic"
                },
                &BoxStoreCycleSummary {
                    ok: false,
                    reason: Some("locked".into()),
                    store_id: None,
                    duration_ms: cycle_started_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                    categories: Vec::new(),
                    manifest_entries: status.last_manifest_entries,
                    store_db_entries: 0,
                    store_db_complete: false,
                    agent_dir_entries: 0,
                    total_files_uploaded: 0,
                    total_bytes_uploaded: 0,
                    total_failures: 0,
                    metadata_failures: 0,
                },
            );
            return Ok(());
        }
        let (store_id, store) = self.resolve_object_store()?;
        let manifest_v2_requested = self
            .env
            .get(SAND_MANIFEST_V2_ENV)
            .is_some_and(|value| value == "1");
        let sand_root = get_sand_root_dir();
        let hydration_marker_path = sand_root.join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME);
        let loaded = load_manifest_for_write(
            store.as_ref(),
            manifest_v2_requested,
            Some(&hydration_marker_path),
        )?;
        if loaded.invalid_nonblocking {
            self.log("manifest load failed, starting empty: manifest is invalid or unreadable");
        }
        let manifest_baseline = loaded.baseline;
        let manifest_v2 = loaded.manifest_v2;
        let fully_hydrated = loaded.fully_hydrated;
        let mut entries = loaded.manifest;
        let mut categories = Vec::new();
        let mut store_db_complete = false;
        let mut store_db_capture: Option<(Instant, ProductionStoreDbCaptureResult)> = None;

        if chrome_only {
            categories.push(sync_chrome_session(
                &self.transfer,
                store.as_ref(),
                &mut entries,
                manifest_v2,
                &self.deps.report_chrome_session_stage,
            )?);
        } else {
            let sand_data_excludes = sand_data_excludes();
            let sand_data_exclude_refs = sand_data_excludes
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            categories.push(self.transfer.sync_tree_category(
                store.as_ref(),
                &mut entries,
                &sand_root,
                SAND_DATA_REL_PREFIX,
                "sand-data",
                &sand_data_exclude_refs,
                None,
                true,
                manifest_v2,
            )?);
            let workspace_ignore =
                load_workspace_ignore(WORKSPACE_ROOT, SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS);
            categories.push(self.transfer.sync_tree_category(
                store.as_ref(),
                &mut entries,
                Path::new(WORKSPACE_ROOT),
                WORKSPACE_REL_PREFIX,
                "workspace",
                &[],
                Some(&workspace_ignore),
                false,
                manifest_v2,
            )?);
            categories.push(self.transfer.sync_tree_category(
                store.as_ref(),
                &mut entries,
                Path::new(CLI_CONFIG_ROOT),
                CLI_CONFIG_REL_PREFIX,
                "cli-config",
                CLI_CONFIG_EXCLUDES,
                None,
                false,
                manifest_v2,
            )?);
            categories.push(sync_chrome_session(
                &self.transfer,
                store.as_ref(),
                &mut entries,
                manifest_v2,
                &self.deps.report_chrome_session_stage,
            )?);
            for rel_dir in CHROME_AUTH_STATE_REL_DIRS {
                categories.push(sync_chrome_auth_state_category(
                    &self.transfer,
                    store.as_ref(),
                    &mut entries,
                    rel_dir,
                    manifest_v2,
                )?);
            }
            if include_idle_only {
                categories.push(sync_chrome_profile_category(
                    &self.transfer,
                    store.as_ref(),
                    &mut entries,
                    manifest_v2,
                )?);
            }
            if is_better_cli_home_enabled(
                self.env.get(SAND_STORE_BETTER_CLI_ENV).map(String::as_str),
                self.env.get(SAND_USER_NON_ROOT_ENV).map(String::as_str),
            ) {
                categories.push(sync_box_home_category(
                    &self.transfer,
                    store.as_ref(),
                    &mut entries,
                    manifest_v2,
                )?);
            }
            if include_store_dbs {
                let store_db_started_at = Instant::now();
                let capture = sync_store_db_snapshots_with_trace(
                    Arc::clone(&store),
                    &store_id,
                    &mut entries,
                    &sand_root,
                    skip_live_handle_store_dbs,
                    None,
                    self.env
                        .get(FABUSHI_OPENCLAW_PORTABLE_BACKUP_ENV)
                        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE")),
                )?;
                store_db_complete = capture.summary.failures == 0
                    && capture.summary.oversize == 0
                    && capture.summary.metadata_failures == 0;
                categories.push(capture.summary.clone());
                store_db_capture = Some((store_db_started_at, capture));
            }
        }

        let has_cycle_failures = categories
            .iter()
            .any(|category| category.failures > 0 || category.oversize > 0);
        let writer_window_id = format!("mahayana-host-{}", std::process::id());
        let manifest_commit_started_at = Instant::now();
        let manifest_result = write_manifest_with_retry_and_conflict_reporter(
            store.as_ref(),
            &store_id,
            manifest_baseline,
            &entries,
            manifest_v2,
            Some(&writer_window_id),
            fully_hydrated,
            ManifestSaveOptions {
                accept_matching_canonical_on_conflict,
                is_forced: has_cycle_failures,
                hydration_update: if !chrome_only
                    && hydration_marker_path.exists()
                    && include_idle_only
                    && include_store_dbs
                    && store_db_complete
                    && categories
                        .iter()
                        .all(|category| category.failures == 0 && category.oversize == 0)
                {
                    Some(ManifestHydrationUpdate::PromoteComplete)
                } else {
                    None
                },
            },
            BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
            BOX_STORE_MANIFEST_RETRY_DELAY_MS,
            &|message| self.log(message),
            &|| self.stopped.load(Ordering::Acquire),
            &|info| self.report_manifest_write_conflict(info),
        );
        if let Some((started_at, capture)) = store_db_capture.as_mut() {
            capture.capture_trace.manifest_commit_duration_ms = capture
                .capture_trace
                .manifest_commit_duration_ms
                .saturating_add(
                    manifest_commit_started_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                );
            if manifest_result.is_err() {
                record_store_db_capture_failure(
                    &mut capture.capture_trace,
                    StoreDbCaptureFailurePhase::ManifestCommit,
                );
                self.report_store_db_capture(
                    "flush",
                    StoreDbCaptureOutcome::Error,
                    capture,
                    false,
                    Some(&store_id),
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    0,
                );
            }
        }
        manifest_result?;

        if let Some((started_at, capture)) = store_db_capture.as_ref() {
            let outcome = aggregate_store_db_sweep_outcome(
                capture
                    .summary
                    .failures
                    .saturating_add(capture.summary.metadata_failures),
                capture.summary.oversize,
                capture.summary.files_uploaded,
                capture.summary.files_scanned,
            );
            let is_committed = store_db_complete
                && matches!(
                    outcome,
                    StoreDbCaptureOutcome::Uploaded | StoreDbCaptureOutcome::Unchanged
                );
            self.report_store_db_capture(
                "flush",
                outcome,
                capture,
                is_committed,
                Some(&store_id),
                started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                0,
            );
        }

        if !chrome_only
            && hydration_marker_path.exists()
            && include_idle_only
            && include_store_dbs
            && store_db_complete
            && categories
                .iter()
                .all(|category| category.failures == 0 && category.oversize == 0)
        {
            if let Err(error) = remove_hydration_handoff_marker(&hydration_marker_path) {
                self.log(&format!("hydration handoff marker cleanup failed: {error}"));
            }
        }

        if include_packs
            && is_box_store_pack_build_enabled(
                self.env.get("SAND_BOX_STORE_PACKS").map(String::as_str),
            )
        {
            let pack_summary = sync_local_packs(store.as_ref(), &entries, || {
                self.flush_waiters.load(Ordering::Acquire) > 0
            });
            if pack_summary.failures > 0 {
                self.log("pack maintenance round failed; preserving the committed manifest and retrying later");
            }
            categories.push(pack_summary);
        }

        let files_uploaded = categories.iter().map(|value| value.files_uploaded).sum();
        let bytes_uploaded = categories.iter().map(|value| value.bytes_uploaded).sum();
        let total_failures = categories
            .iter()
            .map(|value| value.failures + value.oversize)
            .sum::<usize>();
        let metadata_failures = categories
            .iter()
            .map(|value| value.metadata_failures)
            .sum::<usize>();
        let failures = total_failures.saturating_add(metadata_failures);
        let ok = failures == 0;
        {
            let mut status = self
                .status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            status.last_ok = Some(ok);
            status.last_reason = (!ok).then(|| "category-failures".to_string());
            status.last_manifest_entries = entries.len();
            status.last_files_uploaded = files_uploaded;
            status.last_bytes_uploaded = bytes_uploaded;
        }
        self.log(&format!(
            "cycle {}: {} file(s) / {}B uploaded, {} manifest entries, {} failure(s)",
            if chrome_only { "chrome" } else { "periodic" },
            files_uploaded,
            bytes_uploaded,
            entries.len(),
            failures,
        ));
        let summary = BoxStoreCycleSummary {
            ok: true,
            reason: None,
            store_id: Some(store_id),
            duration_ms: cycle_started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            categories,
            manifest_entries: entries.len(),
            store_db_entries: count_store_db_manifest_entries(Some(&entries)),
            store_db_complete,
            agent_dir_entries: count_agent_dir_manifest_entries(Some(&entries)),
            total_files_uploaded: files_uploaded,
            total_bytes_uploaded: bytes_uploaded,
            total_failures,
            metadata_failures,
        };
        self.report_cycle_telemetry(
            if chrome_only {
                "chrome-session"
            } else {
                "periodic"
            },
            &summary,
        );
        Ok(())
    }

    fn legacy_agent_store_for_source_id(
        &self,
        source_id: &str,
    ) -> Result<Arc<dyn BoxObjectStore>, String> {
        let provider =
            AgentStoreObjectStoreProvider::new(AgentStoreClientDependencies {
                backend_url: self
                    .deps
                    .backend_url
                    .clone()
                    .ok_or_else(|| "AgentStore backend URL is not configured".to_string())?,
                get_access_token: self.deps.get_access_token.clone().ok_or_else(|| {
                    "AgentStore auth token resolver is not configured".to_string()
                })?,
                get_machine_id: self.deps.get_machine_id.clone().ok_or_else(|| {
                    "AgentStore machine id resolver is not configured".to_string()
                })?,
            })?;
        Ok(Arc::from(provider.for_store(source_id)))
    }

    fn read_store_status(&self) -> ProductionBoxStoreDurableStatus {
        let result = (|| -> Result<ProductionBoxStoreDurableStatus, String> {
            let store_id = self.resolve_store_id()?;
            let store = self.object_store_for_source_id(&store_id)?;
            let Some(manifest) = read_manifest_strict(store.as_ref())? else {
                return Ok(ProductionBoxStoreDurableStatus::default());
            };
            let total_bytes = manifest
                .entries
                .values()
                .map(|entry| match entry {
                    BoxStoreManifestEntry::LegacyFile { size, .. }
                    | BoxStoreManifestEntry::File { size, .. } => *size,
                    BoxStoreManifestEntry::Symlink { .. } => 0,
                })
                .sum();
            Ok(ProductionBoxStoreDurableStatus {
                durable: !manifest.entries.is_empty(),
                fully_hydrated: manifest.fully_hydrated,
                entry_count: manifest.entries.len(),
                store_db_entries: count_store_db_manifest_entries(Some(&manifest.entries)),
                agent_dir_entries: count_agent_dir_manifest_entries(Some(&manifest.entries)),
                total_bytes,
                last_snapshot_at_ms: manifest.updated_at_ms,
            })
        })();
        match result {
            Ok(status) => status,
            Err(error) => {
                self.log(&format!("readStoreStatus failed: {error}"));
                ProductionBoxStoreDurableStatus::default()
            }
        }
    }

    fn clear_store_now(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        self.store_db_debounce
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .clear();
        self.store_db_wake.notify_all();

        let _cycle = self
            .cycle_lock
            .lock()
            .map_err(|_| "box-store sync cycle lock poisoned".to_string())?;
        if !self.ensure_writer_lock()? {
            return Err("locked".into());
        }
        let store_id = self.resolve_store_id()?;
        if matches!(self.mode, ProductionBoxStoreSyncMode::SandBoxStoreV2) {
            let legacy = self.legacy_agent_store_for_source_id(&store_id)?;
            self.reset_canonical_store(&store_id, legacy.as_ref())
                .map_err(|error| format!("legacy store clear failed: {error}"))?;
        }
        let store = self.object_store_for_source_id(&store_id)?;
        self.reset_canonical_store(&store_id, store.as_ref())?;
        self.transfer.clear_local_stat();
        Ok(())
    }

    fn reset_canonical_store(
        &self,
        store_id: &str,
        store: &dyn BoxObjectStore,
    ) -> Result<(), String> {
        prepare_canonical_manifest_reset(store)?;
        let baseline = store.get(BOX_STORE_MANIFEST_REL_PATH)?;
        let marker_path = get_sand_root_dir().join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME);
        remove_hydration_handoff_marker(&marker_path).map_err(|error| error.to_string())?;
        let manifest_v2 = self
            .env
            .get(SAND_MANIFEST_V2_ENV)
            .is_some_and(|value| value == "1");
        let writer_window_id = format!("mahayana-host-{}", std::process::id());
        write_manifest_with_retry_and_conflict_reporter(
            store,
            store_id,
            baseline,
            &BoxManifestMap::new(),
            manifest_v2,
            Some(&writer_window_id),
            None,
            ManifestSaveOptions {
                hydration_update: Some(ManifestHydrationUpdate::ResetComplete),
                ..ManifestSaveOptions::default()
            },
            BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
            BOX_STORE_MANIFEST_RETRY_DELAY_MS,
            &|message| self.log(message),
            &|| false,
            &|info| self.report_manifest_write_conflict(info),
        )?;
        Ok(())
    }

    fn forget_agent(&self, agent_id: &str) -> Result<(), String> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Ok(());
        }
        self.store_db_debounce
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .remove(agent_id);

        let _cycle = self
            .cycle_lock
            .lock()
            .map_err(|_| "box-store sync cycle lock poisoned".to_string())?;
        if !self.ensure_writer_lock()? {
            return Err("locked".into());
        }
        let store_id = self.resolve_store_id()?;
        let store = self.object_store_for_source_id(&store_id)?;
        let marker_path = get_sand_root_dir().join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME);
        let loaded = load_manifest_for_write(
            store.as_ref(),
            self.env
                .get(SAND_MANIFEST_V2_ENV)
                .is_some_and(|value| value == "1"),
            Some(&marker_path),
        )?;
        let mut entries = loaded.manifest;
        let needle = format!("/agents/{agent_id}/");
        let own_prefix = format!("agents/{agent_id}/");
        let before = entries.len();
        let removed_paths = entries
            .keys()
            .filter(|path| path.contains(&needle) || path.starts_with(&own_prefix))
            .cloned()
            .collect::<Vec<_>>();
        for path in &removed_paths {
            entries.remove(path);
            self.transfer.forget_local_stat_path(path);
        }
        if entries.len() == before {
            return Ok(());
        }
        let writer_window_id = format!("mahayana-host-{}", std::process::id());
        write_manifest_with_retry_and_conflict_reporter(
            store.as_ref(),
            &store_id,
            loaded.baseline,
            &entries,
            loaded.manifest_v2,
            Some(&writer_window_id),
            loaded.fully_hydrated,
            ManifestSaveOptions::default(),
            BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
            BOX_STORE_MANIFEST_RETRY_DELAY_MS,
            &|message| self.log(message),
            &|| false,
            &|info| self.report_manifest_write_conflict(info),
        )?;
        self.log(&format!(
            "forgot {agent_id} ({} entries)",
            before.saturating_sub(entries.len())
        ));
        Ok(())
    }

    fn run_local_agent_db_snapshot(&self, agent_id: &str) -> Result<(), String> {
        self.agent_db_capture_queues
            .run_serialized_with_queue_duration(agent_id, |queue_duration_ms| {
                self.run_local_agent_db_snapshot_unqueued(agent_id, queue_duration_ms)
            })
    }

    fn run_local_agent_db_snapshot_unqueued(
        &self,
        agent_id: &str,
        queue_duration_ms: u64,
    ) -> Result<(), String> {
        let started_at = Instant::now();
        let _cycle = match self.cycle_lock.lock() {
            Ok(cycle) => cycle,
            Err(_) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    Some(StoreDbCaptureFailurePhase::Capture),
                    1,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err("box-store sync cycle lock poisoned".into());
            }
        };
        if self.stopped.load(Ordering::Acquire) {
            self.report_empty_store_db_capture(
                "turn_end",
                StoreDbCaptureOutcome::Skipped,
                None,
                0,
                started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                queue_duration_ms,
            );
            return Ok(());
        }
        match self.ensure_writer_lock() {
            Ok(true) => {}
            Ok(false) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Skipped,
                    None,
                    0,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Ok(());
            }
            Err(error) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    Some(StoreDbCaptureFailurePhase::Capture),
                    1,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err(error);
            }
        }
        let (store_id, store) = match self.resolve_object_store() {
            Ok(resolved) => resolved,
            Err(error) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    Some(StoreDbCaptureFailurePhase::Capture),
                    1,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err(error);
            }
        };
        let manifest_v2_requested = self
            .env
            .get(SAND_MANIFEST_V2_ENV)
            .is_some_and(|value| value == "1");
        let sand_root = get_sand_root_dir();
        let hydration_marker_path = sand_root.join(BOX_STORE_HYDRATION_HANDOFF_FILE_NAME);
        let loaded = match load_manifest_for_write(
            store.as_ref(),
            manifest_v2_requested,
            Some(&hydration_marker_path),
        ) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    Some(StoreDbCaptureFailurePhase::Capture),
                    1,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err(error);
            }
        };
        if loaded.invalid_nonblocking {
            self.log("manifest load failed, starting empty: manifest is invalid or unreadable");
        }
        let manifest_baseline = loaded.baseline;
        let manifest_v2 = loaded.manifest_v2;
        let fully_hydrated = loaded.fully_hydrated;
        let mut entries = loaded.manifest;
        let entries_before = entries.clone();
        let mut capture = match sync_store_db_snapshots_with_trace(
            store.clone(),
            &store_id,
            &mut entries,
            &sand_root,
            false,
            Some(agent_id),
            self.env
                .get(FABUSHI_OPENCLAW_PORTABLE_BACKUP_ENV)
                .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE")),
        ) {
            Ok(capture) => capture,
            Err(error) => {
                self.report_empty_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    Some(StoreDbCaptureFailurePhase::Capture),
                    1,
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err(error);
            }
        };

        let mut manifest_committed = true;
        if entries != entries_before {
            let writer_window_id = format!("mahayana-host-{}", std::process::id());
            let manifest_commit_started_at = Instant::now();
            let manifest_result = write_manifest_with_retry_and_conflict_reporter(
                store.as_ref(),
                &store_id,
                manifest_baseline,
                &entries,
                manifest_v2,
                Some(&writer_window_id),
                fully_hydrated,
                ManifestSaveOptions::default(),
                BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
                BOX_STORE_MANIFEST_RETRY_DELAY_MS,
                &|message| self.log(message),
                &|| self.stopped.load(Ordering::Acquire),
                &|info| self.report_manifest_write_conflict(info),
            );
            capture.capture_trace.manifest_commit_duration_ms = capture
                .capture_trace
                .manifest_commit_duration_ms
                .saturating_add(
                    manifest_commit_started_at
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64,
                );
            if let Err(error) = manifest_result {
                manifest_committed = false;
                record_store_db_capture_failure(
                    &mut capture.capture_trace,
                    StoreDbCaptureFailurePhase::ManifestCommit,
                );
                self.report_store_db_capture(
                    "turn_end",
                    StoreDbCaptureOutcome::Error,
                    &capture,
                    false,
                    Some(&store_id),
                    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                    queue_duration_ms,
                );
                return Err(error);
            }
        }

        let outcome = aggregate_store_db_sweep_outcome(
            capture
                .summary
                .failures
                .saturating_add(capture.summary.metadata_failures),
            capture.summary.oversize,
            capture.summary.files_uploaded,
            capture.summary.files_scanned,
        );
        capture.agent_count = if outcome == StoreDbCaptureOutcome::Skipped {
            0
        } else {
            1
        };
        let is_committed = manifest_committed
            && matches!(
                outcome,
                StoreDbCaptureOutcome::Uploaded | StoreDbCaptureOutcome::Unchanged
            );
        self.report_store_db_capture(
            "turn_end",
            outcome,
            &capture,
            is_committed,
            Some(&store_id),
            started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            queue_duration_ms,
        );

        let failures =
            capture.summary.failures + capture.summary.oversize + capture.summary.metadata_failures;
        self.log(&format!(
            "turn-end store.db {agent_id}: {} file(s) / {}B uploaded, {} failure(s)",
            capture.summary.files_uploaded, capture.summary.bytes_uploaded, failures
        ));
        Ok(())
    }
}

pub fn resolve_production_box_store_sync_mode(
    env: &BTreeMap<String, String>,
) -> ProductionBoxStoreSyncMode {
    if !is_box_store_sync_enabled(env) {
        return ProductionBoxStoreSyncMode::Disabled;
    }
    let policy = resolve_box_store_backend_policy(env);
    match policy.kind {
        BoxStoreBackendKind::Native => ProductionBoxStoreSyncMode::Native,
        BoxStoreBackendKind::LocalFs => {
            let Some(base_dir) = policy.local_dir else {
                return ProductionBoxStoreSyncMode::InvalidLocalConfiguration {
                    reason: "local-fs backend requires an absolute SAND_BOX_STORE_LOCAL_DIR".into(),
                };
            };
            let store_id_override = env
                .get(SAND_BOX_STORE_ID_ENV)
                .map(String::as_str)
                .map(str::trim)
                .filter(|value| is_frozen_agent_store_source_id(value))
                .map(str::to_string);
            ProductionBoxStoreSyncMode::LocalFs {
                base_dir,
                store_id_override,
            }
        }
        BoxStoreBackendKind::SandBoxStoreV2 => ProductionBoxStoreSyncMode::SandBoxStoreV2,
        BoxStoreBackendKind::AgentStore => ProductionBoxStoreSyncMode::AgentStore,
    }
}

fn is_frozen_agent_store_source_id(value: &str) -> bool {
    fn valid_uuid(value: &str) -> bool {
        let bytes = value.as_bytes();
        if bytes.len() != 36
            || bytes[8] != b'-'
            || bytes[13] != b'-'
            || bytes[18] != b'-'
            || bytes[23] != b'-'
        {
            return false;
        }
        if !matches!(bytes[14], b'1'..=b'5') {
            return false;
        }
        if !matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b') {
            return false;
        }
        bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
    }

    if valid_uuid(value) {
        return true;
    }
    let Some(rest) = value
        .strip_prefix("bc-")
        .or_else(|| value.strip_prefix("BC-"))
        .or_else(|| value.strip_prefix("Bc-"))
        .or_else(|| value.strip_prefix("bC-"))
    else {
        return false;
    };
    if rest.len() == 36 {
        return valid_uuid(rest);
    }
    if rest.len() <= 37 {
        return false;
    }
    let split = rest.len() - 36;
    let (prefix_with_dash, uuid) = rest.split_at(split);
    let Some(prefix) = prefix_with_dash.strip_suffix('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.bytes().enumerate().all(|(index, byte)| {
            let lowered = byte.to_ascii_lowercase();
            (index == 0 && (lowered.is_ascii_lowercase() || lowered.is_ascii_digit()))
                || (index > 0
                    && (lowered.is_ascii_lowercase()
                        || lowered.is_ascii_digit()
                        || lowered == b'-'))
        })
        && valid_uuid(uuid)
}

fn mode_name(mode: &ProductionBoxStoreSyncMode) -> &'static str {
    match mode {
        ProductionBoxStoreSyncMode::Disabled => "disabled",
        ProductionBoxStoreSyncMode::Native => "fabushi-native",
        ProductionBoxStoreSyncMode::LocalFs { .. } => "local-fs",
        ProductionBoxStoreSyncMode::AgentStore => "agent-store",
        ProductionBoxStoreSyncMode::SandBoxStoreV2 => "sand-box-store-v2",
        ProductionBoxStoreSyncMode::UnsupportedRemote {
            backend: BoxStoreBackendKind::AgentStore,
        } => "agent-store-invalid",
        ProductionBoxStoreSyncMode::UnsupportedRemote {
            backend: BoxStoreBackendKind::SandBoxStoreV2,
        } => "sand-box-store-v2-invalid",
        ProductionBoxStoreSyncMode::UnsupportedRemote {
            backend: BoxStoreBackendKind::LocalFs,
        } => "local-fs-unwired",
        ProductionBoxStoreSyncMode::UnsupportedRemote {
            backend: BoxStoreBackendKind::Native,
        } => "fabushi-native-invalid",
        ProductionBoxStoreSyncMode::InvalidLocalConfiguration { .. } => "local-fs-invalid",
    }
}

fn load_manifest(store: &dyn BoxObjectStore) -> Result<Option<BoxStoreManifest>, String> {
    read_manifest_strict(store)
}

fn write_manifest_if_unchanged(
    store: &dyn BoxObjectStore,
    baseline: Option<&[u8]>,
    entries: &BoxManifestMap,
    manifest_v2: bool,
    writer_window_id: String,
) -> Result<(), String> {
    write_manifest_with_retry(
        store,
        "production-test",
        baseline.map(|value| value.to_vec()),
        entries,
        manifest_v2,
        Some(&writer_window_id),
        None,
        ManifestSaveOptions {
            is_forced: true,
            ..ManifestSaveOptions::default()
        },
        1,
        0,
        &|_| {},
        &|| false,
    )
    .map(|_| ())
}

fn write_manifest(
    store: &dyn BoxObjectStore,
    entries: &BoxManifestMap,
    manifest_v2: bool,
    writer_window_id: String,
) -> Result<(), String> {
    let bytes = serialize_manifest_bytes(
        entries,
        manifest_v2,
        Some(&writer_window_id),
        None,
        now_ms(),
    )?;
    store.put(BOX_STORE_MANIFEST_REL_PATH, &bytes)
}

fn sand_data_excludes() -> Vec<String> {
    let mut excludes = SAND_DATA_EXCLUDES
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    excludes.extend(
        BOX_STORE_SAND_DATA_EXCLUDED_FILE_NAMES
            .iter()
            .map(|name| format!("{SAND_DATA_REL_PREFIX}/{name}")),
    );
    excludes
}

fn snapshot_transfer_roots(env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut roots = vec![
        get_sand_root_dir(),
        PathBuf::from(WORKSPACE_ROOT),
        PathBuf::from(CLI_CONFIG_ROOT),
        PathBuf::from("/home/box/chrome-profile"),
    ];
    if is_better_cli_home_enabled(
        env.get(SAND_STORE_BETTER_CLI_ENV).map(String::as_str),
        env.get(SAND_USER_NON_ROOT_ENV).map(String::as_str),
    ) {
        roots.push(PathBuf::from(BOX_HOME_DIR));
    }
    roots
}
fn sync_chrome_auth_state_category(
    transfer: &BoxStoreTransfer,
    store: &dyn BoxObjectStore,
    manifest: &mut BoxManifestMap,
    rel_dir: &str,
    manifest_v2: bool,
) -> Result<CategoryTransferSummary, String> {
    let root = Path::new(CHROME_SESSION_DB_DIR).join(rel_dir);
    let rel_prefix = format!("{CHROME_SESSION_DB_REL_DIR}/{rel_dir}");
    let excludes = CHROME_AUTH_STATE_CACHE_EXCLUDE_NAMES
        .iter()
        .map(|cache| format!("{rel_prefix}/{cache}"))
        .collect::<Vec<_>>();
    let exclude_refs = excludes.iter().map(String::as_str).collect::<Vec<_>>();
    transfer.sync_tree_category(
        store,
        manifest,
        &root,
        &rel_prefix,
        &format!("chrome-{}", rel_dir.to_ascii_lowercase().replace(' ', "-")),
        &exclude_refs,
        None,
        false,
        manifest_v2,
    )
}

fn sync_chrome_profile_category(
    transfer: &BoxStoreTransfer,
    store: &dyn BoxObjectStore,
    manifest: &mut BoxManifestMap,
    manifest_v2: bool,
) -> Result<CategoryTransferSummary, String> {
    let mut excludes = vec![
        "home/box/chrome-profile/*/Cache".to_string(),
        "home/box/chrome-profile/*/Code Cache".to_string(),
        "home/box/chrome-profile/*/GPUCache".to_string(),
        "home/box/chrome-profile/*/Service Worker/CacheStorage".to_string(),
    ];
    for name in CHROME_SESSION_DB_NAMES {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            excludes.push(format!("{CHROME_SESSION_DB_REL_DIR}/{name}{suffix}"));
        }
    }
    excludes.extend(
        CHROME_AUTH_STATE_REL_DIRS
            .iter()
            .map(|dir| format!("{CHROME_SESSION_DB_REL_DIR}/{dir}")),
    );
    let exclude_refs = excludes.iter().map(String::as_str).collect::<Vec<_>>();
    transfer.sync_tree_category(
        store,
        manifest,
        Path::new("/home/box/chrome-profile"),
        "home/box/chrome-profile",
        "chrome-profile",
        &exclude_refs,
        None,
        false,
        manifest_v2,
    )
}

fn sync_box_home_category(
    transfer: &BoxStoreTransfer,
    store: &dyn BoxObjectStore,
    manifest: &mut BoxManifestMap,
    manifest_v2: bool,
) -> Result<CategoryTransferSummary, String> {
    let category = build_box_home_category(BOX_HOME_DIR);
    let ignore = build_box_home_ignore();
    let exclude_refs = category
        .excludes
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    transfer.sync_tree_category(
        store,
        manifest,
        Path::new(BOX_HOME_DIR),
        BOX_HOME_REL_PREFIX,
        &category.name,
        &exclude_refs,
        Some(&ignore),
        false,
        manifest_v2,
    )
}

fn sync_chrome_session(
    transfer: &BoxStoreTransfer,
    store: &dyn BoxObjectStore,
    manifest: &mut BoxManifestMap,
    manifest_v2: bool,
    report_chrome_session_stage: &BoxStoreSyncTelemetryReporter,
) -> Result<CategoryTransferSummary, String> {
    let staged = match stage_box_chrome_session_with_report(|report| {
        let telemetry = chrome_session_stage_telemetry(&report);
        report_chrome_session_stage(telemetry.level, &telemetry.metadata);
    }) {
        Ok(staged) => staged,
        Err(error) => {
            transfer.log_message(&format!("stage chrome-session failed: {error}"));
            return Ok(CategoryTransferSummary {
                name: "chrome-session".into(),
                ..CategoryTransferSummary::default()
            });
        }
    };
    let files = staged
        .files
        .iter()
        .map(|file| StagedTransferFile {
            abs_path: file.abs_path.clone(),
            rel_path: file.rel_path.clone(),
            mode: file.mode,
        })
        .collect::<Vec<_>>();
    let result = transfer.sync_staged_files(
        store,
        manifest,
        "chrome-session",
        &files,
        staged.skipped,
        manifest_v2,
    );
    if let Err(error) = staged.cleanup() {
        transfer.log_message(&format!("stage cleanup chrome-session failed: {error}"));
    }
    result
}

#[derive(Clone)]
struct ProductionStoreDbSnapshotRuntime {
    store: Arc<dyn BoxObjectStore>,
}

impl StoreDbSnapshotRuntimePort for ProductionStoreDbSnapshotRuntime {
    fn put_bytes(
        &self,
        _store_id: &str,
        key: &str,
        bytes: &[u8],
        _content_addressed: bool,
    ) -> Result<(), String> {
        self.store.put(key, bytes)
    }

    fn put_from_file(
        &self,
        _store_id: &str,
        key: &str,
        path: &Path,
        _sha: &str,
        _size: u64,
    ) -> Result<(), String> {
        self.store.put_from_file(key, path)
    }
}

fn resolve_pack_source_path(rel_path: &str) -> Option<PathBuf> {
    for (prefix, root) in [
        (SAND_DATA_REL_PREFIX, get_sand_root_dir()),
        (WORKSPACE_REL_PREFIX, PathBuf::from(WORKSPACE_ROOT)),
        (CLI_CONFIG_REL_PREFIX, PathBuf::from(CLI_CONFIG_ROOT)),
    ] {
        if rel_path == prefix {
            return Some(root);
        }
        if let Some(suffix) = rel_path.strip_prefix(&format!("{prefix}/")) {
            return Some(root.join(suffix));
        }
    }
    None
}

fn sweep_pack_temp_dir(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age.as_millis() as u64 > PACK_TMP_MAX_AGE_MS)
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn sync_local_packs<F>(
    store: &dyn BoxObjectStore,
    manifest: &BoxManifestMap,
    should_abort: F,
) -> CategoryTransferSummary
where
    F: Fn() -> bool,
{
    let mut summary = CategoryTransferSummary {
        name: "packs".into(),
        ..CategoryTransferSummary::default()
    };
    let result = (|| -> Result<(), String> {
        let mut live = HashMap::<String, u64>::new();
        let mut eligible = HashMap::<String, u64>::new();
        let mut local_by_sha = HashMap::<String, PathBuf>::new();

        for (rel_path, entry) in manifest {
            let (sha, size) = match entry {
                BoxStoreManifestEntry::LegacyFile { sha, size }
                | BoxStoreManifestEntry::File { sha, size, .. } => (sha, *size),
                BoxStoreManifestEntry::Symlink { .. } => continue,
            };
            live.insert(sha.clone(), size);
            if size >= PACK_MEMBER_MAX_BYTES || eligible.contains_key(sha) {
                continue;
            }
            let Some(abs_path) = resolve_pack_source_path(rel_path) else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(&abs_path) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() != size {
                continue;
            }
            eligible.insert(sha.clone(), size);
            local_by_sha.insert(sha.clone(), abs_path);
        }

        let raw_index = store.get(BOX_STORE_PACK_INDEX_KEY)?;
        let index = raw_index
            .as_deref()
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .and_then(parse_pack_index);
        let plan = plan_pack_maintenance(
            index.as_ref(),
            &live,
            &eligible,
            PACK_MAX_MEMBER_SIZE_SUM,
            PACK_BUILD_MIN_MEMBERS,
            PACK_BUILD_MIN_BYTES,
        );
        summary.skipped_unchanged = plan.kept_packs.len();
        summary.removed = plan.retired_pack_ids.len();
        summary.files_scanned = plan.new_packs.len();
        if plan.new_packs.is_empty() && plan.retired_pack_ids.is_empty() {
            return Ok(());
        }

        let tmp_dir = std::env::temp_dir().join(PACK_TMP_DIR_NAME);
        fs::create_dir_all(&tmp_dir).map_err(|error| error.to_string())?;
        sweep_pack_temp_dir(&tmp_dir);
        let mut built_packs = Vec::<PackEntry>::new();

        for (group_index, planned) in plan.new_packs.iter().enumerate() {
            if should_abort() {
                return Ok(());
            }
            let sources = planned
                .iter()
                .filter_map(|member| {
                    local_by_sha.get(&member.sha).map(|abs_path| PackSource {
                        abs_path: abs_path.clone(),
                        sha: member.sha.clone(),
                        size: member.size,
                        vmtime: member.vmtime,
                    })
                })
                .collect::<Vec<_>>();
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let tmp_path = tmp_dir.join(format!(
                "build-{}-{nonce}-{group_index}",
                std::process::id()
            ));
            let built = match build_pack_file(&tmp_path, &sources, || should_abort()) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Ok(());
                }
                Err(error) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Err(error);
                }
            };
            let built_size_sum = built.members.iter().map(|member| member.size).sum::<u64>();
            if built.members.is_empty()
                || (built.members.len() < PACK_BUILD_MIN_MEMBERS
                    && built_size_sum < PACK_BUILD_MIN_BYTES)
            {
                let _ = fs::remove_file(&tmp_path);
                continue;
            }

            let pack_id = sha256_file(&tmp_path)?;
            let key = format!("{BOX_STORE_PACKS_PREFIX}/{pack_id}");
            let upload = store.put_from_file(&key, &tmp_path);
            let _ = fs::remove_file(&tmp_path);
            upload?;
            summary.files_uploaded += 1;
            summary.bytes_uploaded = summary.bytes_uploaded.saturating_add(built.file_bytes);
            built_packs.push(PackEntry {
                id: pack_id,
                bytes: built.file_bytes,
                members: built.members,
            });
        }

        if should_abort() {
            return Ok(());
        }
        if built_packs.is_empty() && plan.retired_pack_ids.is_empty() {
            return Ok(());
        }

        let max_built_vmtime = built_packs
            .iter()
            .flat_map(|pack| pack.members.iter().map(|member| member.vmtime))
            .max()
            .unwrap_or(0);
        let next_index = PackIndex {
            version: PACK_INDEX_VERSION,
            max_vmtime: index
                .as_ref()
                .map(|index| index.max_vmtime)
                .unwrap_or(0)
                .max(max_built_vmtime),
            packs: plan.kept_packs.iter().cloned().chain(built_packs).collect(),
        };
        let referenced = next_index
            .packs
            .iter()
            .map(|pack| pack.id.as_str())
            .collect::<HashSet<_>>();
        let retiring = plan
            .retired_pack_ids
            .iter()
            .filter(|id| !referenced.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        let raw_retired = store.get(BOX_STORE_PACK_RETIRED_KEY)?;
        let existing_retired = match raw_retired.as_deref() {
            None => Vec::new(),
            Some(raw) => {
                let text = match std::str::from_utf8(raw) {
                    Ok(value) => value,
                    Err(_) if retiring.is_empty() => "",
                    Err(_) => {
                        return Err(
                            "packs/retired.json is present but unreadable; refusing to overwrite"
                                .into(),
                        );
                    }
                };
                match parse_pack_retired(text) {
                    Some(value) => value,
                    None if retiring.is_empty() => Vec::new(),
                    None => {
                        return Err(
                            "packs/retired.json is present but unreadable; refusing to overwrite"
                                .into(),
                        );
                    }
                }
            }
        };
        let mut merged_retired = existing_retired.clone();
        for pack_id in &retiring {
            if !merged_retired.contains(pack_id) {
                merged_retired.push(pack_id.clone());
            }
        }
        merged_retired.retain(|id| !referenced.contains(id.as_str()));
        if merged_retired != existing_retired {
            let bytes =
                serialize_pack_retired(&merged_retired).map_err(|error| error.to_string())?;
            store.put(BOX_STORE_PACK_RETIRED_KEY, bytes.as_bytes())?;
        }

        let index_bytes = serialize_pack_index(&next_index).map_err(|error| error.to_string())?;
        store.put(BOX_STORE_PACK_INDEX_KEY, index_bytes.as_bytes())?;
        for pack_id in retiring {
            let _ = store.delete(&format!("{BOX_STORE_PACKS_PREFIX}/{pack_id}"));
        }
        Ok(())
    })();
    if result.is_err() {
        summary.failures += 1;
    }
    summary
}

#[derive(Debug, Clone)]
struct ProductionStoreDbCaptureResult {
    summary: CategoryTransferSummary,
    agent_count: usize,
    capture_trace: StoreDbCaptureTrace,
}

fn sync_store_db_snapshots(
    store: Arc<dyn BoxObjectStore>,
    store_id: &str,
    manifest: &mut BoxManifestMap,
    sand_root: &Path,
    skip_live_handles: bool,
    only_agent_id: Option<&str>,
) -> Result<CategoryTransferSummary, String> {
    sync_store_db_snapshots_with_trace(
        store,
        store_id,
        manifest,
        sand_root,
        skip_live_handles,
        only_agent_id,
        openclaw_portable_backup_enabled(),
    )
    .map(|capture| capture.summary)
}

fn sync_store_db_snapshots_with_options(
    store: Arc<dyn BoxObjectStore>,
    store_id: &str,
    manifest: &mut BoxManifestMap,
    sand_root: &Path,
    skip_live_handles: bool,
    only_agent_id: Option<&str>,
    portable_backup_enabled: bool,
) -> Result<CategoryTransferSummary, String> {
    sync_store_db_snapshots_with_trace(
        store,
        store_id,
        manifest,
        sand_root,
        skip_live_handles,
        only_agent_id,
        portable_backup_enabled,
    )
    .map(|capture| capture.summary)
}

fn sync_store_db_snapshots_with_trace(
    store: Arc<dyn BoxObjectStore>,
    store_id: &str,
    manifest: &mut BoxManifestMap,
    sand_root: &Path,
    skip_live_handles: bool,
    only_agent_id: Option<&str>,
    portable_backup_enabled: bool,
) -> Result<ProductionStoreDbCaptureResult, String> {
    let mut capture_trace = create_store_db_capture_trace();
    let mut agent_count = 0_usize;
    let mut summary = CategoryTransferSummary {
        name: "store.db".into(),
        ..CategoryTransferSummary::default()
    };
    let agents_root = sand_root.join("agents");
    let mut seen = HashSet::new();
    let mut blocked_from_prune = HashSet::new();
    let mut walk_complete = true;
    let agent_dirs = match fs::read_dir(&agents_root) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            summary.metadata_failures += 1;
            record_store_db_capture_failure(
                &mut capture_trace,
                StoreDbCaptureFailurePhase::Capture,
            );
            record_store_db_capture_failure(
                &mut capture_trace,
                StoreDbCaptureFailurePhase::Capture,
            );
            walk_complete = false;
            None
        }
    };

    let runtime: Arc<dyn StoreDbSnapshotRuntimePort> =
        Arc::new(ProductionStoreDbSnapshotRuntime { store });
    let uploader = StoreDbSnapshotUpload::new(
        runtime,
        Arc::new(now_ms),
        Arc::new(|message| eprintln!("[box-store-sync] {message}")),
        DEFAULT_MAX_OBJECT_BYTES,
        LARGE_OBJECT_THRESHOLD_BYTES,
    );

    if let Some(agent_dirs) = agent_dirs {
        for agent_entry in agent_dirs {
            let agent_entry = match agent_entry {
                Ok(entry) => entry,
                Err(_) => {
                    summary.metadata_failures += 1;
                    record_store_db_capture_failure(
                        &mut capture_trace,
                        StoreDbCaptureFailurePhase::Capture,
                    );
                    walk_complete = false;
                    continue;
                }
            };
            let file_type = match agent_entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    summary.metadata_failures += 1;
                    record_store_db_capture_failure(
                        &mut capture_trace,
                        StoreDbCaptureFailurePhase::Capture,
                    );
                    walk_complete = false;
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let agent_id = agent_entry.file_name().to_string_lossy().to_string();
            if agent_id.is_empty() {
                summary.metadata_failures += 1;
                record_store_db_capture_failure(
                    &mut capture_trace,
                    StoreDbCaptureFailurePhase::Capture,
                );
                walk_complete = false;
                continue;
            }
            if only_agent_id.is_some_and(|target| target != agent_id) {
                continue;
            }
            let agent_dir = agent_entry.path();
            let bundle_paths = AGENT_STORE_DB_BASENAMES
                .iter()
                .map(|basename| format!("{SAND_DATA_REL_PREFIX}/agents/{agent_id}/{basename}"))
                .collect::<Vec<_>>();

            if skip_live_handles && has_live_sand_agent_db_handle(&agent_dir.join("store.db")) {
                blocked_from_prune.extend(bundle_paths);
                continue;
            }
            if agent_has_pending_db_recovery(&agent_dir) {
                summary.failures += 1;
                record_store_db_capture_failure(
                    &mut capture_trace,
                    StoreDbCaptureFailurePhase::Capture,
                );
                blocked_from_prune.extend(bundle_paths);
                continue;
            }
            let initial_identity = match agent_db_bundle_identity(&agent_dir) {
                Ok(identity) => identity,
                Err(error) => {
                    eprintln!(
                        "[box-store-sync] agent db bundle identity failed {agent_id}: {error}"
                    );
                    summary.metadata_failures += 1;
                    record_store_db_capture_failure(
                        &mut capture_trace,
                        StoreDbCaptureFailurePhase::Capture,
                    );
                    blocked_from_prune.extend(bundle_paths);
                    continue;
                }
            };

            let mut staged_manifest = manifest.clone();
            let mut bundle_ok = true;
            let mut present_paths = HashSet::new();
            let mut staged_portable_entries = Vec::new();
            let mut agent_has_db = false;
            for basename in AGENT_STORE_DB_BASENAMES {
                let source_path = agent_dir.join(basename);
                let metadata = match fs::metadata(&source_path) {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => {
                        summary.metadata_failures += 1;
                        record_store_db_capture_failure(
                            &mut capture_trace,
                            StoreDbCaptureFailurePhase::Capture,
                        );
                        bundle_ok = false;
                        continue;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        summary.metadata_failures += 1;
                        record_store_db_capture_failure(
                            &mut capture_trace,
                            StoreDbCaptureFailurePhase::Capture,
                        );
                        bundle_ok = false;
                        continue;
                    }
                };
                let rel_path = format!("{SAND_DATA_REL_PREFIX}/agents/{agent_id}/{basename}");
                seen.insert(rel_path.clone());
                present_paths.insert(rel_path.clone());
                summary.files_scanned += 1;
                agent_has_db = true;

                let temp_path = PathBuf::from(format!(
                    "{}{}{}",
                    source_path.display(),
                    BOX_STORE_SNAPSHOT_TMP_SUFFIX,
                    uuid::Uuid::new_v4().simple(),
                ));
                let capture_started_at = Instant::now();
                if let Err(error) = uploader.run_vacuum_off_thread(&source_path, &temp_path) {
                    capture_trace.capture_duration_ms =
                        capture_trace.capture_duration_ms.saturating_add(
                            capture_started_at
                                .elapsed()
                                .as_millis()
                                .min(u128::from(u64::MAX)) as u64,
                        );
                    record_store_db_capture_failure(
                        &mut capture_trace,
                        StoreDbCaptureFailurePhase::Capture,
                    );
                    uploader.discard_snapshot_temp(&temp_path, &rel_path);
                    eprintln!(
                        "[box-store-sync] store.db snapshot capture failed {rel_path}: {error}"
                    );
                    summary.failures += 1;
                    bundle_ok = false;
                    continue;
                }
                capture_trace.capture_duration_ms =
                    capture_trace.capture_duration_ms.saturating_add(
                        capture_started_at
                            .elapsed()
                            .as_millis()
                            .min(u128::from(u64::MAX)) as u64,
                    );

                let blob_upload_started_at = Instant::now();
                let result = uploader.upload_agent_db_snapshot(
                    store_id,
                    &mut staged_manifest,
                    &rel_path,
                    &temp_path,
                    file_mode(&metadata),
                );
                capture_trace.blob_upload_duration_ms =
                    capture_trace.blob_upload_duration_ms.saturating_add(
                        blob_upload_started_at
                            .elapsed()
                            .as_millis()
                            .min(u128::from(u64::MAX)) as u64,
                    );
                uploader.discard_snapshot_temp(&temp_path, &rel_path);
                match result.outcome {
                    SnapshotUploadOutcome::Uploaded => {
                        summary.files_uploaded += 1;
                        summary.bytes_uploaded =
                            summary.bytes_uploaded.saturating_add(result.bytes_uploaded);
                    }
                    SnapshotUploadOutcome::Unchanged => summary.skipped_unchanged += 1,
                    SnapshotUploadOutcome::Oversize => {
                        summary.oversize += 1;
                        bundle_ok = false;
                    }
                    SnapshotUploadOutcome::Error => {
                        summary.failures += 1;
                        record_store_db_capture_failure(
                            &mut capture_trace,
                            StoreDbCaptureFailurePhase::BlobUpload,
                        );
                        bundle_ok = false;
                    }
                }
            }

            if bundle_ok && agent_has_db && portable_backup_enabled {
                let source_db = agent_dir.join("store.db");
                let portable_prefix =
                    format!("{SAND_DATA_REL_PREFIX}/agents/{agent_id}/portable/");
                // Rebuild this generation from exactly the exported artifact
                // set. Old table entries must not survive a successful export
                // when a table was removed from the source database.
                staged_manifest.retain(|path, _| !path.starts_with(&portable_prefix));
                let staging_dir = agent_dir.join(format!(
                    ".openclaw-portable-staging-{}",
                    uuid::Uuid::new_v4().simple()
                ));
                let connection =
                    Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_ONLY);
                let portable_result = connection
                    .map_err(|error| format!("open portable backup source: {error}"))
                    .and_then(|connection| {
                        uploader.export_and_upload_sqlite_backup(
                            store_id,
                            &mut staged_manifest,
                            &connection,
                            &staging_dir,
                            crate::extensions::snapshot::git_backup_manager::GitBackupIdentity {
                                role: "agent".into(),
                                agent_id: Some(agent_id.clone()),
                                id: None,
                            },
                            &BTreeSet::new(),
                            &format!("{SAND_DATA_REL_PREFIX}/agents/{agent_id}/portable"),
                            fs::metadata(&source_db)
                                .map(|metadata| file_mode(&metadata))
                                .unwrap_or(0o600),
                        )
                    });
                match portable_result {
                    Ok(results)
                        if results.iter().all(|result| {
                            matches!(
                                result.outcome,
                                SnapshotUploadOutcome::Uploaded | SnapshotUploadOutcome::Unchanged
                            )
                        }) =>
                    {
                        summary.files_uploaded += results
                            .iter()
                            .filter(|result| result.outcome == SnapshotUploadOutcome::Uploaded)
                            .count();
                        summary.bytes_uploaded = summary.bytes_uploaded.saturating_add(
                            results.iter().map(|result| result.bytes_uploaded).sum(),
                        );
                        for (path, entry) in staged_manifest.iter() {
                            if path.starts_with(&portable_prefix) {
                                staged_portable_entries.push((path.clone(), entry.clone()));
                            }
                        }
                    }
                    Ok(_) | Err(_) => {
                        summary.failures += 1;
                        record_store_db_capture_failure(
                            &mut capture_trace,
                            StoreDbCaptureFailurePhase::BlobUpload,
                        );
                        bundle_ok = false;
                    }
                }
            }

            if agent_has_db {
                agent_count += 1;
            }

            let final_identity = agent_db_bundle_identity(&agent_dir);
            if final_identity.as_ref().ok() != Some(&initial_identity) {
                summary.failures += 1;
                record_store_db_capture_failure(
                    &mut capture_trace,
                    StoreDbCaptureFailurePhase::Capture,
                );
                bundle_ok = false;
            }
            if !bundle_ok {
                blocked_from_prune.extend(bundle_paths);
                blocked_from_prune.extend(
                    manifest
                        .keys()
                        .filter(|path| {
                            path.starts_with(&format!(
                                "{SAND_DATA_REL_PREFIX}/agents/{agent_id}/portable/"
                            ))
                        })
                        .cloned(),
                );
                continue;
            }

            for (path, entry) in staged_portable_entries {
                seen.insert(path.clone());
                set_manifest_entry(manifest, &path, entry);
            }

            for basename in AGENT_STORE_DB_BASENAMES {
                let rel_path = format!("{SAND_DATA_REL_PREFIX}/agents/{agent_id}/{basename}");
                if present_paths.contains(&rel_path) {
                    if let Some(entry) = staged_manifest.get(&rel_path).cloned() {
                        set_manifest_entry(manifest, &rel_path, entry);
                    }
                } else if manifest.remove(&rel_path).is_some() {
                    summary.removed += 1;
                }
            }
        }
    }

    if walk_complete && only_agent_id.is_none() {
        let prefix = format!("{SAND_DATA_REL_PREFIX}/agents/");
        let stale = manifest
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .filter(|path| is_agent_store_db_path(path) || is_agent_portable_backup_path(path))
            .filter(|path| !seen.contains(*path))
            .filter(|path| !blocked_from_prune.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        for path in stale {
            manifest.remove(&path);
            summary.removed += 1;
        }
    }

    Ok(ProductionStoreDbCaptureResult {
        summary,
        agent_count,
        capture_trace,
    })
}

fn agent_has_pending_db_recovery(agent_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(agent_dir) else {
        return true;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        name == "conversation-blobs.db.pending"
            || (name.starts_with("conversation-blobs.db.corrupt-")
                && (name.ends_with(".intent") || name.ends_with(".pending")))
    })
}

fn agent_db_bundle_identity(agent_dir: &Path) -> Result<String, String> {
    let mut fields = Vec::new();
    for basename in AGENT_STORE_DB_BASENAMES {
        let path = agent_dir.join(basename);
        match fs::metadata(&path) {
            Ok(metadata) => {
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_nanos())
                    .unwrap_or_default();
                let wal_path = PathBuf::from(format!("{}-wal", path.display()));
                let (wal_size, wal_modified) = match fs::metadata(&wal_path) {
                    Ok(wal) => (
                        wal.len(),
                        wal.modified()
                            .ok()
                            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                            .map(|value| value.as_nanos())
                            .unwrap_or_default(),
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, 0),
                    Err(error) => return Err(error.to_string()),
                };
                let write_generation = if basename == "store.db" {
                    get_sand_agent_db_write_generation(&path)
                } else {
                    0
                };
                fields.push(format!(
                    "{basename}:present:{write_generation}:{}:{modified}:{}:{wal_size}:{wal_modified}",
                    metadata.len(),
                    file_mode(&metadata),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fields.push(format!("{basename}:absent"));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(fields.join("|"))
}

#[cfg(test)]
fn test_sync_tree_category(
    store: &dyn BoxObjectStore,
    manifest: &mut BoxManifestMap,
    root: &Path,
    rel_prefix: &str,
    name: &str,
    excludes: &[&str],
    ignore: Option<&WorkspaceIgnore>,
    manifest_v2: bool,
) -> Result<CategoryTransferSummary, String> {
    BoxStoreTransfer::new(
        DEFAULT_MAX_OBJECT_BYTES,
        DEFAULT_SNAPSHOT_OUT_CONCURRENCY,
        LARGE_OBJECT_THRESHOLD_BYTES,
        None,
        Arc::new(|_| {}),
    )
    .sync_tree_category(
        store,
        manifest,
        root,
        rel_prefix,
        name,
        excludes,
        ignore,
        false,
        manifest_v2,
    )
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn file_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o666
        }
    }
}

fn is_excluded(rel_path: &str, excludes: &[&str]) -> bool {
    excludes
        .iter()
        .any(|pattern| glob_matches_path(pattern, rel_path))
}

fn is_agent_store_db_path(rel_path: &str) -> bool {
    if !rel_path.contains("/agents/") {
        return false;
    }
    AGENT_STORE_DB_BASENAMES.iter().any(|basename| {
        rel_path.ends_with(&format!("/{basename}"))
            || rel_path.ends_with(&format!("/{basename}-wal"))
            || rel_path.ends_with(&format!("/{basename}-shm"))
            || rel_path.ends_with(&format!("/{basename}-journal"))
            || rel_path.contains(&format!("/{basename}.corrupt-"))
            || rel_path.contains(&format!("/{basename}.replacement"))
            || rel_path.ends_with(&format!("/{basename}.pending"))
    })
}

fn is_agent_portable_backup_path(rel_path: &str) -> bool {
    rel_path.contains("/agents/") && rel_path.contains("/portable/")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn sleep_interruptibly(stopped: &AtomicBool, total_ms: u64) -> bool {
    let mut remaining = total_ms;
    while remaining > 0 {
        if stopped.load(Ordering::Acquire) {
            return false;
        }
        let slice = remaining.min(POLL_SLEEP_SLICE_MS);
        thread::sleep(Duration::from_millis(slice));
        remaining -= slice;
    }
    !stopped.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::box_store_sync::box_object_store::LocalFsObjectStore;

    fn temp_root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-box-store-production-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).expect("create temp root");
        path
    }

    #[test]
    fn conditional_manifest_write_rejects_stale_cycle_baseline() {
        let root = temp_root("manifest-cas");
        let store = LocalFsObjectStore::new(root.join("store"));
        let mut first = BoxManifestMap::new();
        first.insert(
            "workspace/a.txt".into(),
            BoxStoreManifestEntry::File {
                sha: "a".into(),
                size: 1,
                mode: 0o644,
            },
        );
        write_manifest(&store, &first, true, "writer-a".into()).expect("seed manifest");
        let baseline = store
            .get(BOX_STORE_MANIFEST_REL_PATH)
            .expect("read baseline")
            .expect("baseline exists");

        let mut concurrent = first.clone();
        concurrent.insert(
            "workspace/b.txt".into(),
            BoxStoreManifestEntry::File {
                sha: "b".into(),
                size: 1,
                mode: 0o644,
            },
        );
        write_manifest(&store, &concurrent, true, "writer-b".into()).expect("concurrent manifest");

        let error = write_manifest_if_unchanged(
            &store,
            Some(&baseline),
            &first,
            true,
            "stale-writer".into(),
        )
        .expect_err("stale writer must lose");
        assert!(error.contains("concurrent-write race"));

        let persisted = load_manifest(&store)
            .expect("load winning manifest")
            .expect("manifest exists");
        assert!(persisted.entries.contains_key("workspace/b.txt"));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn local_snapshot_round_trips_blob_manifest_ignore_and_pruning() {
        let root = temp_root("snapshot");
        let workspace = root.join("workspace");
        let store_root = root.join("store");
        fs::create_dir_all(workspace.join("src")).expect("create source dir");
        fs::create_dir_all(workspace.join("node_modules/pkg")).expect("create ignored dir");
        fs::write(workspace.join("src/main.ts"), b"export const value = 42;\n")
            .expect("write source file");
        fs::write(workspace.join("node_modules/pkg/index.js"), b"ignored")
            .expect("write ignored file");

        let store = LocalFsObjectStore::new(store_root.clone());
        let ignore = load_workspace_ignore(&workspace, SAND_BOX_WORKSPACE_DEFAULT_IGNORE_PATTERNS);
        let mut manifest = BoxManifestMap::new();
        let summary = test_sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            WORKSPACE_REL_PREFIX,
            "workspace",
            &[],
            Some(&ignore),
            true,
        )
        .expect("snapshot workspace");

        assert_eq!(summary.failures, 0);
        assert_eq!(summary.metadata_failures, 0);
        assert_eq!(summary.files_uploaded, 1);
        assert!(manifest.contains_key("workspace/src/main.ts"));
        assert!(!manifest.contains_key("workspace/node_modules/pkg/index.js"));

        let entry = manifest
            .get("workspace/src/main.ts")
            .expect("source manifest entry");
        let sha = match entry {
            BoxStoreManifestEntry::File { sha, size, mode: _ } => {
                assert_eq!(*size, b"export const value = 42;\n".len() as u64);
                sha.clone()
            }
            other => panic!("expected v2 file entry, got {other:?}"),
        };
        assert!(store_root.join(BOX_STORE_BLOBS_PREFIX).join(&sha).is_file());

        write_manifest(&store, &manifest, true, "test-window".into()).expect("write manifest");
        let persisted = load_manifest(&store)
            .expect("read manifest")
            .expect("manifest exists");
        assert_eq!(persisted.version, BOX_STORE_MANIFEST_VERSION);
        assert_eq!(persisted.writer_window_id.as_deref(), Some("test-window"));
        assert_eq!(persisted.entries, manifest);

        fs::remove_file(workspace.join("src/main.ts")).expect("remove source file");
        let summary = test_sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            WORKSPACE_REL_PREFIX,
            "workspace",
            &[],
            Some(&ignore),
            true,
        )
        .expect("snapshot pruned workspace");
        assert_eq!(summary.removed, 1);
        assert!(!manifest.contains_key("workspace/src/main.ts"));

        fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn local_snapshot_captures_agent_store_databases_instead_of_skipping_them() {
        let root = temp_root("store-db");
        let sand_root = root.join("sand-data");
        let agent_dir = sand_root.join("agents/agent-a");
        let store_root = root.join("store");
        fs::create_dir_all(&agent_dir).expect("create agent dir");

        let source_db = agent_dir.join("store.db");
        {
            let db = rusqlite::Connection::open(&source_db).expect("open source store.db");
            db.execute_batch(
                "CREATE TABLE state (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO state(value) VALUES ('durable');",
            )
            .expect("seed source store.db");
        }

        let store = Arc::new(LocalFsObjectStore::new(&store_root));
        let mut manifest = BoxManifestMap::new();
        let summary = sync_store_db_snapshots(
            store.clone(),
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
        )
        .expect("snapshot store.db");

        assert_eq!(summary.failures, 0);
        assert_eq!(summary.metadata_failures, 0);
        assert_eq!(summary.files_scanned, 1);
        assert_eq!(summary.files_uploaded, 1);
        let rel_path = "home/box/sand-data/agents/agent-a/store.db";
        let sha = match manifest.get(rel_path).expect("store.db manifest entry") {
            BoxStoreManifestEntry::File { sha, size, .. } => {
                assert!(*size > 0);
                sha.clone()
            }
            other => panic!("expected file manifest entry, got {other:?}"),
        };
        assert!(store_root.join(BOX_STORE_BLOBS_PREFIX).join(sha).is_file());

        fs::remove_file(&source_db).expect("remove source db");
        let summary = sync_store_db_snapshots(
            store.clone(),
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
        )
        .expect("prune removed store.db");
        assert_eq!(summary.removed, 1);
        assert!(!manifest.contains_key(rel_path));

        fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn production_cycle_can_publish_openclaw_portable_backup_when_enabled() {
        let root = temp_root("store-db-portable");
        let sand_root = root.join("sand-data");
        let agent_dir = sand_root.join("agents/agent-a");
        let store_root = root.join("store");
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        let source_db = agent_dir.join("store.db");
        {
            let db = rusqlite::Connection::open(&source_db).expect("open source store.db");
            db.execute_batch(
                "CREATE TABLE state (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO state(value) VALUES ('portable');",
            )
            .expect("seed source store.db");
        }
        let store = Arc::new(LocalFsObjectStore::new(&store_root));
        let mut manifest = BoxManifestMap::new();
        let result = sync_store_db_snapshots_with_options(
            store,
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
            true,
        );
        let summary = result.expect("portable production snapshot");
        assert_eq!(summary.failures, 0);
        assert!(summary.files_uploaded >= 4);
        assert!(manifest.contains_key("home/box/sand-data/agents/agent-a/portable/manifest.json"));
        assert!(manifest.contains_key("home/box/sand-data/agents/agent-a/portable/schema.sql"));
        assert!(
            manifest.contains_key("home/box/sand-data/agents/agent-a/portable/tables/state.jsonl")
        );

        // A later generation with a removed table must replace, rather than
        // extend, the portable prefix.
        {
            let db = rusqlite::Connection::open(&source_db).expect("reopen portable source");
            db.execute_batch("DROP TABLE state")
                .expect("remove source table");
        }
        sync_store_db_snapshots_with_options(
            Arc::new(LocalFsObjectStore::new(&store_root)),
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
            true,
        )
        .expect("publish replacement portable generation");
        assert!(!manifest
            .keys()
            .any(|path| path.ends_with("/portable/tables/state.jsonl")));
        fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn agent_db_bundle_does_not_publish_partial_manifest_on_capture_failure() {
        let root = temp_root("store-db-bundle-failure");
        let sand_root = root.join("sand-data");
        let agent_dir = sand_root.join("agents/agent-a");
        let store_root = root.join("store");
        fs::create_dir_all(&agent_dir).expect("create agent dir");

        let source_db = agent_dir.join("store.db");
        {
            let db = rusqlite::Connection::open(&source_db).expect("open source store.db");
            db.execute_batch(
                "CREATE TABLE state (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO state(value) VALUES ('durable');",
            )
            .expect("seed source store.db");
        }
        fs::write(agent_dir.join("conversation-blobs.db"), b"not-a-sqlite-db")
            .expect("write corrupt conversation blob db");

        let store = Arc::new(LocalFsObjectStore::new(&store_root));
        let mut manifest = BoxManifestMap::new();
        let summary = sync_store_db_snapshots(
            store.clone(),
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
        )
        .expect("run failing bundle snapshot");

        assert!(summary.failures >= 1);
        assert!(
            !manifest.contains_key("home/box/sand-data/agents/agent-a/store.db"),
            "a successful first DB must not publish a partial Agent DB bundle"
        );
        assert!(!manifest.contains_key("home/box/sand-data/agents/agent-a/conversation-blobs.db"));

        fs::remove_dir_all(root).expect("cleanup temp root");
    }

    #[test]
    fn store_db_periodic_sweep_skips_live_handle_without_pruning() {
        let root = temp_root("store-db-live-handle");
        let sand_root = root.join("sand-data");
        let agent_dir = sand_root.join("agents/agent-a");
        let store_root = root.join("store");
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        let source_db = agent_dir.join("store.db");
        {
            let db = rusqlite::Connection::open(&source_db).expect("open store.db");
            db.execute_batch(
                "CREATE TABLE state (id INTEGER PRIMARY KEY, value TEXT);
                 INSERT INTO state(value) VALUES ('first');",
            )
            .expect("seed store.db");
        }
        let store = Arc::new(LocalFsObjectStore::new(&store_root));
        let mut manifest = BoxManifestMap::new();
        sync_store_db_snapshots_with_options(
            store.clone(),
            "store-a",
            &mut manifest,
            &sand_root,
            false,
            None,
            false,
        )
        .expect("initial capture");
        let baseline = manifest.clone();

        crate::storage::store_db::register_live_db_handle(&source_db);
        {
            let db = rusqlite::Connection::open(&source_db).expect("reopen store.db");
            db.execute("INSERT INTO state(value) VALUES ('second')", [])
                .expect("mutate live db");
        }
        let summary = sync_store_db_snapshots_with_options(
            store.clone(),
            "store-a",
            &mut manifest,
            &sand_root,
            true,
            None,
            false,
        )
        .expect("live-handle sweep");
        crate::storage::store_db::release_live_db_handle(&source_db);

        assert_eq!(summary.files_scanned, 0);
        assert_eq!(summary.files_uploaded, 0);
        assert_eq!(
            manifest, baseline,
            "live Agent DB must remain durable but untouched"
        );
        assert!(
            manifest.keys().all(|path| !path.contains("/portable/")),
            "default-off periodic cycles must not publish portable artifacts"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn store_db_bundle_identity_fences_shipping_write_generation() {
        let root = temp_root("store-db-generation");
        let agent_dir = root.join("agents/agent-a");
        fs::create_dir_all(&agent_dir).expect("create agent dir");
        let source_db = agent_dir.join("store.db");
        fs::write(&source_db, b"sqlite-placeholder").expect("seed store.db");

        let before = agent_db_bundle_identity(&agent_dir).expect("identity before");
        crate::storage::store_db::bump_db_write_generation(&source_db);
        let after = agent_db_bundle_identity(&agent_dir).expect("identity after");
        crate::storage::store_db::delete_sand_agent_db_write_generation(&source_db);

        assert_ne!(
            before, after,
            "a shipping DB mutation must invalidate capture identity"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn store_db_debounce_queue_coalesces_per_agent_and_requeues_after_fire() {
        let now = Instant::now();
        let mut queue = StoreDbDebounceQueue::default();
        queue.schedule("agent-a".into(), now + Duration::from_millis(50));
        queue.schedule("agent-a".into(), now + Duration::from_millis(100));
        queue.schedule("agent-b".into(), now + Duration::from_millis(75));

        assert!(queue.take_due(now + Duration::from_millis(60)).is_empty());
        assert_eq!(
            queue.take_due(now + Duration::from_millis(80)),
            vec!["agent-b".to_string()]
        );
        assert_eq!(
            queue.take_due(now + Duration::from_millis(120)),
            vec!["agent-a".to_string()]
        );

        queue.schedule("agent-a".into(), now + Duration::from_millis(150));
        assert_eq!(
            queue.take_due(now + Duration::from_millis(160)),
            vec!["agent-a".to_string()]
        );
    }

    #[test]
    fn manifest_v1_fails_closed_for_symlink_semantics() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = temp_root("symlink");
            let workspace = root.join("workspace");
            let store_root = root.join("store");
            fs::create_dir_all(&workspace).expect("create workspace");
            fs::write(workspace.join("target.txt"), b"target").expect("write target");
            symlink("target.txt", workspace.join("link.txt")).expect("create symlink");

            let store = LocalFsObjectStore::new(store_root);
            let mut manifest = BoxManifestMap::new();
            let summary = test_sync_tree_category(
                &store,
                &mut manifest,
                &workspace,
                WORKSPACE_REL_PREFIX,
                "workspace",
                &[],
                None,
                false,
            )
            .expect("snapshot v1 workspace");

            assert!(summary.failures >= 1);
            assert!(!manifest.contains_key("workspace/link.txt"));
            fs::remove_dir_all(root).expect("cleanup temp root");
        }
    }

    #[test]
    fn v2_backend_resolves_to_live_production_mode() {
        let env = BTreeMap::from([
            ("SAND_BOX_STORE_SYNC".to_string(), "1".to_string()),
            ("SAND_BOX_STORE_BACKEND".to_string(), "v2".to_string()),
        ]);
        assert_eq!(
            resolve_production_box_store_sync_mode(&env),
            ProductionBoxStoreSyncMode::SandBoxStoreV2
        );
        assert_eq!(
            mode_name(&ProductionBoxStoreSyncMode::SandBoxStoreV2),
            "sand-box-store-v2"
        );
    }
}
