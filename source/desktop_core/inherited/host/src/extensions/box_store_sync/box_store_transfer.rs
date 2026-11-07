use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::extensions::box_store_sync::box_object_store::BoxObjectStore;
use crate::extensions::box_store_sync::box_store_download::{
    BOX_STORE_RESTORE_TMP_SUFFIX, LARGE_OBJECT_FREE_SPACE_FACTOR,
};
use crate::extensions::box_store_sync::box_store_manifest::{
    AGENT_STORE_DB_BASENAMES, BoxManifestMap, set_manifest_entry,
};
use crate::extensions::box_store_sync::box_store_manifest_format::{
    BOX_STORE_BLOBS_PREFIX, BoxStoreManifestEntry,
};
use crate::extensions::box_store_sync::workspace_ignore::WorkspaceIgnore;
use crate::r#box::box_transfer::for_each_bounded;

pub const SNAPSHOT_OUT_LARGE_CONCURRENCY: usize = 2;
pub const BOX_STORE_SNAPSHOT_TMP_SUFFIX: &str = ".box-store-snap-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    Uploaded,
    Unchanged,
    Oversize,
    MetadataError,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransferSummary {
    pub files_uploaded: usize,
    pub bytes_uploaded: u64,
    pub skipped_unchanged: usize,
    pub oversize: usize,
    pub failures: usize,
    pub metadata_failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CategoryTransferSummary {
    pub name: String,
    pub files_scanned: usize,
    pub files_uploaded: usize,
    pub bytes_uploaded: u64,
    pub skipped_unchanged: usize,
    pub removed: usize,
    pub oversize: usize,
    pub failures: usize,
    pub metadata_failures: usize,
    pub excluded_files: usize,
    pub excluded_bytes: u64,
    pub pruned_dirs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedTransferFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalStat {
    mtime_ms: u128,
    size: u64,
    sha: String,
    mode: u32,
}

#[derive(Debug, Clone)]
struct Candidate {
    ordinal: usize,
    rel_path: String,
    abs_path: PathBuf,
    file_stat: FileStat,
    mode_override: Option<u32>,
    bypass_stat_cache: bool,
}

#[derive(Debug, Clone, Copy)]
struct FileStat {
    mtime_ms: u128,
    size: u64,
    mode: u32,
}

#[derive(Debug)]
struct ProcessedCandidate {
    ordinal: usize,
    rel_path: String,
    outcome: FileOutcome,
    tally_size: u64,
    scanned: bool,
    entry: Option<BoxStoreManifestEntry>,
    symlink: bool,
}

pub struct BoxStoreTransfer {
    max_object_bytes: u64,
    upload_concurrency: usize,
    large_object_threshold: u64,
    hydration_handoff_marker_path: Option<PathBuf>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    local_stat: Mutex<HashMap<String, LocalStat>>,
}

impl BoxStoreTransfer {
    pub fn new(
        max_object_bytes: u64,
        upload_concurrency: usize,
        large_object_threshold: u64,
        hydration_handoff_marker_path: Option<PathBuf>,
        log: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Self {
        Self {
            max_object_bytes,
            upload_concurrency: upload_concurrency.max(1),
            large_object_threshold: large_object_threshold.max(1),
            hydration_handoff_marker_path,
            log,
            local_stat: Mutex::new(HashMap::new()),
        }
    }

    pub fn log_message(&self, message: &str) {
        (self.log)(message);
    }

    pub fn clear_local_stat(&self) {
        self.local_stat
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub fn forget_local_stat_path(&self, rel_path: &str) {
        self.local_stat
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(rel_path);
    }

    pub fn sync_tree_category(
        &self,
        store: &dyn BoxObjectStore,
        manifest: &mut BoxManifestMap,
        root: &Path,
        rel_prefix: &str,
        name: &str,
        excludes: &[&str],
        ignore: Option<&WorkspaceIgnore>,
        contains_agent_store_dbs: bool,
        manifest_v2: bool,
    ) -> Result<CategoryTransferSummary, String> {
        let mut summary = CategoryTransferSummary {
            name: name.to_string(),
            ..CategoryTransferSummary::default()
        };
        let mut seen = HashSet::<String>::new();
        let mut candidates = Vec::<Candidate>::new();
        let mut walk_complete = true;

        if root.exists() {
            self.collect_tree_candidates(
                root,
                root,
                rel_prefix,
                excludes,
                ignore,
                contains_agent_store_dbs,
                &mut seen,
                &mut candidates,
                &mut summary,
                &mut walk_complete,
            );
        }

        self.upload_candidates(store, manifest, &candidates, manifest_v2, &mut summary)?;

        if walk_complete {
            let prefix = format!("{rel_prefix}/");
            let stale = manifest
                .keys()
                .filter(|path| path.starts_with(&prefix))
                .filter(|path| !seen.contains(*path))
                .filter(|path| !is_excluded(path, excludes))
                .filter(|path| !contains_agent_store_dbs || !is_agent_store_db_path(path))
                .cloned()
                .collect::<Vec<_>>();
            for path in stale {
                manifest.remove(&path);
                self.forget_local_stat_path(&path);
                summary.removed = summary.removed.saturating_add(1);
            }
        }

        if ignore.is_some() && (summary.pruned_dirs > 0 || summary.excluded_files > 0) {
            self.log_message(&format!(
                "{name} ignore: pruned {} dirs, excluded {} files ({}B)",
                summary.pruned_dirs, summary.excluded_files, summary.excluded_bytes
            ));
        }

        Ok(summary)
    }

    pub fn sync_staged_files(
        &self,
        store: &dyn BoxObjectStore,
        manifest: &mut BoxManifestMap,
        name: &str,
        files: &[StagedTransferFile],
        skipped: usize,
        manifest_v2: bool,
    ) -> Result<CategoryTransferSummary, String> {
        let mut summary = CategoryTransferSummary {
            name: name.to_string(),
            ..CategoryTransferSummary::default()
        };
        let mut candidates = Vec::with_capacity(files.len());

        for file in files {
            let metadata = match fs::symlink_metadata(&file.abs_path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    tally_category_metadata_failure(&mut summary);
                    continue;
                }
            };
            if !metadata.file_type().is_file() {
                tally_category_metadata_failure(&mut summary);
                continue;
            }
            candidates.push(Candidate {
                ordinal: candidates.len(),
                rel_path: file.rel_path.clone(),
                abs_path: file.abs_path.clone(),
                file_stat: file_stat(&metadata),
                mode_override: Some(file.mode),
                bypass_stat_cache: true,
            });
        }

        self.upload_candidates(store, manifest, &candidates, manifest_v2, &mut summary)?;
        summary.failures = summary.failures.saturating_add(skipped);
        Ok(summary)
    }

    pub fn sweep_leaked_temps(&self, roots: &[PathBuf]) -> usize {
        let mut removed = 0usize;
        for root in dedupe_nested_roots(roots.iter()) {
            removed = removed.saturating_add(self.sweep_leaked_temps_under(&root));
        }
        removed
    }

    fn sweep_leaked_temps_under(&self, root: &Path) -> usize {
        let Ok(entries) = fs::read_dir(root) else {
            return 0;
        };
        let mut removed = 0usize;
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                removed = removed.saturating_add(self.sweep_leaked_temps_under(&path));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let value = path.to_string_lossy();
            if (value.contains(BOX_STORE_SNAPSHOT_TMP_SUFFIX)
                || value.contains(BOX_STORE_RESTORE_TMP_SUFFIX))
                && fs::remove_file(&path).is_ok()
            {
                removed = removed.saturating_add(1);
            }
        }
        removed
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_tree_candidates(
        &self,
        root: &Path,
        dir: &Path,
        rel_prefix: &str,
        excludes: &[&str],
        ignore: Option<&WorkspaceIgnore>,
        contains_agent_store_dbs: bool,
        seen: &mut HashSet<String>,
        candidates: &mut Vec<Candidate>,
        summary: &mut CategoryTransferSummary,
        walk_complete: &mut bool,
    ) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                tally_category_metadata_failure(summary);
                *walk_complete = false;
                return;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    tally_category_metadata_failure(summary);
                    *walk_complete = false;
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    tally_category_metadata_failure(summary);
                    *walk_complete = false;
                    continue;
                }
            };
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
                Err(_) => {
                    tally_category_metadata_failure(summary);
                    *walk_complete = false;
                    continue;
                }
            };

            if file_type.is_dir() {
                if ignore.is_some_and(|matcher| matcher.can_prune_dir(&relative)) {
                    summary.pruned_dirs = summary.pruned_dirs.saturating_add(1);
                    continue;
                }
                self.collect_tree_candidates(
                    root,
                    &path,
                    rel_prefix,
                    excludes,
                    ignore,
                    contains_agent_store_dbs,
                    seen,
                    candidates,
                    summary,
                    walk_complete,
                );
                continue;
            }

            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }
            if self.is_hydration_marker_path(&path) {
                continue;
            }

            let rel_path = format!("{rel_prefix}/{relative}");
            if is_excluded(&rel_path, excludes)
                || (contains_agent_store_dbs && is_agent_store_db_path(&rel_path))
                || rel_path.contains(BOX_STORE_SNAPSHOT_TMP_SUFFIX)
                || rel_path.contains(BOX_STORE_RESTORE_TMP_SUFFIX)
            {
                continue;
            }

            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => {
                    seen.insert(rel_path);
                    tally_category_metadata_failure(summary);
                    continue;
                }
            };
            if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
                seen.insert(rel_path);
                tally_category_metadata_failure(summary);
                continue;
            }

            if ignore.is_some_and(|matcher| matcher.ignores(&relative)) {
                summary.excluded_files = summary.excluded_files.saturating_add(1);
                summary.excluded_bytes = summary.excluded_bytes.saturating_add(metadata.len());
                continue;
            }

            seen.insert(rel_path.clone());
            candidates.push(Candidate {
                ordinal: candidates.len(),
                rel_path,
                abs_path: path,
                file_stat: file_stat(&metadata),
                mode_override: None,
                bypass_stat_cache: false,
            });
        }
    }

    fn is_hydration_marker_path(&self, path: &Path) -> bool {
        let Some(marker) = self.hydration_handoff_marker_path.as_ref() else {
            return false;
        };
        if path == marker {
            return true;
        }
        let marker = marker.to_string_lossy();
        let value = path.to_string_lossy();
        value.starts_with(&format!("{marker}.")) && value.ends_with(".tmp")
    }

    fn upload_candidates(
        &self,
        store: &dyn BoxObjectStore,
        manifest: &mut BoxManifestMap,
        candidates: &[Candidate],
        manifest_v2: bool,
        summary: &mut CategoryTransferSummary,
    ) -> Result<(), String> {
        let mut small = Vec::new();
        let mut large = Vec::new();
        for candidate in candidates {
            if candidate.file_stat.size >= self.large_object_threshold {
                large.push(candidate.clone());
            } else {
                small.push(candidate.clone());
            }
        }

        self.upload_candidate_group(
            store,
            manifest,
            &small,
            self.upload_concurrency,
            manifest_v2,
            summary,
        )?;
        self.upload_candidate_group(
            store,
            manifest,
            &large,
            SNAPSHOT_OUT_LARGE_CONCURRENCY,
            manifest_v2,
            summary,
        )?;
        Ok(())
    }

    fn upload_candidate_group(
        &self,
        store: &dyn BoxObjectStore,
        manifest: &mut BoxManifestMap,
        candidates: &[Candidate],
        concurrency: usize,
        manifest_v2: bool,
        summary: &mut CategoryTransferSummary,
    ) -> Result<(), String> {
        if candidates.is_empty() {
            return Ok(());
        }
        let manifest_snapshot = manifest.clone();
        let results = Mutex::new(Vec::<ProcessedCandidate>::with_capacity(candidates.len()));
        for_each_bounded(candidates, concurrency, |candidate| {
            let processed =
                self.process_candidate(store, &manifest_snapshot, candidate, manifest_v2);
            results
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(processed);
            Ok::<(), String>(())
        })?;

        let mut results = results
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        results.sort_by_key(|result| result.ordinal);

        for mut result in results {
            if result.scanned {
                summary.files_scanned = summary.files_scanned.saturating_add(1);
            }
            if let Some(entry) = result.entry.take() {
                let changed = set_manifest_entry(manifest, &result.rel_path, entry);
                if result.symlink {
                    result.outcome = if changed {
                        FileOutcome::Uploaded
                    } else {
                        FileOutcome::Unchanged
                    };
                    self.forget_local_stat_path(&result.rel_path);
                }
            }
            tally_category_outcome(summary, result.outcome, result.tally_size);
        }
        Ok(())
    }

    fn process_candidate(
        &self,
        store: &dyn BoxObjectStore,
        manifest: &BoxManifestMap,
        candidate: &Candidate,
        manifest_v2: bool,
    ) -> ProcessedCandidate {
        let metadata = match fs::symlink_metadata(&candidate.abs_path) {
            Ok(metadata) => metadata,
            Err(_) => {
                return ProcessedCandidate {
                    ordinal: candidate.ordinal,
                    rel_path: candidate.rel_path.clone(),
                    outcome: FileOutcome::MetadataError,
                    tally_size: 0,
                    scanned: false,
                    entry: None,
                    symlink: false,
                };
            }
        };

        if metadata.file_type().is_symlink() {
            if !manifest_v2 {
                return ProcessedCandidate {
                    ordinal: candidate.ordinal,
                    rel_path: candidate.rel_path.clone(),
                    outcome: FileOutcome::Error,
                    tally_size: 0,
                    scanned: true,
                    entry: None,
                    symlink: true,
                };
            }
            return match fs::read_link(&candidate.abs_path) {
                Ok(target) => ProcessedCandidate {
                    ordinal: candidate.ordinal,
                    rel_path: candidate.rel_path.clone(),
                    outcome: FileOutcome::Unchanged,
                    tally_size: 0,
                    scanned: true,
                    entry: Some(BoxStoreManifestEntry::Symlink {
                        target: target.to_string_lossy().to_string(),
                    }),
                    symlink: true,
                },
                Err(error) => {
                    self.log_message(&format!(
                        "readlink failed {}: {error}",
                        candidate.rel_path
                    ));
                    ProcessedCandidate {
                        ordinal: candidate.ordinal,
                        rel_path: candidate.rel_path.clone(),
                        outcome: FileOutcome::MetadataError,
                        tally_size: 0,
                        scanned: true,
                        entry: None,
                        symlink: true,
                    }
                }
            };
        }

        if !metadata.file_type().is_file() {
            return ProcessedCandidate {
                ordinal: candidate.ordinal,
                rel_path: candidate.rel_path.clone(),
                outcome: FileOutcome::MetadataError,
                tally_size: 0,
                scanned: true,
                entry: None,
                symlink: false,
            };
        }

        let current = file_stat(&metadata);
        let (outcome, entry) = self.sync_file(
            store,
            manifest.get(&candidate.rel_path),
            &candidate.rel_path,
            &candidate.abs_path,
            current,
            candidate.bypass_stat_cache,
            candidate.mode_override,
            manifest_v2,
        );
        ProcessedCandidate {
            ordinal: candidate.ordinal,
            rel_path: candidate.rel_path.clone(),
            outcome,
            tally_size: current.size,
            scanned: true,
            entry,
            symlink: false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_file(
        &self,
        store: &dyn BoxObjectStore,
        existing: Option<&BoxStoreManifestEntry>,
        rel_path: &str,
        abs_path: &Path,
        file_stat: FileStat,
        bypass_stat_cache: bool,
        mode_override: Option<u32>,
        manifest_v2: bool,
    ) -> (FileOutcome, Option<BoxStoreManifestEntry>) {
        let mut mode = mode_override.unwrap_or(file_stat.mode) & 0o777;

        if file_stat.size > self.max_object_bytes {
            self.log_message(&format!(
                "oversize {rel_path}: {}B over {}B",
                file_stat.size, self.max_object_bytes
            ));
            return (
                classify_file_failure(existing, mode, FileOutcome::Oversize),
                None,
            );
        }

        if !bypass_stat_cache {
            let cached = self
                .local_stat
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(rel_path)
                .cloned();
            if let Some(cached) = cached {
                if cached.mtime_ms == file_stat.mtime_ms
                    && cached.size == file_stat.size
                    && entry_sha_size(existing)
                        .is_some_and(|(sha, size)| sha == cached.sha && size == cached.size)
                {
                    mode = mode_override.unwrap_or(file_stat.mode) & 0o777;
                    return (
                        FileOutcome::Unchanged,
                        Some(file_entry(cached.sha, cached.size, mode, manifest_v2)),
                    );
                }
            }
        }

        if file_stat.size >= self.large_object_threshold {
            return self.sync_large_file(
                store,
                existing,
                rel_path,
                abs_path,
                file_stat.size,
                mode,
                mode_override,
                manifest_v2,
            );
        }

        let mut file = match open_source_nofollow(abs_path) {
            Ok(file) => file,
            Err(error) => {
                self.log_message(&format!("read failed {rel_path}: {error}"));
                let outcome = if is_snapshot_node_race(&error) {
                    FileOutcome::MetadataError
                } else {
                    classify_file_failure(existing, mode, FileOutcome::Error)
                };
                return (outcome, None);
            }
        };
        let live = match file.metadata() {
            Ok(metadata) if metadata.file_type().is_file() => metadata,
            Ok(_) => {
                self.log_message(&format!(
                    "read failed {rel_path}: snapshot source is not a regular file"
                ));
                return (FileOutcome::MetadataError, None);
            }
            Err(error) => {
                self.log_message(&format!("read failed {rel_path}: {error}"));
                return (
                    if is_snapshot_node_race(&error) {
                        FileOutcome::MetadataError
                    } else {
                        classify_file_failure(existing, mode, FileOutcome::Error)
                    },
                    None,
                );
            }
        };
        mode = mode_override.unwrap_or_else(|| file_mode(&live)) & 0o777;
        let mut bytes = Vec::with_capacity(live.len().min(8 * 1024 * 1024) as usize);
        if let Err(error) = file.read_to_end(&mut bytes) {
            self.log_message(&format!("read failed {rel_path}: {error}"));
            return (
                if is_snapshot_node_race(&error) {
                    FileOutcome::MetadataError
                } else {
                    classify_file_failure(existing, mode, FileOutcome::Error)
                },
                None,
            );
        }

        let sha = format!("{:x}", Sha256::digest(&bytes));
        self.local_stat
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                rel_path.to_string(),
                LocalStat {
                    mtime_ms: metadata_mtime_ms(&live),
                    size: bytes.len() as u64,
                    sha: sha.clone(),
                    mode,
                },
            );

        if entry_sha_size(existing)
            .is_some_and(|(existing_sha, size)| existing_sha == sha && size == bytes.len() as u64)
        {
            return (
                FileOutcome::Unchanged,
                Some(file_entry(sha, bytes.len() as u64, mode, manifest_v2)),
            );
        }

        let key = format!("{BOX_STORE_BLOBS_PREFIX}/{sha}");
        if let Err(error) = store.put(&key, &bytes) {
            self.log_message(&format!("upload failed {rel_path}: {error}"));
            return (
                classify_file_failure(existing, mode, FileOutcome::Error),
                None,
            );
        }
        (
            FileOutcome::Uploaded,
            Some(file_entry(sha, bytes.len() as u64, mode, manifest_v2)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_large_file(
        &self,
        store: &dyn BoxObjectStore,
        existing: Option<&BoxStoreManifestEntry>,
        rel_path: &str,
        abs_path: &Path,
        expected_size_bytes: u64,
        expected_mode: u32,
        mode_override: Option<u32>,
        manifest_v2: bool,
    ) -> (FileOutcome, Option<BoxStoreManifestEntry>) {
        if let Some(BoxStoreManifestEntry::File {
            sha,
            size,
            mode: existing_mode,
        }) = existing
        {
            if *existing_mode != expected_mode {
                if let Some(matched) = self.match_existing_large_file(
                    abs_path,
                    sha,
                    *size,
                    expected_mode,
                    mode_override.is_some(),
                ) {
                    self.local_stat
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(rel_path.to_string(), matched.clone());
                    return (
                        FileOutcome::Unchanged,
                        Some(file_entry(
                            matched.sha,
                            matched.size,
                            matched.mode,
                            manifest_v2,
                        )),
                    );
                }
            }
        }

        if !self.has_disk_space_for_large_object(abs_path, expected_size_bytes) {
            self.log_message(&format!(
                "insufficient disk space for {rel_path} snapshot copy ({expected_size_bytes}B needed x{LARGE_OBJECT_FREE_SPACE_FACTOR}); deferring to a later cycle"
            ));
            return (
                classify_file_failure(existing, expected_mode, FileOutcome::Error),
                None,
            );
        }

        let temp_path = PathBuf::from(format!(
            "{}{}{}",
            abs_path.display(),
            BOX_STORE_SNAPSHOT_TMP_SUFFIX,
            Uuid::new_v4().simple()
        ));
        let copied = match copy_file_hashing(abs_path, &temp_path) {
            Ok(copied) => copied,
            Err(error) => {
                self.discard_temp(&temp_path, rel_path);
                self.log_message(&format!("read failed {rel_path}: {error}"));
                return (
                    if is_snapshot_node_race(&error) {
                        FileOutcome::MetadataError
                    } else {
                        classify_file_failure(existing, expected_mode, FileOutcome::Error)
                    },
                    None,
                );
            }
        };
        let mode = mode_override.unwrap_or(copied.mode) & 0o777;

        let result = if copied.size > self.max_object_bytes {
            self.log_message(&format!(
                "oversize {rel_path}: {}B over {}B",
                copied.size, self.max_object_bytes
            ));
            (
                classify_file_failure(existing, mode, FileOutcome::Oversize),
                None,
            )
        } else {
            let already_stored = entry_sha_size(existing)
                .is_some_and(|(sha, size)| sha == copied.sha && size == copied.size);
            if !already_stored {
                let key = format!("{BOX_STORE_BLOBS_PREFIX}/{}", copied.sha);
                if let Err(error) = store.put_from_file(&key, &temp_path) {
                    self.log_message(&format!("upload failed {rel_path}: {error}"));
                    self.discard_temp(&temp_path, rel_path);
                    return (
                        classify_file_failure(existing, mode, FileOutcome::Error),
                        None,
                    );
                }
            }

            self.remember_large_local_stat(rel_path, abs_path, &copied.sha, copied.size, mode);
            (
                if already_stored {
                    FileOutcome::Unchanged
                } else {
                    FileOutcome::Uploaded
                },
                Some(file_entry(copied.sha, copied.size, mode, manifest_v2)),
            )
        };

        self.discard_temp(&temp_path, rel_path);
        result
    }

    fn match_existing_large_file(
        &self,
        abs_path: &Path,
        expected_sha: &str,
        expected_size: u64,
        expected_mode: u32,
        has_mode_override: bool,
    ) -> Option<LocalStat> {
        let before = fs::symlink_metadata(abs_path).ok()?;
        if !before.file_type().is_file() || before.len() != expected_size {
            return None;
        }
        let sha = sha256_file_nofollow(abs_path).ok()?;
        let after = fs::symlink_metadata(abs_path).ok()?;
        let source_mode = file_mode(&after) & 0o777;
        if !after.file_type().is_file()
            || !same_file_snapshot(&before, &after)
            || sha != expected_sha
            || (!has_mode_override && source_mode != expected_mode)
        {
            return None;
        }
        Some(LocalStat {
            mtime_ms: metadata_mtime_ms(&after),
            size: expected_size,
            sha,
            mode: expected_mode,
        })
    }

    fn remember_large_local_stat(
        &self,
        rel_path: &str,
        abs_path: &Path,
        sha: &str,
        size: u64,
        mode: u32,
    ) {
        let mut cache = self
            .local_stat
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match fs::symlink_metadata(abs_path) {
            Ok(metadata)
                if metadata.file_type().is_file()
                    && metadata.len() == size
                    && (file_mode(&metadata) & 0o777) == mode =>
            {
                cache.insert(
                    rel_path.to_string(),
                    LocalStat {
                        mtime_ms: metadata_mtime_ms(&metadata),
                        size,
                        sha: sha.to_string(),
                        mode,
                    },
                );
            }
            _ => {
                cache.remove(rel_path);
            }
        }
    }

    fn has_disk_space_for_large_object(&self, path: &Path, object_bytes: u64) -> bool {
        let parent = path.parent().unwrap_or(path);
        match read_free_disk_bytes(parent) {
            Some(free_bytes) => {
                free_bytes >= object_bytes.saturating_mul(LARGE_OBJECT_FREE_SPACE_FACTOR)
            }
            None => true,
        }
    }

    fn discard_temp(&self, temp_path: &Path, label: &str) {
        match fs::remove_file(temp_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                self.log_message(&format!("temp cleanup failed {label}: {error}"));
            }
        }
    }
}

#[derive(Debug)]
struct CopiedFile {
    sha: String,
    size: u64,
    mode: u32,
}

fn copy_file_hashing(source_path: &Path, temp_path: &Path) -> Result<CopiedFile, io::Error> {
    let mut source = open_source_nofollow(source_path)?;
    let source_metadata = source.metadata()?;
    if !source_metadata.file_type().is_file() {
        return Err(io::Error::other("snapshot source is not a regular file"));
    }

    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 128 * 1024];

    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    destination.flush()?;

    Ok(CopiedFile {
        sha: format!("{:x}", digest.finalize()),
        size,
        mode: file_mode(&source_metadata) & 0o777,
    })
}

fn open_source_nofollow(path: &Path) -> Result<File, io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn sha256_file_nofollow(path: &Path) -> Result<String, io::Error> {
    let mut file = open_source_nofollow(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::other("snapshot source is not a regular file"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn read_free_disk_bytes(path: &Path) -> Option<u64> {
    let mut current = path.to_path_buf();
    loop {
        if let Ok(bytes) = fs2::available_space(&current) {
            return Some(bytes);
        }
        let parent = current.parent()?.to_path_buf();
        if parent == current {
            return None;
        }
        current = parent;
    }
}

pub fn tally_file_outcome(summary: &mut TransferSummary, outcome: FileOutcome, size: u64) {
    match outcome {
        FileOutcome::Uploaded => {
            summary.files_uploaded += 1;
            summary.bytes_uploaded = summary.bytes_uploaded.saturating_add(size);
        }
        FileOutcome::Unchanged => summary.skipped_unchanged += 1,
        FileOutcome::Oversize => summary.oversize += 1,
        FileOutcome::MetadataError => {
            summary.failures += 1;
            summary.metadata_failures += 1;
        }
        FileOutcome::Error => summary.failures += 1,
    }
}

pub fn tally_metadata_failure(summary: &mut TransferSummary) {
    summary.failures += 1;
    summary.metadata_failures += 1;
}

pub fn classify_file_failure(
    existing: Option<&BoxStoreManifestEntry>,
    mode: u32,
    fallback: FileOutcome,
) -> FileOutcome {
    match existing {
        Some(BoxStoreManifestEntry::Symlink { .. }) => FileOutcome::MetadataError,
        Some(BoxStoreManifestEntry::File {
            mode: existing_mode,
            ..
        }) if *existing_mode != mode => FileOutcome::MetadataError,
        _ => fallback,
    }
}

fn tally_category_outcome(
    summary: &mut CategoryTransferSummary,
    outcome: FileOutcome,
    size: u64,
) {
    match outcome {
        FileOutcome::Uploaded => {
            summary.files_uploaded = summary.files_uploaded.saturating_add(1);
            summary.bytes_uploaded = summary.bytes_uploaded.saturating_add(size);
        }
        FileOutcome::Unchanged => {
            summary.skipped_unchanged = summary.skipped_unchanged.saturating_add(1);
        }
        FileOutcome::Oversize => {
            summary.oversize = summary.oversize.saturating_add(1);
        }
        FileOutcome::MetadataError => tally_category_metadata_failure(summary),
        FileOutcome::Error => summary.failures = summary.failures.saturating_add(1),
    }
}

fn tally_category_metadata_failure(summary: &mut CategoryTransferSummary) {
    summary.failures = summary.failures.saturating_add(1);
    summary.metadata_failures = summary.metadata_failures.saturating_add(1);
}

fn file_entry(sha: String, size: u64, mode: u32, manifest_v2: bool) -> BoxStoreManifestEntry {
    if manifest_v2 {
        BoxStoreManifestEntry::File { sha, size, mode }
    } else {
        BoxStoreManifestEntry::LegacyFile { sha, size }
    }
}

fn entry_sha_size(entry: Option<&BoxStoreManifestEntry>) -> Option<(&str, u64)> {
    match entry? {
        BoxStoreManifestEntry::LegacyFile { sha, size }
        | BoxStoreManifestEntry::File {
            sha,
            size,
            mode: _,
        } => Some((sha.as_str(), *size)),
        BoxStoreManifestEntry::Symlink { .. } => None,
    }
}

fn file_stat(metadata: &fs::Metadata) -> FileStat {
    FileStat {
        mtime_ms: metadata_mtime_ms(metadata),
        size: metadata.len(),
        mode: file_mode(metadata) & 0o777,
    }
}

fn metadata_mtime_ms(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
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

fn is_snapshot_node_race(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
        || error.raw_os_error().is_some_and(|errno| errno == libc::ELOOP)
}

#[cfg(unix)]
fn same_file_snapshot(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.file_type().is_file()
        && after.file_type().is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.size() == after.size()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
        && before.mode() == after.mode()
}

#[cfg(not(unix))]
fn same_file_snapshot(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.file_type().is_file()
        && after.file_type().is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && file_mode(before) == file_mode(after)
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

fn is_excluded(rel_path: &str, excludes: &[&str]) -> bool {
    excludes
        .iter()
        .any(|pattern| glob_matches_path(pattern, rel_path))
}

pub fn dedupe_nested_roots<I, P>(roots: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut unique = Vec::<PathBuf>::new();
    for root in roots {
        let root = root.as_ref().to_path_buf();
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
        .iter()
        .filter(|root| {
            !unique
                .iter()
                .any(|other| other != *root && root.starts_with(other))
        })
        .cloned()
        .collect()
}

pub fn glob_matches_path(glob: &str, path: &str) -> bool {
    let glob = glob.trim_start_matches('/');
    let path = path.trim_start_matches('/');
    let pattern = glob.as_bytes();
    let text = path.as_bytes();
    if glob_match(pattern, text) {
        return true;
    }
    for (index, byte) in text.iter().enumerate() {
        if *byte == b'/' && glob_match(pattern, &text[..index]) {
            return true;
        }
    }
    false
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    if pattern.starts_with(b"**") {
        let rest = &pattern[2..];
        return (0..=text.len()).any(|index| glob_match(rest, &text[index..]));
    }
    if pattern[0] == b'*' {
        let rest = &pattern[1..];
        return (0..=text.len())
            .take_while(|index| *index == 0 || text[*index - 1] != b'/')
            .any(|index| glob_match(rest, &text[index..]));
    }
    if text.is_empty() || pattern[0] != text[0] {
        return false;
    }
    glob_match(&pattern[1..], &text[1..])
}
