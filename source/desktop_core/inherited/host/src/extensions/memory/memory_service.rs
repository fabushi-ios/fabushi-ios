use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use chrono::{DateTime, NaiveDate, Utc};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use serde::{Deserialize, Serialize};

use super::project_membership::AgentProjectMembership;
use crate::watched_directory::{ChangeListener, WatchedDirectory};

pub const MEMORY_DIRNAME: &str = "memory";
pub const PROFILE_FILENAME: &str = "profile.md";
pub const LOG_DIRNAME: &str = "log";
pub const MEMORY_CHANGE_DEBOUNCE_MS: u64 = 50;
pub const MEMORY_PROFILE_PROMPT_LIMIT: usize = 100;
pub const MEMORY_MAX_CONTENT_LENGTH: usize = 500;
pub const MEMORY_SYNTHESIS_INPUT_LIMIT: usize = 512;
pub const MEMORY_SYNTHESIS_REFRESH_INTERVAL_MS: i64 = 86_400_000;

pub(crate) const METADATA_DIRNAME: &str = ".dreaming";
pub(crate) const EXPLICIT_DIRNAME: &str = "explicit";
pub(crate) const SYNTHESIZED_DIRNAME: &str = "synthesized";
pub(crate) const TOMBSTONE_DIRNAME: &str = "tombstones";
pub(crate) const REFRESH_FILENAME: &str = "next-refresh-at";

pub(crate) const FACT_PREFIX: &str = "- (";
pub(crate) const PROFILE_HEADER: &str =
    "# About the user\n\n<!-- Enduring facts, one per line as \"- (YYYY-MM-DD) <fact>\". -->\n\n";
