use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use super::box_object_store::BoxObjectStore;
use super::box_store_manifest_format::{BOX_STORE_BLOBS_PREFIX, BoxStoreManifestEntry};
use super::box_store_pack::{
    BOX_STORE_PACK_INDEX_KEY, BOX_STORE_PACKS_PREFIX, PACK_INDEX_MAX_BYTES, PackAdmissionPermit,
    PackExtractionSink, PackMember, extract_pack_members, parse_pack_index,
};
use super::box_store_pack_pipeline::{
    BoxStoreBlobGroup, PACK_DOWNLOAD_CONCURRENCY, PACK_EXTRACT_CONCURRENCY, PACK_TMP_DIR_NAME,
    should_restore_from_pack, temp_is_stale,
};
use super::box_store_sync::{
    DEFAULT_COPY_IN_CONCURRENCY, DOWNLOAD_IN_FLIGHT_BYTE_BUDGET, LARGE_OBJECT_THRESHOLD_BYTES,
};
use crate::r#box::box_transfer::{for_each_bounded, for_each_wave_pipelined};

pub const COPY_IN_LARGE_BLOB_CONCURRENCY: usize = 4;
pub const COPY_IN_WAVE_SIZE: usize = 500;
pub const BOX_STORE_RESTORE_TMP_SUFFIX: &str = ".box-store-part-";
pub const LARGE_OBJECT_FREE_SPACE_FACTOR: u64 = 2;

pub fn is_critical_rel_path(rel_path: &str) -> bool {
    let basename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    matches!(
        basename,
        "store.db"
            | "conversation-blobs.db"
            | "Cookies"
            | "Login Data"
            | "Web Data"
            | "source-map.json"
    )
}

pub fn resolve_restore_destination(target_root: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.is_empty() || rel_path.contains('\0') || Path::new(rel_path).is_absolute() {
        return None;
    }
    if rel_path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    let root = lexical_normalize(target_root);
    let destination = lexical_normalize(&root.join(rel_path));
    (destination != root && destination.starts_with(&root)).then_some(destination)
}

pub fn symlink_target_stays_within_root(
    target_root: &Path,
    symlink_path: &Path,
    target: &str,
) -> bool {
    if target.contains('\0') {
        return false;
    }
    let root = lexical_normalize(target_root);
    let target_path = Path::new(target);
    let candidate = if target_path.is_absolute() {
        lexical_normalize(target_path)
    } else {
        let Some(parent) = symlink_path.parent() else {
            return false;
        };
        lexical_normalize(&parent.join(target_path))
    };
    candidate == root || candidate.starts_with(&root)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[derive(Debug, Default)]
struct ByteBudgetState {
    in_flight_bytes: u64,
    waiters: VecDeque<(u64, u64)>,
}

#[derive(Debug)]
pub struct BoxStoreByteBudget {
    budget_bytes: u64,
    next_ticket: AtomicU64,
    state: Mutex<ByteBudgetState>,
    wake: Condvar,
}

impl BoxStoreByteBudget {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            next_ticket: AtomicU64::new(1),
            state: Mutex::new(ByteBudgetState::default()),
            wake: Condvar::new(),
        }
    }

    pub fn try_acquire(&self, bytes: u64) -> bool {
        if bytes == 0 {
            return true;
        }
        let mut state = self.state.lock().expect("byte budget poisoned");
        if !state.waiters.is_empty()
            || (state.in_flight_bytes > 0
                && state.in_flight_bytes.saturating_add(bytes) > self.budget_bytes)
        {
            return false;
        }
        state.in_flight_bytes = state.in_flight_bytes.saturating_add(bytes);
        true
    }

    pub fn acquire(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.lock().expect("byte budget poisoned");
        if state.waiters.is_empty()
            && (state.in_flight_bytes == 0
                || state.in_flight_bytes.saturating_add(bytes) <= self.budget_bytes)
        {
            state.in_flight_bytes = state.in_flight_bytes.saturating_add(bytes);
            return;
        }
        state.waiters.push_back((ticket, bytes));
        loop {
            let is_head = state
                .waiters
                .front()
                .is_some_and(|(head_ticket, _)| *head_ticket == ticket);
            let fits = state.in_flight_bytes == 0
                || state.in_flight_bytes.saturating_add(bytes) <= self.budget_bytes;
            if is_head && fits {
                state.waiters.pop_front();
                state.in_flight_bytes = state.in_flight_bytes.saturating_add(bytes);
                self.wake.notify_all();
                return;
            }
            state = self.wake.wait(state).expect("byte budget wait poisoned");
        }
    }

    pub fn release(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut state = self.state.lock().expect("byte budget poisoned");
        state.in_flight_bytes = state.in_flight_bytes.saturating_sub(bytes);
        self.wake.notify_all();
    }

    pub fn in_flight_bytes(&self) -> u64 {
        self.state
            .lock()
            .expect("byte budget poisoned")
            .in_flight_bytes
    }
}

