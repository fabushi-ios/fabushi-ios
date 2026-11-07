use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use super::box_object_store::BoxObjectStore;
use super::box_store_diagnostics::report_box_store_diagnostic;
use super::box_store_hydration::{
    is_hydration_handoff_manifest_path, remove_hydration_handoff_marker,
    write_hydration_handoff_marker,
};
use super::box_store_manifest_format::{
    BOX_STORE_LEGACY_MANIFEST_VERSION, BOX_STORE_MANIFEST_REL_PATH, BOX_STORE_MANIFEST_VERSION,
    BoxStoreManifest, BoxStoreManifestEntry, parse_box_store_manifest,
};
use super::object_store_port::{
    BoxStoreCanonicalWriteConflictError, BoxStoreConditionalWriteOutcome,
};

pub const AGENT_STORE_DB_BASENAMES: [&str; 2] = ["store.db", "conversation-blobs.db"];
pub type BoxManifestMap = BTreeMap<String, BoxStoreManifestEntry>;

pub fn count_store_db_manifest_entries(manifest: Option<&BoxManifestMap>) -> usize {
    manifest
        .into_iter()
        .flat_map(|manifest| manifest.iter())
        .filter(|(path, entry)| {
            extract_store_db_agent_id(path).is_some() && !entry.is_symlink()
        })
        .count()
}

pub fn get_store_db_manifest_agent_ids(manifest: Option<&BoxManifestMap>) -> HashSet<String> {
    manifest
        .into_iter()
        .flat_map(|manifest| manifest.iter())
        .filter_map(|(path, entry)| {
            (!entry.is_symlink())
                .then(|| extract_store_db_agent_id(path))
                .flatten()
                .map(str::to_string)
        })
        .collect()
}

pub fn count_agent_dir_manifest_entries(manifest: Option<&BoxManifestMap>) -> usize {
    manifest
        .into_iter()
        .flat_map(|manifest| manifest.keys())
        .filter_map(|path| extract_agent_dir_id(path).map(str::to_string))
        .collect::<HashSet<_>>()
        .len()
}

fn extract_store_db_agent_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("home/box/sand-data/agents/")?;
    let (agent_id, basename) = rest.split_once('/')?;
    (basename == "store.db" && !agent_id.is_empty()).then_some(agent_id)
}

fn extract_agent_dir_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("home/box/sand-data/agents/")?;
    let (agent_id, _) = rest.split_once('/')?;
    (!agent_id.is_empty()).then_some(agent_id)
}

