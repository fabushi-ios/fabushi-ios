//! OCRM File Memory Store Submodule (OCRM-MODULAR-01)
//!
//! Decomposed from `memory_service.rs` to satisfy single responsibility
//! and cohesive file size budget. Implements `FileMemoryStore` with filesystem-backed
//! profile, log, explicit, synthesized, and tombstone fact management.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

use crate::watched_directory::{ChangeListener, WatchedDirectory};
use super::memory_service::*;

#[derive(Debug, Clone)]
struct StoredMemoryFact {
    record: MemoryRecord,
    path: PathBuf,
    line: usize,
    order: usize,
}

#[derive(Debug, Clone)]
struct SynthesisStoredFact {
    record: MemoryRecord,
    path: PathBuf,
    line: usize,
    order: usize,
    origin: MemoryOrigin,
}

type SynthesisEnabledProbe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct FileMemoryStore {
    dir: WatchedDirectory,
    profile_file: PathBuf,
    log_dir: PathBuf,
    explicit_dir: PathBuf,
    synthesized_dir: PathBuf,
    tombstone_dir: PathBuf,
    refresh_file: PathBuf,
    synthesis_enabled: SynthesisEnabledProbe,
}

impl fmt::Debug for FileMemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileMemoryStore")
            .field("location", &self.dir.get_location())
            .field("synthesis_enabled", &self.synthesis_metadata_enabled())
            .finish()
    }
}

impl FileMemoryStore {
    pub fn new(memory_dir: impl Into<PathBuf>) -> Self {
        Self::with_synthesis_metadata(memory_dir, false)
    }

    pub fn with_synthesis_metadata(
        memory_dir: impl Into<PathBuf>,
        synthesis_metadata_enabled: bool,
    ) -> Self {
        Self::with_synthesis_probe(
            memory_dir,
            Arc::new(move || synthesis_metadata_enabled),
        )
    }

    pub fn with_synthesis_probe(
        memory_dir: impl Into<PathBuf>,
        synthesis_enabled: SynthesisEnabledProbe,
    ) -> Self {
        let memory_dir = memory_dir.into();
        let metadata = memory_dir.join(METADATA_DIRNAME);
        Self {
            profile_file: memory_dir.join(PROFILE_FILENAME),
            log_dir: memory_dir.join(LOG_DIRNAME),
            explicit_dir: metadata.join(EXPLICIT_DIRNAME),
            synthesized_dir: metadata.join(SYNTHESIZED_DIRNAME),
            tombstone_dir: metadata.join(TOMBSTONE_DIRNAME),
            refresh_file: metadata.join(REFRESH_FILENAME),
            synthesis_enabled,
            dir: WatchedDirectory::new(memory_dir, MEMORY_CHANGE_DEBOUNCE_MS),
        }
    }

    fn synthesis_metadata_enabled(&self) -> bool {
        (self.synthesis_enabled)()
    }

    pub fn get_location(&self) -> PathBuf {
        self.dir.get_location().to_path_buf()
    }

    pub fn set_on_change(&self, listener: Option<ChangeListener>) -> Result<(), String> {
        self.dir.set_on_change(listener)
    }

    pub fn recall(&self, recent_limit: usize) -> MemoryRecall {
        let mut facts = self.facts();
        facts.sort_by(|a, b| {
            b.record
                .created_at
                .cmp(&a.record.created_at)
                .then_with(|| b.order.cmp(&a.order))
        });
        MemoryRecall {
            profile: facts
                .iter()
                .filter(|fact| fact.record.kind == MemoryKind::Profile)
                .take(MEMORY_PROFILE_PROMPT_LIMIT)
                .map(|fact| fact.record.clone())
                .collect(),
            recent: facts
                .iter()
                .filter(|fact| fact.record.kind == MemoryKind::Log)
                .take(recent_limit)
                .map(|fact| fact.record.clone())
                .collect(),
        }
    }