pub fn claimed_destinations_are_unique<I>(paths: I) -> bool
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut seen = HashSet::new();
    paths.into_iter().all(|path| seen.insert(path))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoxStoreDownloadProgress {
    pub files: usize,
    pub bytes: u64,
    pub verified: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxStoreDownloadTrace {
    pub event: String,
    pub manifest_entries: usize,
    pub file_entries: usize,
    pub symlink_entries: usize,
    pub symlinks_started: usize,
    pub symlinks_completed: usize,
    pub symlinks_in_flight: usize,
    pub active_symlink_steps: BTreeMap<String, usize>,
    pub symlink_ordinal: Option<usize>,
    pub symlink_step: Option<String>,
    pub symlink_outcome: Option<String>,
}

#[derive(Clone)]
pub struct BoxStoreDownloadOptions {
    pub download_concurrency: usize,
    pub large_object_threshold: u64,
    pub download_byte_budget: u64,
    pub owner: Option<(u32, u32)>,
    pub on_progress: Option<Arc<dyn Fn(BoxStoreDownloadProgress) + Send + Sync>>,
    pub on_trace: Option<Arc<dyn Fn(BoxStoreDownloadTrace) + Send + Sync>>,
    pub on_log: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

impl Default for BoxStoreDownloadOptions {
    fn default() -> Self {
        Self {
            download_concurrency: DEFAULT_COPY_IN_CONCURRENCY,
            large_object_threshold: LARGE_OBJECT_THRESHOLD_BYTES,
            download_byte_budget: DOWNLOAD_IN_FLIGHT_BYTE_BUDGET,
            owner: None,
            on_progress: None,
            on_trace: None,
            on_log: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxStoreDownloadSummary {
    pub manifest_entries: usize,
    pub files: usize,
    pub bytes: u64,
    pub verified: usize,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
struct PlannedDownload {
    manifest: BTreeMap<String, BoxStoreManifestEntry>,
    destinations: HashMap<String, PathBuf>,
    critical_small: Vec<BoxStoreBlobGroup>,
    critical_large: Vec<BoxStoreBlobGroup>,
    bulk_small: Vec<BoxStoreBlobGroup>,
    bulk_large: Vec<BoxStoreBlobGroup>,
    symlinks: Vec<(String, String)>,
    failures: Vec<String>,
}

fn file_identity(entry: &BoxStoreManifestEntry) -> Option<(&str, u64)> {
    match entry {
        BoxStoreManifestEntry::LegacyFile { sha, size }
        | BoxStoreManifestEntry::File { sha, size, .. } => Some((sha, *size)),
        BoxStoreManifestEntry::Symlink { .. } => None,
    }
}

fn plan_download(
    target_root: &Path,
    manifest: &BTreeMap<String, BoxStoreManifestEntry>,
    large_object_threshold: u64,
) -> PlannedDownload {
    let mut destinations = HashMap::new();
    let mut claimed = HashSet::new();
    let mut failures = Vec::new();
    let mut critical_small = HashMap::<String, BoxStoreBlobGroup>::new();
    let mut critical_large = HashMap::<String, BoxStoreBlobGroup>::new();
    let mut bulk_small = HashMap::<String, BoxStoreBlobGroup>::new();
    let mut bulk_large = HashMap::<String, BoxStoreBlobGroup>::new();
    let mut symlinks = Vec::new();

    for (rel_path, entry) in manifest {
        let Some(destination) = resolve_restore_destination(target_root, rel_path) else {
            failures.push(format!("{rel_path}: unsafe manifest path"));
            continue;
        };
        if !claimed.insert(destination.clone()) {
            failures.push(format!("{rel_path}: unsafe manifest path"));
            continue;
        }
        destinations.insert(rel_path.clone(), destination);
        match entry {
            BoxStoreManifestEntry::Symlink { target } => {
                symlinks.push((rel_path.clone(), target.clone()));
            }
            _ => {
                let (sha, size) = file_identity(entry).expect("file entry");
                let groups = if is_critical_rel_path(rel_path) {
                    if size >= large_object_threshold {
                        &mut critical_large
                    } else {
                        &mut critical_small
                    }
                } else if size >= large_object_threshold {
                    &mut bulk_large
                } else {
                    &mut bulk_small
                };
                let key = format!("{sha}:{size}");
                groups
                    .entry(key)
                    .and_modify(|group| group.rel_paths.push(rel_path.clone()))
                    .or_insert_with(|| BoxStoreBlobGroup {
                        sha: sha.to_string(),
                        size,
                        rel_paths: vec![rel_path.clone()],
                    });
            }
        }
    }

    PlannedDownload {
        manifest: manifest.clone(),
        destinations,
        critical_small: critical_small.into_values().collect(),
        critical_large: critical_large.into_values().collect(),
        bulk_small: bulk_small.into_values().collect(),
        bulk_large: bulk_large.into_values().collect(),
        symlinks,
        failures,
    }
}

fn create_restore_directory_race_safe(path: &Path) -> Result<bool, String> {
    match fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
                Ok(false)
            } else {
                Err("restore parent is not a directory".into())
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_owned_parent(target_root: &Path, destination: &Path, owner: Option<(u32, u32)>) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "restore destination has no parent".to_string())?;
    let relative = parent
        .strip_prefix(target_root)
        .map_err(|_| "restore directory escapes target root".to_string())?;
    if fs::symlink_metadata(target_root)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("restore target root is a symlink".into());
    }
    let mut current = target_root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        let created = match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => false,
            Ok(_) => {
                if let Err(error) = remove_existing_restore_path(&current) {
                    if fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.is_dir()) {
                        false
                    } else {
                        return Err(error);
                    }
                } else {
                    create_restore_directory_race_safe(&current)?
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_restore_directory_race_safe(&current)?
            }
            Err(error) => return Err(error.to_string()),
        };
        if created {
            apply_owner(&current, owner, false)?;
        }
    }
    Ok(())
}

fn apply_owner(path: &Path, owner: Option<(u32, u32)>, symlink: bool) -> Result<(), String> {
    let Some((uid, gid)) = owner else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        if symlink {
            let raw = CString::new(path.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
            let rc = unsafe { libc::lchown(raw.as_ptr(), uid, gid) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        } else {
            std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|error| error.to_string())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, uid, gid, symlink);
        Ok(())
    }
}

fn open_readonly_nofollow(path: &Path) -> Result<fs::File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|error| error.to_string())
}