pub(crate) const LOG_HEADER: &str =
    "# Memory log\n\n<!-- Dated facts, one per line as \"- (YYYY-MM-DD) <fact>\". -->\n\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryKind {
    Profile,
    Log,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFact {
    pub date: String,
    pub content: String,
    pub kind: MemoryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    pub kind: MemoryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryOrigin {
    Explicit,
    Synthesis,
    Legacy,
}

/// Exact recovery input for a cross-store memory mutation. The record is the
/// durable fact while origin/tombstone preserve synthesis metadata that is
/// otherwise changed by the ordinary explicit-memory APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecoveryRecord {
    pub record: MemoryRecord,
    pub origin: MemoryOrigin,
    pub tombstoned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisMemory {
    pub id: String,
    pub content: String,
    pub created_at: i64,
    pub kind: MemoryKind,
    pub origin: MemoryOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SynthesisChange {
    Create {
        content: String,
        kind: MemoryKind,
    },
    Update {
        id: String,
        content: String,
        kind: MemoryKind,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesisSnapshot {
    pub fingerprint: String,
    pub memories: Vec<SynthesisMemory>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthesisApplyResult {
    Committed,
    Stale,
    Invalid,
}

pub type SynthesisApplyGate = Arc<
    dyn Fn(
            &str,
            &FileMemoryStore,
            &SynthesisSnapshot,
            &[SynthesisChange],
            i64,
        ) -> SynthesisApplyResult
        + Send
        + Sync,
>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryRecall {
    pub profile: Vec<MemoryRecord>,
    pub recent: Vec<MemoryRecord>,
}

pub use super::memory_service_ocrm::*;

pub trait MemorySynthesisBridge: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn record_turn(
        &self,
        agent_id: &str,
        evidence_id: Option<String>,
        user: &str,
        assistant: &str,
        occurred_at: i64,
    );
}

pub type MemoryChangeListener = Arc<dyn Fn() + Send + Sync>;
pub type MemoryUnsubscribe = Box<dyn FnOnce() + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedMemoryRecord {
    pub agent_id: String,
    pub agent_name: String,
    pub memory: MemoryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserMemoryRecall {
    pub profile: Vec<ScopedMemoryRecord>,
    pub recent: Vec<ScopedMemoryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryRecord {
    pub project: String,
    pub agent_id: String,
    pub agent_name: String,
    pub memory: MemoryRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryBlock {
    pub slug: String,
    pub name: String,
    pub own_shard_dir: PathBuf,
    pub recall: UserMemoryRecall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMemoryMembership {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectMemoryPromptRecall {
    pub injected: Vec<ProjectMemoryBlock>,
    pub also_member_of: Vec<ProjectMemoryMembership>,
}

fn shared_memory_importance(content: &str) -> f64 {
    if content.starts_with("[episode] ") {
        1.5
    } else if content.starts_with("[note] ") {
        0.5
    } else {
        1.0
    }
}

fn shared_memory_recall_rank(memory: &MemoryRecord) -> f64 {
    const DAY_MS: f64 = 86_400_000.0;
    const HALF_LIFE_DAYS: f64 = 30.0;
    shared_memory_importance(&memory.content).log2()
        + memory.created_at as f64 / (HALF_LIFE_DAYS * DAY_MS)
}

fn merge_scoped_memory_records(
    records: Vec<ScopedMemoryRecord>,
    limit: usize,
    rank_by_recall: bool,
) -> Vec<ScopedMemoryRecord> {
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<ScopedMemoryRecord> = Vec::new();

    for record in records {
        let key = memory_dedupe_key(&record.memory.content);
        if let Some(index) = by_key.get(&key).copied() {
            if record.memory.created_at > merged[index].memory.created_at {
                merged[index] = record;
            }
        } else {
            by_key.insert(key, merged.len());
            merged.push(record);
        }
    }

    if rank_by_recall {
        merged.sort_by(|left, right| {
            shared_memory_recall_rank(&right.memory)
                .partial_cmp(&shared_memory_recall_rank(&left.memory))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.memory.created_at.cmp(&left.memory.created_at))
        });
    } else {
        merged.sort_by(|left, right| {
            right
                .memory
                .created_at
                .cmp(&left.memory.created_at)
                .then_with(|| left.memory.content.cmp(&right.memory.content))
        });
    }
    merged.truncate(limit);
    merged
}

/// Merges semantic/BM25/FTS hybrid memory records with deduplication and ranking.
pub fn merge_hybrid_results(
    records: Vec<ScopedMemoryRecord>,
    limit: usize,
    rank_by_recall: bool,
) -> Vec<ScopedMemoryRecord> {
    merge_scoped_memory_records(records, limit, rank_by_recall)
}

#[derive(Clone)]
pub struct UserMemoryStore {
    sand_root: PathBuf,
    own_agent_id: String,
    resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl fmt::Debug for UserMemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserMemoryStore")
            .field("sand_root", &self.sand_root)
            .field("own_agent_id", &self.own_agent_id)
            .finish()
    }
}

impl UserMemoryStore {
    pub fn new(
        sand_root: impl Into<PathBuf>,
        own_agent_id: impl Into<String>,
        resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> Self {
        Self {
            sand_root: sand_root.into(),
            own_agent_id: own_agent_id.into(),
            resolve_agent_name,
        }
    }

    pub fn get_location(&self) -> PathBuf {
        get_user_memory_dir(&self.sand_root)
    }

    pub fn get_own_shard_location(&self) -> PathBuf {
        get_user_memory_shard_dir(&self.sand_root, &self.own_agent_id)
    }

    pub fn recall(&self, profile_limit: usize, recent_limit: usize) -> UserMemoryRecall {
        let mut profile = Vec::new();
        let mut recent = Vec::new();
        let mut agent_ids = read_child_directory_names(&get_user_memory_shards_dir(&self.sand_root));
        agent_ids.sort();
        for agent_id in agent_ids {
            let store = FileMemoryStore::new(get_user_memory_shard_dir(&self.sand_root, &agent_id));
            let recalled = store.recall(usize::MAX);
            let agent_name = (self.resolve_agent_name)(&agent_id);
            profile.extend(recalled.profile.into_iter().map(|memory| ScopedMemoryRecord {
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                memory,
            }));
            recent.extend(recalled.recent.into_iter().map(|memory| ScopedMemoryRecord {
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                memory,
            }));
        }
        UserMemoryRecall {
            profile: merge_scoped_memory_records(profile, profile_limit, false),
            recent: merge_scoped_memory_records(recent, recent_limit, true),
        }
    }
}

#[derive(Clone)]
pub struct ProjectMemoryStore {
    sand_root: PathBuf,
    own_agent_id: String,
    membership: AgentProjectMembership,
    resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
}

impl fmt::Debug for ProjectMemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectMemoryStore")
            .field("sand_root", &self.sand_root)
            .field("own_agent_id", &self.own_agent_id)
            .field("membership", &self.membership)
            .finish()
    }
}

impl ProjectMemoryStore {
    pub fn new(
        sand_root: impl Into<PathBuf>,
        own_agent_id: impl Into<String>,
        membership: AgentProjectMembership,
        resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> Self {
        Self {
            sand_root: sand_root.into(),
            own_agent_id: own_agent_id.into(),
            membership,
            resolve_agent_name,
        }
    }

    pub fn get_location(&self) -> PathBuf {
        get_projects_root_dir(&self.sand_root)
    }

    pub fn recall(
        &self,
        _profile_limit: usize,
        recent_limit: usize,
        cap: usize,
    ) -> Vec<ProjectMemoryRecord> {
        let mut output = Vec::new();
        for project in self.membership.read() {
            let mut agent_ids =
                read_child_directory_names(&get_project_memory_shards_dir(&self.sand_root, &project));
            agent_ids.sort();
            for agent_id in agent_ids {
                let store = FileMemoryStore::new(get_project_memory_shard_dir(
                    &self.sand_root,
                    &project,
                    &agent_id,
                ));
                let recalled = store.recall(recent_limit);
                let agent_name = (self.resolve_agent_name)(&agent_id);
                for memory in recalled
                    .profile
                    .into_iter()
                    .chain(recalled.recent.into_iter())
                {
                    output.push(ProjectMemoryRecord {
                        project: project.clone(),
                        agent_id: agent_id.clone(),
                        agent_name: agent_name.clone(),
                        memory,
                    });
                }
            }
        }
        output.sort_by(|left, right| right.memory.created_at.cmp(&left.memory.created_at));
        output.truncate(cap);
        output
    }

    pub fn recall_for_prompt(
        &self,
        profile_limit: usize,
        recent_limit: usize,
        injected_cap: usize,
    ) -> ProjectMemoryPromptRecall {
        let mut blocks = Vec::new();
        for slug in self.membership.read() {
            let mut profile = Vec::new();
            let mut recent = Vec::new();
            let mut agent_ids =
                read_child_directory_names(&get_project_memory_shards_dir(&self.sand_root, &slug));
            agent_ids.sort();
            for agent_id in agent_ids {
                let store = FileMemoryStore::new(get_project_memory_shard_dir(
                    &self.sand_root,
                    &slug,
                    &agent_id,
                ));
                let recalled = store.recall(usize::MAX);
                let agent_name = (self.resolve_agent_name)(&agent_id);
                profile.extend(recalled.profile.into_iter().map(|memory| ScopedMemoryRecord {
                    agent_id: agent_id.clone(),
                    agent_name: agent_name.clone(),
                    memory,
                }));
                recent.extend(recalled.recent.into_iter().map(|memory| ScopedMemoryRecord {
                    agent_id: agent_id.clone(),
                    agent_name: agent_name.clone(),
                    memory,
                }));
            }
            let profile = merge_scoped_memory_records(profile, profile_limit, false);
            let recent = merge_scoped_memory_records(recent, recent_limit, true);
            blocks.push(ProjectMemoryBlock {
                name: read_project_name(&self.sand_root, &slug),
                own_shard_dir: get_project_memory_shard_dir(
                    &self.sand_root,
                    &slug,
                    &self.own_agent_id,
                ),
                slug,
                recall: UserMemoryRecall { profile, recent },
            });
        }
        blocks.sort_by(|left, right| {
            let left_has = !left.recall.profile.is_empty() || !left.recall.recent.is_empty();
            let right_has = !right.recall.profile.is_empty() || !right.recall.recent.is_empty();
            let newest = |block: &ProjectMemoryBlock| {
                block
                    .recall
                    .profile
                    .iter()
                    .chain(block.recall.recent.iter())
                    .map(|record| record.memory.created_at)
                    .max()
                    .unwrap_or(0)
            };
            right_has
                .cmp(&left_has)
                .then_with(|| newest(right).cmp(&newest(left)))
                .then_with(|| left.slug.cmp(&right.slug))
        });
        let injected_count = injected_cap.min(blocks.len());
        let also_member_of = blocks[injected_count..]
            .iter()
            .map(|block| ProjectMemoryMembership {
                slug: block.slug.clone(),
                name: block.name.clone(),
            })
            .collect();
        let injected = blocks.into_iter().take(injected_count).collect();
        ProjectMemoryPromptRecall {
            injected,
            also_member_of,
        }
    }
}

fn read_project_name(sand_root: &Path, slug: &str) -> String {
    let raw = fs::read_to_string(get_project_dir(sand_root, slug).join("project.md"))
        .unwrap_or_default();
    let mut in_frontmatter = false;
    for line in raw.lines() {
        let line = line.trim();
        if line == "---" {
            if in_frontmatter {
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if !in_frontmatter {
            continue;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    slug.to_string()
}

#[derive(Clone)]
pub struct MemoryService {
    sand_root: PathBuf,
    agents_root_dir: PathBuf,
    synthesis_bridge: Arc<Mutex<Option<Weak<dyn MemorySynthesisBridge>>>>,
    synthesis_apply_gate: Arc<Mutex<Option<SynthesisApplyGate>>>,
    active_agent_id: Arc<Mutex<Option<String>>>,
    listeners: Arc<Mutex<HashMap<u64, MemoryChangeListener>>>,
    next_listener_id: Arc<AtomicU64>,
}

impl fmt::Debug for MemoryService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryService")
            .field("sand_root", &self.sand_root)
            .field("agents_root_dir", &self.agents_root_dir)
            .field("active_agent_id", &self.active_agent_id())
            .field("synthesis_enabled", &self.synthesis_enabled())
            .finish()
    }
}

impl MemoryService {
    pub fn new(agents_root_dir: impl Into<PathBuf>) -> Self {
        let agents_root_dir = agents_root_dir.into();
        let sand_root = if agents_root_dir.file_name().and_then(|value| value.to_str()) == Some("agents") {
            agents_root_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| agents_root_dir.clone())
        } else {
            agents_root_dir.clone()
        };
        Self::new_with_sand_root(sand_root, agents_root_dir)
    }

    pub fn new_with_sand_root(
        sand_root: impl Into<PathBuf>,
        agents_root_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            sand_root: sand_root.into(),
            agents_root_dir: agents_root_dir.into(),
            synthesis_bridge: Arc::new(Mutex::new(None)),
            synthesis_apply_gate: Arc::new(Mutex::new(None)),
            active_agent_id: Arc::new(Mutex::new(None)),
            listeners: Arc::new(Mutex::new(HashMap::new())),
            next_listener_id: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn create_agent_store(&self, agent_dir: impl AsRef<Path>) -> FileMemoryStore {
        let synthesis_bridge = Arc::downgrade(&self.synthesis_bridge);
        FileMemoryStore::with_synthesis_probe(
            get_agent_memory_dir(agent_dir),
            Arc::new(move || {
                let Some(synthesis_bridge) = synthesis_bridge.upgrade() else {
                    return false;
                };
                let bridge = synthesis_bridge
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .and_then(Weak::upgrade);
                bridge.is_some_and(|bridge| bridge.is_enabled())
            }),
        )
    }

    pub fn create_project_membership(&self, agent_dir: impl AsRef<Path>) -> AgentProjectMembership {
        AgentProjectMembership::new(agent_dir.as_ref())
    }

    pub fn project_membership_for_agent(&self, agent_id: &str) -> AgentProjectMembership {
        self.create_project_membership(self.agents_root_dir.join(agent_id))
    }

    pub fn store_for_agent(&self, agent_id: &str) -> FileMemoryStore {
        self.create_agent_store(self.agents_root_dir.join(agent_id))
    }

    pub fn set_synthesis_apply_gate(&self, gate: Option<SynthesisApplyGate>) {
        *self
            .synthesis_apply_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = gate;
    }

    pub fn apply_synthesis_with_gate(
        &self,
        agent_id: &str,
        store: &FileMemoryStore,
        snapshot: &SynthesisSnapshot,
        changes: &[SynthesisChange],
        now_ms: i64,
    ) -> SynthesisApplyResult {
        let gate = self
            .synthesis_apply_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        gate.map_or(SynthesisApplyResult::Invalid, |gate| {
            gate(agent_id, store, snapshot, changes, now_ms)
        })
    }

    pub fn user_memory_store(
        &self,
        own_agent_id: impl Into<String>,
        resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> UserMemoryStore {
        UserMemoryStore::new(&self.sand_root, own_agent_id, resolve_agent_name)
    }

    pub fn project_memory_store(
        &self,
        own_agent_id: impl Into<String>,
        membership: AgentProjectMembership,
        resolve_agent_name: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> ProjectMemoryStore {
        ProjectMemoryStore::new(
            &self.sand_root,
            own_agent_id,
            membership,
            resolve_agent_name,
        )
    }

    pub fn list(&self, agent_id: &str) -> Vec<MemoryRecord> {
        self.store_for_agent(agent_id).list_memories(100)
    }

    pub fn remove(&self, agent_id: &str, id: &str) -> io::Result<bool> {
        let removed = self.store_for_agent(agent_id).remove_memory(id)?;
        if removed {
            self.emit();
        }
        Ok(removed)
    }

    pub fn clear(&self, agent_id: &str) -> io::Result<()> {
        self.store_for_agent(agent_id).clear_memories()?;
        self.emit();
        Ok(())
    }

    pub fn active_agent_id(&self) -> Option<String> {
        self.active_agent_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_active_agent(&self, agent_id: Option<String>) {
        let changed = {
            let mut active = self
                .active_agent_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *active == agent_id {
                false
            } else {
                *active = agent_id;
                true
            }
        };
        if changed {
            self.emit();
        }
    }

    pub fn subscribe(&self, listener: MemoryChangeListener) -> MemoryUnsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(id, listener);
        let listeners = Arc::downgrade(&self.listeners);
        Box::new(move || {
            if let Some(listeners) = listeners.upgrade() {
                listeners
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
            }
        })
    }

    pub fn dispose(&self) {
        self.clear_synthesis_bridge();
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    fn emit(&self) {
        let listeners = self
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for listener in listeners {
            listener();
        }
    }

    pub fn set_synthesis_bridge(&self, bridge: Weak<dyn MemorySynthesisBridge>) {
        *self
            .synthesis_bridge
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bridge);
    }

    pub fn clear_synthesis_bridge(&self) {
        *self
            .synthesis_bridge
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn synthesis_enabled(&self) -> bool {
        self.synthesis_bridge
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|bridge| bridge.is_enabled())
    }

    pub fn record_memory_evidence(
        &self,
        agent_id: &str,
        evidence_id: Option<String>,
        user: &str,
        assistant: &str,
        occurred_at: i64,
    ) -> bool {
        let bridge = self
            .synthesis_bridge
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        let Some(bridge) = bridge.filter(|bridge| bridge.is_enabled()) else {
            return false;
        };
        bridge.record_turn(
            agent_id,
            evidence_id,
            user,
            assistant,
            occurred_at,
        );
        true
    }

    pub fn synthesis_target_for_agent(&self, agent_id: &str) -> Option<FileMemoryStore> {
        let agent_dir = self.agents_root_dir.join(agent_id);
        agent_dir.is_dir().then(|| {
            FileMemoryStore::with_synthesis_metadata(get_agent_memory_dir(agent_dir), true)
        })
    }

    pub fn list_synthesis_targets(&self) -> Vec<(String, FileMemoryStore)> {
        let mut agent_ids = fs::read_dir(&self.agents_root_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        agent_ids.sort();
        agent_ids
            .into_iter()
            .filter_map(|agent_id| {
                self.synthesis_target_for_agent(&agent_id)
                    .map(|store| (agent_id, store))
            })
            .collect()
    }

    pub fn agent_has_content(&self, agent_dir: impl AsRef<Path>) -> bool {
        agent_memory_has_content(agent_dir)
    }

    pub fn sand_root(&self) -> &Path {
        &self.sand_root
    }

    pub fn agents_root_dir(&self) -> &Path {
        &self.agents_root_dir
    }
}

pub fn get_agent_memory_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(MEMORY_DIRNAME)
}

pub fn normalize_memory_content(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MEMORY_MAX_CONTENT_LENGTH)
        .collect()
}

pub fn memory_dedupe_key(content: &str) -> String {
    normalize_memory_content(content).to_lowercase()
}

pub fn memory_id_for(content: &str) -> String {
    let digest = Sha1::digest(memory_dedupe_key(content).as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .chars()
        .take(16)
        .collect()
}

pub fn format_memory_date(created_at: i64) -> String {
    if created_at <= 0 {
        return "unknown date".into();
    }
    DateTime::<Utc>::from_timestamp_millis(created_at)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown date".into())
}

pub fn serialize_fact_line(content: &str, created_at: i64) -> String {
    format!("- ({}) {}", format_memory_date(created_at), content)
}

pub fn parse_facts(raw: &str, kind: MemoryKind) -> Vec<MemoryFact> {
    raw.lines()
        .filter_map(|line| parse_fact_line(line, kind))
        .collect()
}

pub fn agent_memory_has_content(agent_dir: impl AsRef<Path>) -> bool {
    let memory_dir = get_agent_memory_dir(agent_dir);
    let profile_path = memory_dir.join(PROFILE_FILENAME);
    if read_valid_facts(&profile_path, MemoryKind::Profile).is_some_and(|facts| !facts.is_empty()) {
        return true;
    }

    let log_dir = memory_dir.join(LOG_DIRNAME);
    let Ok(entries) = fs::read_dir(log_dir) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        })
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("md"))
        .any(|entry| {
            read_valid_facts(&entry.path(), MemoryKind::Log).is_some_and(|facts| !facts.is_empty())
        })
}

pub fn get_user_memory_dir(sand_root: impl AsRef<Path>) -> PathBuf {
    sand_root.as_ref().join("user-memory")
}

pub fn get_user_memory_shards_dir(sand_root: impl AsRef<Path>) -> PathBuf {
    get_user_memory_dir(sand_root).join("agents")
}

pub fn get_user_memory_shard_dir(sand_root: impl AsRef<Path>, agent_id: &str) -> PathBuf {
    get_user_memory_shards_dir(sand_root).join(agent_id)
}

pub fn get_projects_root_dir(sand_root: impl AsRef<Path>) -> PathBuf {
    sand_root.as_ref().join("projects")
}

pub fn get_project_dir(sand_root: impl AsRef<Path>, slug: &str) -> PathBuf {
    get_projects_root_dir(sand_root).join(slug)
}

pub fn get_project_memory_shards_dir(sand_root: impl AsRef<Path>, slug: &str) -> PathBuf {
    get_project_dir(sand_root, slug)
        .join("memory")
        .join("agents")
}

pub fn get_project_memory_shard_dir(
    sand_root: impl AsRef<Path>,
    slug: &str,
    agent_id: &str,
) -> PathBuf {
    get_project_memory_shards_dir(sand_root, slug).join(agent_id)
}

pub fn project_dir_exists(sand_root: impl AsRef<Path>, slug: &str) -> bool {
    fs::metadata(get_project_dir(sand_root, slug))
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn read_child_directory_names(path: &Path) -> Vec<String> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}