    pub fn list_memories(&self, limit: usize) -> Vec<MemoryRecord> {
        let mut facts = self.facts();
        facts.sort_by(|a, b| {
            let a_profile = usize::from(a.record.kind == MemoryKind::Profile);
            let b_profile = usize::from(b.record.kind == MemoryKind::Profile);
            b_profile
                .cmp(&a_profile)
                .then_with(|| b.record.created_at.cmp(&a.record.created_at))
                .then_with(|| b.order.cmp(&a.order))
        });
        facts
            .into_iter()
            .take(limit)
            .map(|fact| fact.record)
            .collect()
    }

    pub fn snapshot_for_recovery(&self, records: &[MemoryRecord]) -> Vec<MemoryRecoveryRecord> {
        records
            .iter()
            .cloned()
            .map(|record| MemoryRecoveryRecord {
                origin: self.memory_origin(&record.content),
                tombstoned: self.is_tombstoned(&record.content),
                record,
            })
            .collect()
    }

    pub fn restore_records_exact(&self, records: &[MemoryRecoveryRecord]) -> io::Result<()> {
        for recovery in records {
            let _ = self.add_memory(
                &recovery.record.content,
                recovery.record.created_at,
                recovery.record.kind,
            )?;
            if self.synthesis_metadata_enabled() {
                self.clear_origins(&recovery.record.content)?;
                self.mark_origin(&recovery.record.content, recovery.origin)?;
                if recovery.tombstoned {
                    self.mark_tombstone(&recovery.record.content)?;
                } else {
                    self.clear_tombstone(&recovery.record.content)?;
                }
            }
        }
        Ok(())
    }

    pub fn count_memories(&self) -> usize {
        self.facts().len()
    }

    pub fn has_memories(&self) -> bool {
        self.count_memories() > 0
    }

    pub fn add_memory(
        &self,
        content: &str,
        created_at: i64,
        kind: MemoryKind,
    ) -> io::Result<Option<MemoryRecord>> {
        let normalized = normalize_memory_content(content);
        if normalized.is_empty() {
            return Ok(None);
        }
        let key = memory_dedupe_key(&normalized);
        if let Some(existing) = self
            .facts()
            .into_iter()
            .find(|fact| memory_dedupe_key(&fact.record.content) == key)
        {
            if self.synthesis_metadata_enabled() {
                self.clear_tombstone(&existing.record.content)?;
                self.clear_origins(&existing.record.content)?;
                self.mark_origin(&existing.record.content, MemoryOrigin::Explicit)?;
            }
            return Ok(None);
        }

        let path = match kind {
            MemoryKind::Profile => self.profile_file.clone(),
            MemoryKind::Log => self.log_dir.join(format!(
                "{}.md",
                format_memory_date(created_at)
                    .chars()
                    .take(7)
                    .collect::<String>()
            )),
        };
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let header = match kind {
            MemoryKind::Profile => PROFILE_HEADER,
            MemoryKind::Log => LOG_HEADER,
        };
        let mut next = if raw.is_empty() {
            header.to_string()
        } else {
            raw
        };
        if !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&serialize_fact_line(&normalized, created_at));
        next.push('\n');
        self.dir.write_file_atomic(&path, next.as_bytes())?;
        if self.synthesis_metadata_enabled() {
            self.clear_tombstone(&normalized)?;
            self.clear_origins(&normalized)?;
            self.mark_origin(&normalized, MemoryOrigin::Explicit)?;
        }