fn apply_open_file_owner(file: &fs::File, owner: Option<(u32, u32)>) -> Result<(), String> {
    let Some((uid, gid)) = owner else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let rc = unsafe { libc::fchown(file.as_raw_fd(), uid, gid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (file, uid, gid);
    }
    Ok(())
}

fn entry_mode(entry: &BoxStoreManifestEntry) -> Option<u32> {
    match entry {
        BoxStoreManifestEntry::File { mode, .. } => Some(*mode),
        _ => None,
    }
}

fn apply_file_metadata(
    path: &Path,
    entry: &BoxStoreManifestEntry,
    owner: Option<(u32, u32)>,
    legacy_replacement_mode: Option<u32>,
) -> Result<(), String> {
    let file = open_readonly_nofollow(path)?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("restore destination is not a regular file".into());
    }
    apply_open_file_owner(&file, owner)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = entry_mode(entry).or(legacy_replacement_mode) {
            file.set_permissions(fs::Permissions::from_mode(mode))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn existing_regular_file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn unique_temp_path(destination: &Path) -> PathBuf {
    PathBuf::from(format!(
        "{}{}{}",
        destination.display(),
        BOX_STORE_RESTORE_TMP_SUFFIX,
        uuid::Uuid::new_v4().simple()
    ))
}

fn install_verified_temp(
    temp: &Path,
    destination: &Path,
    entry: &BoxStoreManifestEntry,
    owner: Option<(u32, u32)>,
) -> Result<(), String> {
    let legacy_mode = matches!(entry, BoxStoreManifestEntry::LegacyFile { .. })
        .then(|| existing_regular_file_mode(destination))
        .flatten();
    apply_file_metadata(temp, entry, owner, legacy_mode)?;
    if fs::symlink_metadata(destination).is_ok_and(|metadata| metadata.is_dir()) {
        fs::remove_dir_all(destination).map_err(|error| error.to_string())?;
    }
    fs::rename(temp, destination).map_err(|error| error.to_string())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = open_readonly_nofollow(path)?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("hash source is not a regular file".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn local_file_matches(path: &Path, group: &BoxStoreBlobGroup) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() == group.size)
        && sha256_file(path).ok().as_deref() == Some(group.sha.as_str())
}

fn copy_file_hashing(source: &Path, destination: &Path) -> Result<(String, u64), String> {
    let mut reader = open_readonly_nofollow(source)?;
    let metadata = reader.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("copy source is not a regular file".into());
    }
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).map_err(|error| error.to_string())?;
        hash.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    writer.flush().map_err(|error| error.to_string())?;
    Ok((format!("{:x}", hash.finalize()), size))
}

fn has_disk_space_for_large_object(destination: &Path, object_bytes: u64) -> bool {
    let parent = destination.parent().unwrap_or(destination);
    fs2::available_space(parent)
        .map(|available| available >= object_bytes.saturating_mul(LARGE_OBJECT_FREE_SPACE_FACTOR))
        .unwrap_or(false)
}

fn report_progress(
    options: &BoxStoreDownloadOptions,
    state: &Mutex<BoxStoreDownloadSummary>,
    total: usize,
) {
    let Some(callback) = options.on_progress.as_ref() else {
        return;
    };
    let summary = state.lock().expect("download summary lock").clone();
    callback(BoxStoreDownloadProgress {
        files: summary.files,
        bytes: summary.bytes,
        verified: summary.verified,
        total,
    });
}

fn record_failure(state: &Mutex<BoxStoreDownloadSummary>, failure: String) {
    state
        .lock()
        .expect("download summary lock")
        .failures
        .push(failure);
}

fn restore_small_group(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    planned: &PlannedDownload,
    group: &BoxStoreBlobGroup,
    pending: &[String],
    budget: &BoxStoreByteBudget,
    options: &BoxStoreDownloadOptions,
    state: &Mutex<BoxStoreDownloadSummary>,
) {
    budget.acquire(group.size);
    let result = store.get(&format!("{BOX_STORE_BLOBS_PREFIX}/{}", group.sha));
    match result {
        Ok(Some(blob)) if blob.len() as u64 == group.size
            && format!("{:x}", Sha256::digest(&blob)) == group.sha =>
        {
            for rel_path in pending {
                {
                    let mut summary = state.lock().expect("download summary lock");
                    summary.verified = summary.verified.saturating_add(1);
                }
                let Some(destination) = planned.destinations.get(rel_path) else {
                    record_failure(state, format!("{rel_path}: unsafe manifest path"));
                    continue;
                };
                let Some(entry) = planned.manifest.get(rel_path) else {
                    record_failure(state, format!("{rel_path}: missing manifest entry"));
                    continue;
                };
                let result = (|| -> Result<(), String> {
                    ensure_owned_parent(target_root, destination, options.owner)?;
                    let temp = unique_temp_path(destination);
                    let write = (|| -> Result<(), String> {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&temp)
                            .map_err(|error| error.to_string())?;
                        file.write_all(&blob).map_err(|error| error.to_string())?;
                        file.flush().map_err(|error| error.to_string())?;
                        install_verified_temp(&temp, destination, entry, options.owner)
                    })();
                    if write.is_err() {
                        let _ = fs::remove_file(&temp);
                    }
                    write
                })();
                match result {
                    Ok(()) => {
                        let mut summary = state.lock().expect("download summary lock");
                        summary.files = summary.files.saturating_add(1);
                        summary.bytes = summary.bytes.saturating_add(blob.len() as u64);
                        drop(summary);
                        report_progress(options, state, planned.manifest.len());
                    }
                    Err(error) => record_failure(state, format!("{rel_path}: {error}")),
                }
            }
        }
        Ok(Some(_)) => {
            for rel_path in pending {
                record_failure(state, format!("{rel_path}: sha/size mismatch"));
            }
        }
        Ok(None) => {
            for rel_path in pending {
                record_failure(state, format!("{rel_path}: blob {} missing", group.sha));
            }
        }
        Err(error) => {
            for rel_path in pending {
                record_failure(state, format!("{rel_path}: {error}"));
            }
        }
    }
    budget.release(group.size);
}