pub fn has_manifest_path_conflict<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    let paths = paths.into_iter().collect::<HashSet<_>>();
    for path in &paths {
        for (separator, _) in path.match_indices('/') {
            if paths.contains(&path[..separator]) {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestCapture {
    pub revision: u64,
    pub entries: BoxManifestMap,
}

#[derive(Debug, Default)]
pub struct BoxStoreManifestRevisionState {
    revision: u64,
    persisted: Option<(u64, Option<bool>)>,
    pending_revision_counts: HashMap<u64, usize>,
    construction_count: u64,
}

impl BoxStoreManifestRevisionState {
    pub fn with_persisted(revision: u64, fully_hydrated: Option<bool>) -> Self {
        Self {
            persisted: Some((revision, fully_hydrated)),
            ..Self::default()
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn persisted_revision(&self) -> Option<u64> {
        self.persisted.map(|value| value.0)
    }

    pub fn snapshot_construction_count(&self) -> u64 {
        self.construction_count
    }

    pub fn has_pending_write_for_revision(&self) -> bool {
        self.pending_revision_counts
            .get(&self.revision)
            .copied()
            .unwrap_or_default()
            > 0
    }

    pub fn mark_changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    pub fn capture(
        &mut self,
        manifest: &BoxManifestMap,
        fully_hydrated: Option<bool>,
        is_required_write: bool,
    ) -> Option<ManifestCapture> {
        let pending = self
            .pending_revision_counts
            .get(&self.revision)
            .copied()
            .unwrap_or_default();
        if !is_required_write
            && (pending > 0 || self.persisted == Some((self.revision, fully_hydrated)))
        {
            return None;
        }
        self.construction_count = self.construction_count.saturating_add(1);
        *self
            .pending_revision_counts
            .entry(self.revision)
            .or_default() += 1;
        Some(ManifestCapture {
            revision: self.revision,
            entries: manifest.clone(),
        })
    }

    pub fn mark_persisted(&mut self, capture: &ManifestCapture, fully_hydrated: Option<bool>) {
        self.persisted = Some((capture.revision, fully_hydrated));
    }

    pub fn mark_failed(&mut self) {
        self.persisted = None;
    }

    pub fn release(&mut self, capture: &ManifestCapture) {
        let Some(count) = self.pending_revision_counts.get_mut(&capture.revision) else {
            return;
        };
        if *count <= 1 {
            self.pending_revision_counts.remove(&capture.revision);
        } else {
            *count -= 1;
        }
    }
}

pub fn set_manifest_entry(
    manifest: &mut BoxManifestMap,
    rel_path: &str,
    entry: BoxStoreManifestEntry,
) -> bool {
    let mut changed = false;
    for ancestor in ancestor_paths(rel_path) {
        changed |= manifest.remove(&ancestor).is_some();
    }
    let prefix = format!("{rel_path}/");
    let descendants = manifest
        .keys()
        .filter(|path| path.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    for descendant in descendants {
        changed |= manifest.remove(&descendant).is_some();
    }
    if manifest.get(rel_path) == Some(&entry) {
        return changed;
    }
    manifest.insert(rel_path.to_string(), entry);
    true
}

fn ancestor_paths(rel_path: &str) -> Vec<String> {
    rel_path
        .match_indices('/')
        .map(|(index, _)| rel_path[..index].to_string())
        .collect()
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxStoreManifestParseError {
    UnsupportedVersion(u64),
    PathConflict,
}

impl fmt::Display for BoxStoreManifestParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported box store manifest version {version}")
            }
            Self::PathConflict => {
                write!(formatter, "box store manifest contains conflicting node paths")
            }
        }
    }
}

impl std::error::Error for BoxStoreManifestParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestLoad {
    pub baseline: Option<Vec<u8>>,
    pub manifest: BoxManifestMap,
    pub manifest_v2: bool,
    pub fully_hydrated: Option<bool>,
    pub present: bool,
    pub canonical_snapshot: bool,
    pub invalid_nonblocking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestHydrationUpdate {
    MarkIncomplete,
    PromoteComplete,
    ResetComplete,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ManifestSaveOptions {
    pub accept_matching_canonical_on_conflict: bool,
    pub is_forced: bool,
    pub hydration_update: Option<ManifestHydrationUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestWriteResult {
    pub attempts: usize,
    pub written: bool,
    pub accepted_matching_canonical: bool,
    pub fully_hydrated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreManifestConflictInfo {
    pub store_id: String,
    pub attempts: usize,
    pub accepted: bool,
    pub covered: bool,
    pub canonical_matches_attempt: bool,
    pub live_view_changed: bool,
    pub attempted_entries: usize,
    pub last_base_etag: Option<String>,
    pub last_baseline_source: Option<String>,
    pub last_conflict_rel_path: Option<String>,
    pub canonical_readable: bool,
    pub canonical_entry_count: Option<usize>,
    pub canonical_updated_at_ms: Option<u64>,
    pub canonical_writer_window_id: Option<String>,
    pub our_window_id: Option<String>,
}

pub fn parse_manifest_bytes(
    bytes: &[u8],
) -> Result<Option<BoxStoreManifest>, BoxStoreManifestParseError> {
    let value = match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if let Some(version) = value
        .as_object()
        .and_then(|map| map.get("version"))
        .and_then(Value::as_u64)
    {
        if version != BOX_STORE_LEGACY_MANIFEST_VERSION && version != BOX_STORE_MANIFEST_VERSION {
            return Err(BoxStoreManifestParseError::UnsupportedVersion(version));
        }
    }
    let Some(manifest) = parse_box_store_manifest(&value) else {
        return Ok(None);
    };
    if has_manifest_path_conflict(manifest.entries.keys().map(String::as_str)) {
        return Err(BoxStoreManifestParseError::PathConflict);
    }
    Ok(Some(manifest))
}

pub fn configured_manifest_entry(
    entry: &BoxStoreManifestEntry,
    manifest_v2: bool,
) -> Option<BoxStoreManifestEntry> {
    if manifest_v2 {
        return Some(entry.clone());
    }
    match entry {
        BoxStoreManifestEntry::Symlink { .. } => None,
        BoxStoreManifestEntry::LegacyFile { sha, size }
        | BoxStoreManifestEntry::File {
            sha,
            size,
            mode: _,
        } => Some(BoxStoreManifestEntry::LegacyFile {
            sha: sha.clone(),
            size: *size,
        }),
    }
}

pub fn configured_manifest_entries(
    entries: &BoxManifestMap,
    manifest_v2: bool,
) -> BoxManifestMap {
    entries
        .iter()
        .filter_map(|(path, entry)| {
            configured_manifest_entry(entry, manifest_v2)
                .map(|configured| (path.clone(), configured))
        })
        .collect()
}

fn manifest_entry_value(entry: &BoxStoreManifestEntry) -> Value {
    let mut value = Map::new();
    match entry {
        BoxStoreManifestEntry::LegacyFile { sha, size } => {
            value.insert("sha".into(), Value::String(sha.clone()));
            value.insert("size".into(), Value::from(*size));
        }
        BoxStoreManifestEntry::File { sha, size, mode } => {
            value.insert("kind".into(), Value::String("file".into()));
            value.insert("sha".into(), Value::String(sha.clone()));
            value.insert("size".into(), Value::from(*size));
            value.insert("mode".into(), Value::from(*mode));
        }
        BoxStoreManifestEntry::Symlink { target } => {
            value.insert("kind".into(), Value::String("symlink".into()));
            value.insert("target".into(), Value::String(target.clone()));
        }
    }
    Value::Object(value)
}

pub fn serialize_manifest_bytes(
    entries: &BoxManifestMap,
    manifest_v2: bool,
    writer_window_id: Option<&str>,
    fully_hydrated: Option<bool>,
    updated_at_ms: u64,
) -> Result<Vec<u8>, String> {
    let configured = configured_manifest_entries(entries, manifest_v2);
    let mut serialized_entries = Map::new();
    for (path, entry) in configured {
        serialized_entries.insert(path, manifest_entry_value(&entry));
    }

    let mut payload = Map::new();
    payload.insert(
        "version".into(),
        Value::from(if manifest_v2 {
            BOX_STORE_MANIFEST_VERSION
        } else {
            BOX_STORE_LEGACY_MANIFEST_VERSION
        }),
    );
    payload.insert("updatedAtMs".into(), Value::from(updated_at_ms));
    if let Some(writer_window_id) = writer_window_id {
        payload.insert(
            "writerWindowId".into(),
            Value::String(writer_window_id.to_string()),
        );
    }
    if let Some(fully_hydrated) = fully_hydrated {
        payload.insert("fullyHydrated".into(), Value::Bool(fully_hydrated));
    }
    payload.insert("entries".into(), Value::Object(serialized_entries));
    serde_json::to_vec(&Value::Object(payload)).map_err(|error| error.to_string())
}

pub fn load_manifest_for_write(
    store: &dyn BoxObjectStore,
    manifest_v2_requested: bool,
    hydration_handoff_marker_path: Option<&Path>,
) -> Result<ManifestLoad, String> {
    let baseline = store.get(BOX_STORE_MANIFEST_REL_PATH)?;
    let hydration_handoff_pending =
        hydration_handoff_marker_path.is_some_and(Path::exists);

    let Some(bytes) = baseline.as_deref() else {
        if hydration_handoff_pending {
            return Err("manifest missing while legacy hydration handoff is pending".into());
        }
        return Ok(ManifestLoad {
            baseline,
            manifest: BoxManifestMap::new(),
            manifest_v2: manifest_v2_requested,
            fully_hydrated: None,
            present: false,
            canonical_snapshot: true,
            invalid_nonblocking: false,
        });
    };

    let decoded = match parse_manifest_bytes(bytes).map_err(|error| error.to_string())? {
        Some(decoded) => decoded,
        None if hydration_handoff_pending => {
            return Err("manifest unreadable while legacy hydration handoff is pending".into());
        }
        None => {
            return Ok(ManifestLoad {
                baseline,
                manifest: BoxManifestMap::new(),
                manifest_v2: manifest_v2_requested,
                fully_hydrated: None,
                present: true,
                canonical_snapshot: false,
                invalid_nonblocking: true,
            });
        }
    };

    if hydration_handoff_pending && decoded.fully_hydrated.is_none() {
        if let Some(marker_path) = hydration_handoff_marker_path {
            remove_hydration_handoff_marker(marker_path).map_err(|error| error.to_string())?;
        }
    }

    let manifest_v2 =
        manifest_v2_requested || decoded.version == BOX_STORE_MANIFEST_VERSION;
    let fully_hydrated = decoded.fully_hydrated;
    let mut manifest = decoded.entries;
    let before = manifest.len();
    manifest.retain(|path, _| !is_hydration_handoff_manifest_path(path));
    let canonical_snapshot = manifest.len() == before;

    Ok(ManifestLoad {
        baseline,
        manifest,
        manifest_v2,
        fully_hydrated,
        present: true,
        canonical_snapshot,
        invalid_nonblocking: false,
    })
}

pub fn read_manifest_strict(
    store: &dyn BoxObjectStore,
) -> Result<Option<BoxStoreManifest>, String> {
    let Some(bytes) = store.get(BOX_STORE_MANIFEST_REL_PATH)? else {
        return Ok(None);
    };
    parse_manifest_bytes(&bytes)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "box store manifest is present but unparseable".to_string())
        .map(Some)
}

pub fn prepare_canonical_manifest_reset(store: &dyn BoxObjectStore) -> Result<(), String> {
    let Some(bytes) = store.get(BOX_STORE_MANIFEST_REL_PATH)? else {
        return Ok(());
    };
    match parse_manifest_bytes(&bytes) {
        Ok(_) | Err(BoxStoreManifestParseError::PathConflict) => Ok(()),
        Err(error @ BoxStoreManifestParseError::UnsupportedVersion(_)) => Err(error.to_string()),
    }
}

fn apply_hydration_update(
    fully_hydrated: Option<bool>,
    update: Option<ManifestHydrationUpdate>,
) -> Option<bool> {
    match update {
        Some(ManifestHydrationUpdate::MarkIncomplete) => Some(false),
        Some(ManifestHydrationUpdate::PromoteComplete | ManifestHydrationUpdate::ResetComplete) => {
            Some(true)
        }
        None => fully_hydrated,
    }
}

fn manifest_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn canonical_matches_attempt(
    canonical: &BoxStoreManifest,
    version: u64,
    entries: &BoxManifestMap,
    fully_hydrated: Option<bool>,
) -> bool {
    canonical.version == version
        && canonical.entries == *entries
        && canonical.fully_hydrated == fully_hydrated
}

fn optional_string(value: Option<&str>) -> Value {
    value
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null)
}

fn report_manifest_write_conflict_diagnostic(info: &BoxStoreManifestConflictInfo) {
    let mut diagnostic = Map::new();
    diagnostic.insert("extension".into(), Value::String("box_store".into()));
    diagnostic.insert(
        "kind".into(),
        Value::String("manifest_write_conflict".into()),
    );
    diagnostic.insert("storeId".into(), Value::String(info.store_id.clone()));
    diagnostic.insert("attempts".into(), Value::from(info.attempts as u64));
    diagnostic.insert("accepted".into(), Value::Bool(info.accepted));
    diagnostic.insert("covered".into(), Value::Bool(info.covered));
    diagnostic.insert(
        "canonicalMatchesAttempt".into(),
        Value::Bool(info.canonical_matches_attempt),
    );
    diagnostic.insert("liveViewChanged".into(), Value::Bool(info.live_view_changed));
    diagnostic.insert(
        "attemptedEntries".into(),
        Value::from(info.attempted_entries as u64),
    );
    diagnostic.insert(
        "lastBaseEtag".into(),
        optional_string(info.last_base_etag.as_deref()),
    );
    diagnostic.insert(
        "lastBaselineSource".into(),
        optional_string(info.last_baseline_source.as_deref()),
    );
    diagnostic.insert(
        "lastConflictRelPath".into(),
        optional_string(info.last_conflict_rel_path.as_deref()),
    );
    diagnostic.insert("canonicalReadable".into(), Value::Bool(info.canonical_readable));
    diagnostic.insert(
        "canonicalEntryCount".into(),
        info.canonical_entry_count
            .map(|value| Value::from(value as u64))
            .unwrap_or(Value::Null),
    );
    diagnostic.insert(
        "canonicalUpdatedAtMs".into(),
        info.canonical_updated_at_ms
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    diagnostic.insert(
        "canonicalWriterWindowId".into(),
        optional_string(info.canonical_writer_window_id.as_deref()),
    );
    diagnostic.insert("ourWindowId".into(), optional_string(info.our_window_id.as_deref()));
    report_box_store_diagnostic(&diagnostic);
}

pub fn write_manifest_with_retry(
    store: &dyn BoxObjectStore,
    store_id: &str,
    baseline: Option<Vec<u8>>,
    entries: &BoxManifestMap,
    manifest_v2: bool,
    writer_window_id: Option<&str>,
    fully_hydrated: Option<bool>,
    options: ManifestSaveOptions,
    retry_attempts: usize,
    retry_delay_ms: u64,
    log: &dyn Fn(&str),
    is_disposed: &dyn Fn() -> bool,
) -> Result<ManifestWriteResult, String> {
    write_manifest_with_retry_and_conflict_reporter(
        store,
        store_id,
        baseline,
        entries,
        manifest_v2,
        writer_window_id,
        fully_hydrated,
        options,
        retry_attempts,
        retry_delay_ms,
        log,
        is_disposed,
        &|_| {},
    )
}

pub fn write_manifest_with_retry_and_conflict_reporter(
    store: &dyn BoxObjectStore,
    store_id: &str,
    baseline: Option<Vec<u8>>,
    entries: &BoxManifestMap,
    manifest_v2: bool,
    writer_window_id: Option<&str>,
    fully_hydrated: Option<bool>,
    options: ManifestSaveOptions,
    retry_attempts: usize,
    retry_delay_ms: u64,
    log: &dyn Fn(&str),
    is_disposed: &dyn Fn() -> bool,
    on_manifest_write_conflict: &dyn Fn(&BoxStoreManifestConflictInfo),
) -> Result<ManifestWriteResult, String> {
    let configured_entries = configured_manifest_entries(entries, manifest_v2);
    let version = if manifest_v2 {
        BOX_STORE_MANIFEST_VERSION
    } else {
        BOX_STORE_LEGACY_MANIFEST_VERSION
    };
    let fully_hydrated = apply_hydration_update(fully_hydrated, options.hydration_update);
    let required_write = options.accept_matching_canonical_on_conflict
        || options.is_forced
        || options.hydration_update.is_some();

    if !required_write {
        if let Some(bytes) = baseline.as_deref() {
            if let Some(canonical) =
                parse_manifest_bytes(bytes).map_err(|error| error.to_string())?
            {
                if canonical_matches_attempt(
                    &canonical,
                    version,
                    &configured_entries,
                    fully_hydrated,
                ) {
                    return Ok(ManifestWriteResult {
                        attempts: 0,
                        written: false,
                        accepted_matching_canonical: false,
                        fully_hydrated,
                    });
                }
            }
        }
    }

    let retry_attempts = retry_attempts.max(1);
    let mut baseline = baseline;
    let mut last_conflict: Option<BoxStoreCanonicalWriteConflictError> = None;
    let mut canonical_after_conflict: Option<BoxStoreManifest> = None;
    let mut attempts = 0;

    for attempt in 1..=retry_attempts {
        attempts = attempt;
        if attempt > 1 && last_conflict.is_some() && is_disposed() {
            break;
        }

        let bytes = serialize_manifest_bytes(
            &configured_entries,
            manifest_v2,
            writer_window_id,
            fully_hydrated,
            manifest_now_ms(),
        )?;
        match store.put_if_unchanged_detailed(
            BOX_STORE_MANIFEST_REL_PATH,
            baseline.as_deref(),
            &bytes,
        )? {
            BoxStoreConditionalWriteOutcome::Written => {
                return Ok(ManifestWriteResult {
                    attempts,
                    written: true,
                    accepted_matching_canonical: false,
                    fully_hydrated,
                });
            }
            BoxStoreConditionalWriteOutcome::Conflict(conflict) => {
                let canonical_bytes = store.get(BOX_STORE_MANIFEST_REL_PATH)?;
                canonical_after_conflict = match canonical_bytes.as_deref() {
                    Some(bytes) => parse_manifest_bytes(bytes)
                        .map_err(|error| error.to_string())?,
                    None => None,
                };
                baseline = canonical_bytes;
                log(&format!(
                    "manifest save lost a concurrent-write race; retry policy will use the winner's baseline{}",
                    conflict
                        .conflict_rel_path
                        .as_ref()
                        .map(|path| format!(" (lost attempt preserved at {path})"))
                        .unwrap_or_default()
                ));
                last_conflict = Some(conflict);
                if attempt < retry_attempts {
                    if retry_delay_ms > 0 {
                        thread::sleep(Duration::from_millis(retry_delay_ms));
                    }
                    continue;
                }
            }
        }
    }

    let conflict = last_conflict.unwrap_or_else(|| {
        BoxStoreCanonicalWriteConflictError::new(
            BOX_STORE_MANIFEST_REL_PATH,
            None,
            None,
            Some("manifest-cas".into()),
        )
    });
    let covered = canonical_after_conflict.as_ref().is_some_and(|canonical| {
        !configured_entries.is_empty()
            && configured_entries
                .iter()
                .all(|(path, entry)| canonical.entries.get(path) == Some(entry))
    });
    let canonical_matches = canonical_after_conflict.as_ref().is_some_and(|canonical| {
        covered
            && canonical.entries.len() == configured_entries.len()
            && canonical.fully_hydrated == fully_hydrated
            && canonical.version == version
    });
    let accepted =
        options.accept_matching_canonical_on_conflict && canonical_matches;
    let conflict_info = BoxStoreManifestConflictInfo {
        store_id: store_id.to_string(),
        attempts,
        accepted,
        covered,
        canonical_matches_attempt: canonical_matches,
        live_view_changed: false,
        attempted_entries: configured_entries.len(),
        last_base_etag: conflict.base_etag.clone(),
        last_baseline_source: conflict.baseline_source.clone(),
        last_conflict_rel_path: conflict.conflict_rel_path.clone(),
        canonical_readable: canonical_after_conflict.is_some(),
        canonical_entry_count: canonical_after_conflict
            .as_ref()
            .map(|manifest| manifest.entries.len()),
        canonical_updated_at_ms: canonical_after_conflict
            .as_ref()
            .map(|manifest| manifest.updated_at_ms),
        canonical_writer_window_id: canonical_after_conflict
            .as_ref()
            .and_then(|manifest| manifest.writer_window_id.clone()),
        our_window_id: writer_window_id.map(str::to_string),
    };
    report_manifest_write_conflict_diagnostic(&conflict_info);
    on_manifest_write_conflict(&conflict_info);
    if accepted {
        log(&format!(
            "manifest save lost a concurrent-write race; canonical manifest already covers the attempted entries (winner {})",
            canonical_after_conflict
                .as_ref()
                .and_then(|manifest| manifest.writer_window_id.as_deref())
                .unwrap_or("unknown")
        ));
        return Ok(ManifestWriteResult {
            attempts,
            written: false,
            accepted_matching_canonical: true,
            fully_hydrated,
        });
    }
    Err(conflict.to_string())
}

pub fn mark_legacy_hydration_incomplete(
    store: &dyn BoxObjectStore,
    store_id: &str,
    marker_path: &Path,
    manifest_v2_requested: bool,
    writer_window_id: Option<&str>,
    retry_attempts: usize,
    retry_delay_ms: u64,
    log: &dyn Fn(&str),
) -> Result<ManifestWriteResult, String> {
    remove_hydration_handoff_marker(marker_path).map_err(|error| error.to_string())?;
    let load = load_manifest_for_write(store, manifest_v2_requested, None)?;
    write_manifest_with_retry(
        store,
        store_id,
        load.baseline,
        &load.manifest,
        load.manifest_v2,
        writer_window_id,
        load.fully_hydrated,
        ManifestSaveOptions {
            hydration_update: Some(ManifestHydrationUpdate::MarkIncomplete),
            ..ManifestSaveOptions::default()
        },
        retry_attempts,
        retry_delay_ms,
        log,
        &|| false,
    )
}

pub fn mark_legacy_hydration_complete_for_handoff(marker_path: &Path) -> Result<(), String> {
    write_hydration_handoff_marker(marker_path).map_err(|error| error.to_string())
}