        Ok(Some(MemoryRecord {
            id: memory_id_for(&normalized),
            content: normalized,
            created_at,
            kind,
        }))
    }

    pub fn remove_memory_by_content(&self, content: &str) -> io::Result<bool> {
        let normalized = normalize_memory_content(content);
        if normalized.is_empty() {
            return Ok(false);
        }
        self.remove_memory(&memory_id_for(&normalized))
    }

    pub fn remove_memory(&self, id: &str) -> io::Result<bool> {
        let Some(fact) = self.facts().into_iter().find(|fact| fact.record.id == id) else {
            return Ok(false);
        };
        let raw = fs::read_to_string(&fact.path).unwrap_or_default();
        let mut lines = raw.split('\n').map(ToOwned::to_owned).collect::<Vec<_>>();
        if fact.line >= lines.len() {
            return Ok(false);
        }
        lines.remove(fact.line);
        self.dir
            .write_file_atomic(&fact.path, lines.join("\n").as_bytes())?;
        if self.synthesis_metadata_enabled() {
            self.clear_origins(&fact.record.content)?;
            self.mark_tombstone(&fact.record.content)?;
        }
        Ok(true)
    }

    pub fn clear_memories(&self) -> io::Result<()> {
        let facts = self.facts();
        if facts.is_empty() {
            return Ok(());
        }
        if self.synthesis_metadata_enabled() {
            for fact in &facts {
                self.clear_origins(&fact.record.content)?;
                self.mark_tombstone(&fact.record.content)?;
            }
        }
        match fs::remove_dir_all(&self.log_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.dir
            .write_file_atomic(&self.profile_file, PROFILE_HEADER.as_bytes())
    }

    pub fn prepare_synthesis(&self) -> SynthesisSnapshot {
        let (fingerprint, mut facts) = self.read_synthesis_state();
        facts.sort_by(|left, right| {
            usize::from(right.origin == MemoryOrigin::Explicit)
                .cmp(&usize::from(left.origin == MemoryOrigin::Explicit))
                .then_with(|| {
                    usize::from(right.record.kind == MemoryKind::Profile)
                        .cmp(&usize::from(left.record.kind == MemoryKind::Profile))
                })
                .then_with(|| right.record.created_at.cmp(&left.record.created_at))
                .then_with(|| right.order.cmp(&left.order))
        });
        let mut seen = HashSet::new();
        let memories = facts
            .into_iter()
            .filter(|fact| seen.insert(fact.record.id.clone()))
            .take(MEMORY_SYNTHESIS_INPUT_LIMIT)
            .map(|fact| SynthesisMemory {
                id: fact.record.id,
                content: fact.record.content,
                created_at: fact.record.created_at,
                kind: fact.record.kind,
                origin: fact.origin,
            })
            .collect();
        SynthesisSnapshot {
            fingerprint,
            memories,
        }
    }

    pub fn apply_synthesis(
        &self,
        snapshot: &SynthesisSnapshot,
        changes: &[SynthesisChange],
        now_ms: i64,
    ) -> io::Result<SynthesisApplyResult> {
        let (fingerprint, facts) = self.read_synthesis_state();
        if fingerprint != snapshot.fingerprint {
            return Ok(SynthesisApplyResult::Stale);
        }
        let allowed = snapshot
            .memories
            .iter()
            .map(|memory| memory.id.clone())
            .collect::<HashSet<_>>();
        let by_id = facts
            .into_iter()
            .map(|fact| (fact.record.id.clone(), fact))
            .collect::<HashMap<_, _>>();
        let mut changed = HashSet::new();
        for change in changes {
            match change {
                SynthesisChange::Create { content, kind } => {
                    let content = normalize_memory_content(content);
                    if content.is_empty() {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    if !self.is_tombstoned(&content) {
                        self.add_synthesized(&content, now_ms, *kind)?;
                    }
                }
                SynthesisChange::Update { id, content, kind } => {
                    if !allowed.contains(id) || !changed.insert(id.clone()) {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    let Some(current) = by_id.get(id) else {
                        return Ok(SynthesisApplyResult::Invalid);
                    };
                    if current.origin == MemoryOrigin::Explicit {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    let content = normalize_memory_content(content);
                    if content.is_empty() {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    self.remove_fact_for_synthesis(current)?;
                    if !self.is_tombstoned(&content) {
                        self.add_synthesized(&content, now_ms, *kind)?;
                    }
                }
                SynthesisChange::Remove { id } => {
                    if !allowed.contains(id) || !changed.insert(id.clone()) {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    let Some(current) = by_id.get(id) else {
                        return Ok(SynthesisApplyResult::Invalid);
                    };
                    if current.origin == MemoryOrigin::Explicit {
                        return Ok(SynthesisApplyResult::Invalid);
                    }
                    self.remove_fact_for_synthesis(current)?;
                }
            }
        }
        self.mark_temporal_review(now_ms)?;
        Ok(SynthesisApplyResult::Committed)
    }

    pub fn is_temporal_review_due(&self, now_ms: i64) -> bool {
        let next = fs::read_to_string(&self.refresh_file)
            .ok()
            .and_then(|raw| raw.trim().parse::<i64>().ok());
        next.is_none_or(|value| value <= now_ms)
    }

    pub fn mark_temporal_review(&self, now_ms: i64) -> io::Result<()> {
        let next = now_ms.saturating_add(MEMORY_SYNTHESIS_REFRESH_INTERVAL_MS);
        self.dir.write_file_atomic(
            &self.refresh_file,
            format!(
                "{next}
"
            )
            .as_bytes(),
        )
    }

    fn read_synthesis_state(&self) -> (String, Vec<SynthesisStoredFact>) {
        let mut hasher = Sha256::new();
        let mut output = Vec::new();
        let mut files = vec![(self.profile_file.clone(), MemoryKind::Profile)];
        let mut logs = match fs::read_dir(&self.log_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        logs.sort();
        files.extend(logs.into_iter().map(|path| (path, MemoryKind::Log)));
        for (path, kind) in files {
            let raw = fs::read_to_string(&path).unwrap_or_default();
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update([0]);
            hasher.update(raw.as_bytes());
            hasher.update([0]);
            for (line, text) in raw.split('\n').enumerate() {
                let Some(fact) = parse_fact_line(text, kind) else {
                    continue;
                };
                let Some(created_at) = parse_memory_date_ms(&fact.date) else {
                    continue;
                };
                let order = output.len();
                let record = MemoryRecord {
                    id: memory_id_for(&fact.content),
                    content: fact.content,
                    created_at,
                    kind,
                };
                let origin = self.memory_origin(&record.content);
                output.push(SynthesisStoredFact {
                    record,
                    path: path.clone(),
                    line,
                    order,
                    origin,
                });
            }
        }
        let fingerprint = format!("{:x}", hasher.finalize());
        (fingerprint, output)
    }

    fn metadata_path(&self, content: &str, origin: MemoryOrigin) -> PathBuf {
        let dir = match origin {
            MemoryOrigin::Explicit => &self.explicit_dir,
            MemoryOrigin::Synthesis | MemoryOrigin::Legacy => &self.synthesized_dir,
        };
        dir.join(format!("{}.memory", memory_id_for(content)))
    }

    fn tombstone_path(&self, content: &str) -> PathBuf {
        self.tombstone_dir
            .join(format!("{}.deleted", memory_id_for(content)))
    }

    fn memory_origin(&self, content: &str) -> MemoryOrigin {
        if self
            .metadata_path(content, MemoryOrigin::Explicit)
            .is_file()
        {
            MemoryOrigin::Explicit
        } else if self
            .metadata_path(content, MemoryOrigin::Synthesis)
            .is_file()
        {
            MemoryOrigin::Synthesis
        } else {
            MemoryOrigin::Legacy
        }
    }

    fn mark_origin(&self, content: &str, origin: MemoryOrigin) -> io::Result<()> {
        if origin == MemoryOrigin::Legacy {
            return Ok(());
        }
        self.dir
            .write_file_atomic(&self.metadata_path(content, origin), b"")
    }

    fn clear_origins(&self, content: &str) -> io::Result<()> {
        for origin in [MemoryOrigin::Explicit, MemoryOrigin::Synthesis] {
            match fs::remove_file(self.metadata_path(content, origin)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn is_tombstoned(&self, content: &str) -> bool {
        self.tombstone_path(content).is_file()
    }

    fn mark_tombstone(&self, content: &str) -> io::Result<()> {
        self.dir
            .write_file_atomic(&self.tombstone_path(content), b"")
    }

    fn clear_tombstone(&self, content: &str) -> io::Result<()> {
        match fs::remove_file(self.tombstone_path(content)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn remove_fact_for_synthesis(&self, fact: &SynthesisStoredFact) -> io::Result<()> {
        let raw = fs::read_to_string(&fact.path).unwrap_or_default();
        let mut lines = raw.split('\n').map(ToOwned::to_owned).collect::<Vec<_>>();
        if fact.line >= lines.len() {
            return Ok(());
        }
        lines.remove(fact.line);
        self.dir
            .write_file_atomic(&fact.path, lines.join("\n").as_bytes())?;
        self.clear_origins(&fact.record.content)
    }

    fn add_synthesized(&self, content: &str, created_at: i64, kind: MemoryKind) -> io::Result<()> {
        if self
            .facts()
            .iter()
            .any(|fact| memory_dedupe_key(&fact.record.content) == memory_dedupe_key(content))
        {
            return Ok(());
        }
        let path = match kind {
            MemoryKind::Profile => self.profile_file.clone(),
            MemoryKind::Log => self.log_dir.join(format!(
                "{}.md",
                format_memory_date(created_at)
                    .chars()
                    .take(7)
                    .collect::<String>()
            )),
        };
        let raw = fs::read_to_string(&path).unwrap_or_default();
        let header = match kind {
            MemoryKind::Profile => PROFILE_HEADER,
            MemoryKind::Log => LOG_HEADER,
        };
        let mut next = if raw.is_empty() {
            header.to_string()
        } else {
            raw
        };
        if !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(&serialize_fact_line(content, created_at));
        next.push('\n');
        self.dir.write_file_atomic(&path, next.as_bytes())?;
        self.clear_origins(content)?;
        self.mark_origin(content, MemoryOrigin::Synthesis)
    }

    fn facts(&self) -> Vec<StoredMemoryFact> {
        let mut facts = Vec::new();
        append_facts_from_file(&mut facts, &self.profile_file, MemoryKind::Profile);

        let mut log_paths = match fs::read_dir(&self.log_dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|kind| kind.is_file())
                        .unwrap_or(false)
                })
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        log_paths.sort();
        for path in log_paths {
            append_facts_from_file(&mut facts, &path, MemoryKind::Log);
        }
        facts
    }
}


pub(crate) fn append_facts_from_file(output: &mut Vec<StoredMemoryFact>, path: &Path, kind: MemoryKind) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    for (line, text) in raw.split('\n').enumerate() {
        let Some(fact) = parse_fact_line(text, kind) else {
            continue;
        };
        let Some(created_at) = parse_memory_date_ms(&fact.date) else {
            continue;
        };
        let order = output.len();
        output.push(StoredMemoryFact {
            record: MemoryRecord {
                id: memory_id_for(&fact.content),
                content: fact.content,
                created_at,
                kind,
            },
            path: path.to_path_buf(),
            line,
            order,
        });
    }
}

pub(crate) fn read_valid_facts(path: &Path, kind: MemoryKind) -> Option<Vec<MemoryFact>> {
    let raw = fs::read_to_string(path).ok()?;
    Some(parse_facts(&raw, kind))
}

pub(crate) fn parse_fact_line(line: &str, kind: MemoryKind) -> Option<MemoryFact> {
    let line = line.trim();
    let rest = line.strip_prefix(FACT_PREFIX)?;
    let (date, content) = rest.split_once(") ")?;
    if !valid_memory_date(date) {
        return None;
    }
    let content = normalize_memory_content(content);
    if content.is_empty() {
        return None;
    }
    Some(MemoryFact {
        date: date.to_string(),
        content,
        kind,
    })
}

pub(crate) fn parse_memory_date_ms(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    date.and_hms_opt(0, 0, 0)?
        .and_utc()
        .timestamp_millis()
        .into()
}

pub(crate) fn valid_memory_date(value: &str) -> bool {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}