fn restore_large_group(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    planned: &PlannedDownload,
    group: &BoxStoreBlobGroup,
    pending: &[String],
    options: &BoxStoreDownloadOptions,
    state: &Mutex<BoxStoreDownloadSummary>,
) {
    let mut restored_source: Option<PathBuf> = None;
    for rel_path in pending {
        let Some(destination) = planned.destinations.get(rel_path) else {
            record_failure(state, format!("{rel_path}: unsafe manifest path"));
            continue;
        };
        let Some(entry) = planned.manifest.get(rel_path) else {
            record_failure(state, format!("{rel_path}: missing manifest entry"));
            continue;
        };
        if let Err(error) = ensure_owned_parent(target_root, destination, options.owner) {
            record_failure(state, format!("{rel_path}: {error}"));
            continue;
        }
        if !has_disk_space_for_large_object(destination, group.size) {
            record_failure(
                state,
                format!(
                    "{rel_path}: insufficient disk space for {}B restore (x{} required)",
                    group.size, LARGE_OBJECT_FREE_SPACE_FACTOR
                ),
            );
            continue;
        }

        let temp = unique_temp_path(destination);
        let verified_temp = if let Some(source) = restored_source.as_ref() {
            match copy_file_hashing(source, &temp) {
                Ok((sha, size)) if sha == group.sha && size == group.size => Ok(()),
                Ok(_) => Err("sha/size mismatch".to_string()),
                Err(error) => Err(error),
            }
        } else {
            match store.get_to_file(
                &format!("{BOX_STORE_BLOBS_PREFIX}/{}", group.sha),
                &temp,
                Some(group.size),
            ) {
                Ok(Some(written)) if written == group.size => match sha256_file(&temp) {
                    Ok(sha) if sha == group.sha => Ok(()),
                    Ok(_) => Err("sha/size mismatch".to_string()),
                    Err(error) => Err(error),
                },
                Ok(Some(_)) => Err("sha/size mismatch".to_string()),
                Ok(None) => Err(format!("blob {} missing", group.sha)),
                Err(error) => Err(error),
            }
        };

        if let Err(error) = verified_temp {
            let _ = fs::remove_file(&temp);
            record_failure(state, format!("{rel_path}: {error}"));
            continue;
        }
        {
            let mut summary = state.lock().expect("download summary lock");
            summary.verified = summary.verified.saturating_add(1);
        }
        if let Err(error) = install_verified_temp(&temp, destination, entry, options.owner) {
            let _ = fs::remove_file(&temp);
            record_failure(state, format!("{rel_path}: {error}"));
            continue;
        }
        if restored_source.is_none() {
            restored_source = Some(destination.clone());
        }
        let mut summary = state.lock().expect("download summary lock");
        summary.files = summary.files.saturating_add(1);
        summary.bytes = summary.bytes.saturating_add(group.size);
        drop(summary);
        report_progress(options, state, planned.manifest.len());
    }
}

#[derive(Debug, Clone)]
struct PackRestoreGroup {
    size: u64,
    rel_paths: Vec<String>,
}

struct BoxStoreByteBudgetPermit {
    budget: Arc<BoxStoreByteBudget>,
    bytes: u64,
}

impl Drop for BoxStoreByteBudgetPermit {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

struct DownloadPackSink {
    target_root: PathBuf,
    manifest: BTreeMap<String, BoxStoreManifestEntry>,
    groups: HashMap<String, PackRestoreGroup>,
    restored: Mutex<HashSet<String>>,
    owner: Option<(u32, u32)>,
    budget: Arc<BoxStoreByteBudget>,
    summary: Arc<Mutex<BoxStoreDownloadSummary>>,
    progress_report_lock: Mutex<()>,
    options: BoxStoreDownloadOptions,
}

impl PackExtractionSink for DownloadPackSink {
    fn wants(&self, member: &PackMember) -> bool {
        let key = format!("{}:{}", member.sha, member.size);
        let Some(group) = self.groups.get(&key) else {
            return false;
        };
        let restored = self.restored.lock().expect("pack restored lock");
        group.rel_paths.iter().any(|rel_path| !restored.contains(rel_path))
    }

    fn admit_bytes(
        &self,
        bytes: u64,
    ) -> Result<Option<Box<dyn PackAdmissionPermit>>, String> {
        self.budget.acquire(bytes);
        Ok(Some(Box::new(BoxStoreByteBudgetPermit {
            budget: Arc::clone(&self.budget),
            bytes,
        })))
    }

    fn on_blob(&self, member: &PackMember, bytes: Vec<u8>) -> Result<(), String> {
        let key = format!("{}:{}", member.sha, member.size);
        let Some(group) = self.groups.get(&key) else {
            return Ok(());
        };
        if bytes.len() as u64 != member.size
            || format!("{:x}", Sha256::digest(&bytes)) != member.sha
        {
            return Err("pack member sha/size mismatch".into());
        }
        for rel_path in &group.rel_paths {
            let reserved = {
                let mut restored = self.restored.lock().expect("pack restored lock");
                restored.insert(rel_path.clone())
            };
            if !reserved {
                continue;
            }
            let Some(destination) = resolve_restore_destination(&self.target_root, rel_path) else {
                self.restored
                    .lock()
                    .expect("pack restored lock")
                    .remove(rel_path);
                continue;
            };
            let Some(entry) = self.manifest.get(rel_path) else {
                self.restored
                    .lock()
                    .expect("pack restored lock")
                    .remove(rel_path);
                continue;
            };
            if local_file_matches(
                &destination,
                &BoxStoreBlobGroup {
                    sha: member.sha.clone(),
                    size: member.size,
                    rel_paths: Vec::new(),
                },
            ) {
                if apply_file_metadata(&destination, entry, self.owner, None).is_ok() {
                    continue;
                }
            }
            let result = (|| -> Result<(), String> {
                ensure_owned_parent(&self.target_root, &destination, self.owner)?;
                let temp = unique_temp_path(&destination);
                let write = (|| -> Result<(), String> {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)
                        .map_err(|error| error.to_string())?;
                    file.write_all(&bytes).map_err(|error| error.to_string())?;
                    file.flush().map_err(|error| error.to_string())?;
                    install_verified_temp(&temp, &destination, entry, self.owner)
                })();
                if write.is_err() {
                    let _ = fs::remove_file(&temp);
                }
                write
            })();
            if result.is_err() {
                self.restored
                    .lock()
                    .expect("pack restored lock")
                    .remove(rel_path);
                continue;
            }
            let _report_guard = self
                .progress_report_lock
                .lock()
                .expect("pack progress report lock");
            let progress = {
                let mut summary = self.summary.lock().expect("download summary lock");
                summary.verified = summary.verified.saturating_add(1);
                summary.files = summary.files.saturating_add(1);
                summary.bytes = summary.bytes.saturating_add(group.size);
                summary.clone()
            };
            if let Some(callback) = self.options.on_progress.as_ref() {
                callback(BoxStoreDownloadProgress {
                    files: progress.files,
                    bytes: progress.bytes,
                    verified: progress.verified,
                    total: self.manifest.len(),
                });
            }
        }
        Ok(())
    }
}

fn sweep_restore_pack_temp_dir(temp_dir: &Path) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0);
    let Ok(entries) = fs::read_dir(temp_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let modified_ms = modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or(now_ms);
        if temp_is_stale(now_ms, modified_ms) {
            let _ = fs::remove_file(path);
        }
    }
}

fn restore_bulk_small_from_packs(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    planned: &PlannedDownload,
    owner: Option<(u32, u32)>,
    budget: Arc<BoxStoreByteBudget>,
    options: &BoxStoreDownloadOptions,
    summary: Arc<Mutex<BoxStoreDownloadSummary>>,
) -> HashSet<String> {
    let mut groups = HashMap::<String, PackRestoreGroup>::new();
    for group in &planned.bulk_small {
        groups.insert(
            format!("{}:{}", group.sha, group.size),
            PackRestoreGroup {
                size: group.size,
                rel_paths: group.rel_paths.clone(),
            },
        );
    }
    if groups.is_empty() {
        return HashSet::new();
    }

    let raw_index = match store.get(BOX_STORE_PACK_INDEX_KEY) {
        Ok(Some(bytes)) if bytes.len() as u64 <= PACK_INDEX_MAX_BYTES => bytes,
        Ok(_) => return HashSet::new(),
        Err(error) => {
            if let Some(log) = options.on_log.as_ref() {
                log(format!(
                    "pack index read failed (falling back to loose blobs): {error}"
                ));
            }
            return HashSet::new();
        }
    };
    let Some(index) = std::str::from_utf8(&raw_index).ok().and_then(parse_pack_index) else {
        return HashSet::new();
    };

    let sink = Arc::new(DownloadPackSink {
        target_root: target_root.to_path_buf(),
        manifest: planned.manifest.clone(),
        groups: groups.clone(),
        restored: Mutex::new(HashSet::new()),
        owner,
        budget,
        summary,
        progress_report_lock: Mutex::new(()),
        options: options.clone(),
    });
    let temp_dir = std::env::temp_dir().join(PACK_TMP_DIR_NAME);
    if let Err(error) = fs::create_dir_all(&temp_dir) {
        if let Some(log) = options.on_log.as_ref() {
            log(format!(
                "pack temp directory unavailable (falling back to loose blobs): {error}"
            ));
        }
        return HashSet::new();
    }
    sweep_restore_pack_temp_dir(&temp_dir);

    let _ = for_each_bounded(
        &index.packs,
        PACK_DOWNLOAD_CONCURRENCY,
        |pack| {
            let restored = sink.restored.lock().expect("pack restored lock");
            let mut usable_count = 0usize;
            let mut usable_clen = 0u64;
            let mut usable_size = 0u64;
            for member in &pack.members {
                let key = format!("{}:{}", member.sha, member.size);
                let Some(group) = groups.get(&key) else {
                    continue;
                };
                let has_pending = group.rel_paths.iter().any(|path| {
                    if restored.contains(path) {
                        return false;
                    }
                    let Some(destination) = planned.destinations.get(path) else {
                        return true;
                    };
                    !local_file_matches(
                        destination,
                        &BoxStoreBlobGroup {
                            sha: member.sha.clone(),
                            size: member.size,
                            rel_paths: Vec::new(),
                        },
                    )
                });
                if has_pending {
                    usable_count += 1;
                    usable_clen = usable_clen.saturating_add(member.clen);
                    usable_size = usable_size.saturating_add(member.size);
                }
            }
            drop(restored);
            if !should_restore_from_pack(usable_count, usable_clen, usable_size, pack.bytes) {
                return Ok::<(), String>(());
            }

            let temp = temp_dir.join(format!(
                "restore-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            let restored_pack = (|| -> Result<(), String> {
                let Some(written) = store.get_to_file(
                    &format!("{BOX_STORE_PACKS_PREFIX}/{}", pack.id),
                    &temp,
                    Some(pack.bytes),
                )? else {
                    return Ok(());
                };
                let _ = written;
                let result = extract_pack_members(
                    &temp,
                    &pack.members,
                    PACK_EXTRACT_CONCURRENCY,
                    sink.clone(),
                )?;
                if result.mismatched > 0 {
                    if let Some(log) = options.on_log.as_ref() {
                        log(format!(
                            "pack {}: {} member(s) failed sha verify; falling back to loose blobs",
                            pack.id, result.mismatched
                        ));
                    }
                }
                Ok(())
            })();
            let _ = fs::remove_file(&temp);
            if let Err(error) = restored_pack {
                if let Some(log) = options.on_log.as_ref() {
                    log(format!(
                        "pack {} restore failed (falling back to loose blobs): {error}",
                        pack.id
                    ));
                }
            }
            Ok::<(), String>(())
        },
    );

    let restored = sink.restored.lock().expect("pack restored lock").clone();
    restored
}

const SYMLINK_TRACE_STEPS: &[&str] = &[
    "prepare-parent",
    "match-existing",
    "create-temp",
    "inspect-destination",
    "remove-directory",
    "rename-temp",
    "apply-owner",
    "verify-target",
    "cleanup-temp",
];

#[derive(Debug)]
struct SymlinkTraceState {
    started: usize,
    completed: usize,
    active_steps: BTreeMap<String, usize>,
}

impl Default for SymlinkTraceState {
    fn default() -> Self {
        Self {
            started: 0,
            completed: 0,
            active_steps: SYMLINK_TRACE_STEPS
                .iter()
                .map(|step| ((*step).to_string(), 0usize))
                .collect(),
        }
    }
}

fn emit_symlink_trace(
    options: &BoxStoreDownloadOptions,
    state: &Mutex<SymlinkTraceState>,
    manifest_entries: usize,
    file_entries: usize,
    symlink_entries: usize,
    event: &str,
    ordinal: Option<usize>,
    step: Option<&str>,
    outcome: Option<&str>,
) {
    let Some(callback) = options.on_trace.as_ref() else {
        return;
    };
    let trace = state.lock().expect("symlink trace lock");
    let snapshot = BoxStoreDownloadTrace {
        event: event.to_string(),
        manifest_entries,
        file_entries,
        symlink_entries,
        symlinks_started: trace.started,
        symlinks_completed: trace.completed,
        symlinks_in_flight: trace.started.saturating_sub(trace.completed),
        active_symlink_steps: trace.active_steps.clone(),
        symlink_ordinal: ordinal,
        symlink_step: step.map(str::to_string),
        symlink_outcome: outcome.map(str::to_string),
    };
    drop(trace);
    callback(snapshot);
}

fn run_symlink_step<T>(
    options: &BoxStoreDownloadOptions,
    state: &Mutex<SymlinkTraceState>,
    manifest_entries: usize,
    file_entries: usize,
    symlink_entries: usize,
    ordinal: usize,
    step: &'static str,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    {
        let mut trace = state.lock().expect("symlink trace lock");
        *trace.active_steps.entry(step.to_string()).or_default() += 1;
    }
    emit_symlink_trace(
        options,
        state,
        manifest_entries,
        file_entries,
        symlink_entries,
        "symlink-step-started",
        Some(ordinal),
        Some(step),
        None,
    );
    let result = operation();
    {
        let mut trace = state.lock().expect("symlink trace lock");
        let active = trace.active_steps.entry(step.to_string()).or_default();
        *active = active.saturating_sub(1);
    }
    emit_symlink_trace(
        options,
        state,
        manifest_entries,
        file_entries,
        symlink_entries,
        "symlink-step-finished",
        Some(ordinal),
        Some(step),
        None,
    );
    result
}

fn run_download_phase(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    planned: &PlannedDownload,
    groups: &[BoxStoreBlobGroup],
    concurrency: usize,
    large: bool,
    budget: &BoxStoreByteBudget,
    options: &BoxStoreDownloadOptions,
    state: &Mutex<BoxStoreDownloadSummary>,
) {
    let result = for_each_wave_pipelined(
        groups,
        COPY_IN_WAVE_SIZE,
        concurrency.max(1),
        |wave| {
            let prepared = Mutex::new(Vec::<(BoxStoreBlobGroup, Vec<String>)>::new());
            for_each_bounded(wave, concurrency.max(1), |group| {
                let mut pending = Vec::new();
                for rel_path in &group.rel_paths {
                    let Some(destination) = planned.destinations.get(rel_path) else {
                        continue;
                    };
                    if let Err(error) = ensure_owned_parent(target_root, destination, options.owner) {
                        record_failure(state, format!("{rel_path}: {error}"));
                        continue;
                    }
                    if local_file_matches(destination, group) {
                        {
                            let mut summary = state.lock().expect("download summary lock");
                            summary.verified = summary.verified.saturating_add(1);
                        }
                        let Some(entry) = planned.manifest.get(rel_path) else {
                            record_failure(state, format!("{rel_path}: missing manifest entry"));
                            continue;
                        };
                        match apply_file_metadata(destination, entry, options.owner, None) {
                            Ok(()) => {
                                let mut summary = state.lock().expect("download summary lock");
                                summary.files = summary.files.saturating_add(1);
                                summary.bytes = summary.bytes.saturating_add(group.size);
                                drop(summary);
                                report_progress(options, state, planned.manifest.len());
                            }
                            Err(error) => record_failure(state, format!("{rel_path}: {error}")),
                        }
                    } else {
                        pending.push(rel_path.clone());
                    }
                }
                if !pending.is_empty() {
                    prepared
                        .lock()
                        .expect("download prepared lock")
                        .push((group.clone(), pending));
                }
                Ok::<(), String>(())
            })?;
            let prepared = prepared
                .into_inner()
                .expect("download prepared mutex");
            if !prepared.is_empty() {
                let keys = prepared
                    .iter()
                    .map(|(group, _)| format!("{BOX_STORE_BLOBS_PREFIX}/{}", group.sha))
                    .collect::<Vec<_>>();
                if let Err(error) = store.prefetch_reads(&keys) {
                    if let Some(log) = options.on_log.as_ref() {
                        log(format!(
                            "read prefetch failed (falling back to per-blob presign): {error}"
                        ));
                    }
                }
            }
            Ok::<_, String>(prepared)
        },
        |(group, pending)| {
            if large {
                restore_large_group(
                    store,
                    target_root,
                    planned,
                    &group,
                    &pending,
                    options,
                    state,
                );
            } else {
                restore_small_group(
                    store,
                    target_root,
                    planned,
                    &group,
                    &pending,
                    budget,
                    options,
                    state,
                );
            }
            Ok::<(), String>(())
        },
    );
    if let Err(error) = result {
        record_failure(state, format!("restore phase failed: {error}"));
    }
}

pub fn download_manifest(
    store: &dyn BoxObjectStore,
    target_root: &Path,
    manifest: &BTreeMap<String, BoxStoreManifestEntry>,
    options: BoxStoreDownloadOptions,
) -> BoxStoreDownloadSummary {
    if let Err(error) = fs::create_dir_all(target_root) {
        return BoxStoreDownloadSummary {
            manifest_entries: manifest.len(),
            failures: vec![format!("create restore target root: {error}")],
            ..BoxStoreDownloadSummary::default()
        };
    }

    let planned = plan_download(target_root, manifest, options.large_object_threshold);
    let state = Arc::new(Mutex::new(BoxStoreDownloadSummary {
        manifest_entries: manifest.len(),
        failures: planned.failures.clone(),
        ..BoxStoreDownloadSummary::default()
    }));
    let file_entry_count = manifest.values().filter(|entry| entry.is_file()).count();
    let symlink_entry_count = planned.symlinks.len();
    let trace_state = Mutex::new(SymlinkTraceState::default());

    emit_symlink_trace(
        &options,
        &trace_state,
        manifest.len(),
        file_entry_count,
        symlink_entry_count,
        "manifest-planned",
        None,
        None,
        None,
    );

    let budget = Arc::new(BoxStoreByteBudget::new(options.download_byte_budget));
    run_download_phase(
        store,
        target_root,
        &planned,
        &planned.critical_small,
        options.download_concurrency,
        false,
        budget.as_ref(),
        &options,
        &state,
    );
    run_download_phase(
        store,
        target_root,
        &planned,
        &planned.critical_large,
        COPY_IN_LARGE_BLOB_CONCURRENCY,
        true,
        budget.as_ref(),
        &options,
        &state,
    );

    let restored_by_pack = restore_bulk_small_from_packs(
        store,
        target_root,
        &planned,
        options.owner,
        Arc::clone(&budget),
        &options,
        Arc::clone(&state),
    );

    let remaining_bulk_small = planned
        .bulk_small
        .iter()
        .filter_map(|group| {
            let rel_paths = group
                .rel_paths
                .iter()
                .filter(|path| !restored_by_pack.contains(*path))
                .cloned()
                .collect::<Vec<_>>();
            (!rel_paths.is_empty()).then(|| BoxStoreBlobGroup {
                sha: group.sha.clone(),
                size: group.size,
                rel_paths,
            })
        })
        .collect::<Vec<_>>();

    std::thread::scope(|scope| {
        let small = scope.spawn(|| {
            run_download_phase(
                store,
                target_root,
                &planned,
                &remaining_bulk_small,
                options.download_concurrency,
                false,
                budget.as_ref(),
                &options,
                &state,
            )
        });
        let large = scope.spawn(|| {
            run_download_phase(
                store,
                target_root,
                &planned,
                &planned.bulk_large,
                COPY_IN_LARGE_BLOB_CONCURRENCY,
                true,
                budget.as_ref(),
                &options,
                &state,
            )
        });
        if small.join().is_err() {
            record_failure(&state, "bulk small restore worker panicked".into());
        }
        if large.join().is_err() {
            record_failure(&state, "bulk large restore worker panicked".into());
        }
    });

    if !planned.symlinks.is_empty() {
        emit_symlink_trace(
            &options,
            &trace_state,
            manifest.len(),
            file_entry_count,
            symlink_entry_count,
            "symlink-phase-started",
            None,
            None,
            None,
        );
        let indexed = planned
            .symlinks
            .iter()
            .enumerate()
            .map(|(index, (path, target))| (index + 1, path.clone(), target.clone()))
            .collect::<Vec<_>>();

        let _ = for_each_bounded(
            &indexed,
            options.download_concurrency.max(1),
            |(ordinal, rel_path, target)| {
                {
                    let mut trace = trace_state.lock().expect("symlink trace lock");
                    trace.started = trace.started.saturating_add(1);
                }
                emit_symlink_trace(
                    &options,
                    &trace_state,
                    manifest.len(),
                    file_entry_count,
                    symlink_entry_count,
                    "symlink-entry-started",
                    Some(*ordinal),
                    None,
                    None,
                );

                let restored = (|| -> Result<(), String> {
                    let destination = planned
                        .destinations
                        .get(rel_path)
                        .ok_or_else(|| "unsafe manifest path".to_string())?;
                    if !symlink_target_stays_within_root(target_root, destination, target) {
                        return Err("unsafe symlink target".into());
                    }

                    run_symlink_step(
                        &options,
                        &trace_state,
                        manifest.len(),
                        file_entry_count,
                        symlink_entry_count,
                        *ordinal,
                        "prepare-parent",
                        || ensure_owned_parent(target_root, destination, options.owner),
                    )?;

                    let matches = run_symlink_step(
                        &options,
                        &trace_state,
                        manifest.len(),
                        file_entry_count,
                        symlink_entry_count,
                        *ordinal,
                        "match-existing",
                        || {
                            Ok(fs::symlink_metadata(destination)
                                .ok()
                                .is_some_and(|metadata| metadata.file_type().is_symlink())
                                && fs::read_link(destination)
                                    .ok()
                                    .as_deref()
                                    == Some(Path::new(target)))
                        },
                    )?;
                    if matches {
                        return run_symlink_step(
                            &options,
                            &trace_state,
                            manifest.len(),
                            file_entry_count,
                            symlink_entry_count,
                            *ordinal,
                            "apply-owner",
                            || apply_owner(destination, options.owner, true),
                        );
                    }

                    let temp = unique_temp_path(destination);
                    let work = (|| -> Result<(), String> {
                        run_symlink_step(
                            &options,
                            &trace_state,
                            manifest.len(),
                            file_entry_count,
                            symlink_entry_count,
                            *ordinal,
                            "create-temp",
                            || {
                                #[cfg(unix)]
                                {
                                    std::os::unix::fs::symlink(target, &temp)
                                        .map_err(|error| error.to_string())
                                }
                                #[cfg(not(unix))]
                                {
                                    Err("symlink restore unsupported on this platform".into())
                                }
                            },
                        )?;

                        let destination_is_dir = run_symlink_step(
                            &options,
                            &trace_state,
                            manifest.len(),
                            file_entry_count,
                            symlink_entry_count,
                            *ordinal,
                            "inspect-destination",
                            || match fs::symlink_metadata(destination) {
                                Ok(metadata) => Ok(metadata.is_dir()),
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::NotFound =>
                                {
                                    Ok(false)
                                }
                                Err(error) => Err(error.to_string()),
                            },
                        )?;
                        if destination_is_dir {
                            run_symlink_step(
                                &options,
                                &trace_state,
                                manifest.len(),
                                file_entry_count,
                                symlink_entry_count,
                                *ordinal,
                                "remove-directory",
                                || {
                                    fs::remove_dir_all(destination)
                                        .map_err(|error| error.to_string())
                                },
                            )?;
                        }
                        run_symlink_step(
                            &options,
                            &trace_state,
                            manifest.len(),
                            file_entry_count,
                            symlink_entry_count,
                            *ordinal,
                            "rename-temp",
                            || fs::rename(&temp, destination).map_err(|error| error.to_string()),
                        )?;
                        run_symlink_step(
                            &options,
                            &trace_state,
                            manifest.len(),
                            file_entry_count,
                            symlink_entry_count,
                            *ordinal,
                            "apply-owner",
                            || apply_owner(destination, options.owner, true),
                        )?;
                        Ok(())
                    })();

                    let cleanup = run_symlink_step(
                        &options,
                        &trace_state,
                        manifest.len(),
                        file_entry_count,
                        symlink_entry_count,
                        *ordinal,
                        "cleanup-temp",
                        || {
                            let _ = fs::remove_file(&temp);
                            Ok(())
                        },
                    );
                    work?;
                    cleanup?;

                    let verified_target = run_symlink_step(
                        &options,
                        &trace_state,
                        manifest.len(),
                        file_entry_count,
                        symlink_entry_count,
                        *ordinal,
                        "verify-target",
                        || {
                            Ok(fs::symlink_metadata(destination)
                                .ok()
                                .is_some_and(|metadata| metadata.file_type().is_symlink())
                                && fs::read_link(destination)
                                    .ok()
                                    .as_deref()
                                    == Some(Path::new(target)))
                        },
                    )?;
                    if !verified_target {
                        return Err("restored symlink target does not match".into());
                    }
                    Ok(())
                })();

                {
                    let mut trace = trace_state.lock().expect("symlink trace lock");
                    trace.completed = trace.completed.saturating_add(1);
                }
                emit_symlink_trace(
                    &options,
                    &trace_state,
                    manifest.len(),
                    file_entry_count,
                    symlink_entry_count,
                    "symlink-entry-finished",
                    Some(*ordinal),
                    None,
                    Some(if restored.is_ok() { "restored" } else { "failed" }),
                );

                match restored {
                    Ok(()) => {
                        let mut summary = state.lock().expect("download summary lock");
                        summary.verified = summary.verified.saturating_add(1);
                        summary.files = summary.files.saturating_add(1);
                        drop(summary);
                        report_progress(&options, &state, manifest.len());
                    }
                    Err(error) => record_failure(&state, format!("{rel_path}: {error}")),
                }
                Ok::<(), String>(())
            },
        );

        emit_symlink_trace(
            &options,
            &trace_state,
            manifest.len(),
            file_entry_count,
            symlink_entry_count,
            "symlink-phase-finished",
            None,
            None,
            None,
        );
    }

    let summary = state.lock().expect("download summary mutex").clone();
    summary
}

pub fn remove_existing_restore_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(|error| error.to_string()),
        Ok(_) => fs::remove_file(path).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
