use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;

use serde::Deserialize;
use serde_json::Value;

use super::legacy_live_references::remove_workflow_live_references;
use super::plugin_skills_cache::{
    PluginAuthBlock, PluginSkillRecord, PluginSkillsCache, PluginSkillsCacheWriteIndex,
    get_plugin_skills_dir, get_plugins_root_dir, read_plugin_skills_cache,
    write_plugin_skills_cache,
};

pub const EFFECTIVE_PLUGINS_RPC_TIMEOUT_MS: u64 = 15_000;
pub const CURRENT_USER_RPC_TIMEOUT_MS: u64 = 10_000;
pub const PLUGIN_SKILLS_REFRESH_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginIdentifier {
    pub source: String,
    pub name: String,
    pub plugin_db_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    pub name: Option<String>,
    pub description: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub identifier: PluginIdentifier,
    pub display_name: Option<String>,
    pub load_error: Option<String>,
    pub install_path: PathBuf,
    pub skills: Vec<InstalledSkill>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherFacts {
    pub publisher_user_id: Option<u64>,
    pub marketplace_team_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLoadFailure {
    pub plugin_db_id: Option<String>,
    pub plugin_id: Option<String>,
    pub plugin_name: String,
    pub marketplace_name: Option<String>,
    pub error_message: String,
    pub error_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedCacheKey {
    pub marketplace_slug: String,
    pub plugin_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedPlugins {
    pub plugins: Vec<InstalledPlugin>,
    pub auth_blocked: Vec<PluginAuthBlock>,
    pub listed_plugin_ids: Vec<String>,
    pub listed_cache_keys: Vec<ListedCacheKey>,
    pub publisher_facts: HashMap<String, PublisherFacts>,
    pub current_user_id: Option<u64>,
}

pub fn skill_records_identity(records: &[PluginSkillRecord]) -> String {
    let mut values = records
        .iter()
        .map(|record| format!("{}@{}", record.id, record.plugin_version))
        .collect::<Vec<_>>();
    values.sort();
    values.join("\n")
}

pub fn skill_name_from_path(relative_path: &str) -> String {
    let segments = relative_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return String::new();
    }
    let file_index = segments.len() - 1;
    if file_index > 0 {
        segments[file_index - 1].to_string()
    } else {
        segments[file_index].to_string()
    }
}

pub fn plugin_version_of(identifier: &PluginIdentifier) -> String {
    identifier.version.clone().unwrap_or_default()
}

fn plugin_db_id(identifier: &PluginIdentifier) -> Option<&str> {
    matches!(
        identifier.source.as_str(),
        "cursor-first-party" | "cursor-third-party"
    )
    .then_some(identifier.plugin_db_id.as_deref())
    .flatten()
}

fn clamp_line(value: &str, max: usize) -> String {
    value
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 64 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "workflow".into()
    } else {
        out
    }
}

pub fn plugin_contents_to_skill_records(
    plugins: &[InstalledPlugin],
    publisher_facts: &HashMap<String, PublisherFacts>,
) -> Vec<PluginSkillRecord> {
    let mut records = Vec::new();
    let mut used_ids = HashSet::new();
    for plugin in plugins {
        if plugin.load_error.is_some() || plugin.install_path.as_os_str().is_empty() {
            continue;
        }
        let Some(plugin_id) = plugin_db_id(&plugin.identifier) else {
            continue;
        };
        let plugin_name = plugin
            .display_name
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(&plugin.identifier.name);
        for skill in &plugin.skills {
            let raw_name = skill
                .name
                .as_deref()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| skill_name_from_path(&skill.path));
            let name = clamp_line(&raw_name, 80);
            if name.is_empty() {
                continue;
            }
            let slug = slugify(&name);
            let base = format!("plugin-{plugin_id}-{slug}");
            let mut id = base.clone();
            let mut suffix = 2usize;
            while used_ids.contains(&id) {
                id = format!("{base}-{suffix}");
                suffix += 1;
            }
            used_ids.insert(id.clone());
            let facts = publisher_facts.get(plugin_id).copied().unwrap_or_default();
            records.push(PluginSkillRecord {
                id,
                plugin_id: plugin_id.to_string(),
                plugin_name: plugin_name.to_string(),
                name,
                description: clamp_line(skill.description.as_deref().unwrap_or_default(), 1_536),
                file_path: plugin
                    .install_path
                    .join(&skill.path)
                    .to_string_lossy()
                    .into_owned(),
                plugin_version: plugin_version_of(&plugin.identifier),
                install_path: plugin.install_path.to_string_lossy().into_owned(),
                skill_relative_path: skill.path.clone(),
                publisher_user_id: facts.publisher_user_id,
                marketplace_team_id: facts.marketplace_team_id,
            });
        }
    }
    records
}

pub fn plugin_auth_blocks_from_failures(failures: &[PluginLoadFailure]) -> Vec<PluginAuthBlock> {
    let mut blocks = Vec::new();
    let mut seen = HashSet::new();
    for failure in failures {
        let auth_failure = failure.error_kind.as_deref() == Some("user_git_access")
            || failure
                .error_message
                .to_ascii_lowercase()
                .contains("permission denied")
            || failure
                .error_message
                .to_ascii_lowercase()
                .contains("authentication");
        if !auth_failure {
            continue;
        }
        let plugin_id = failure
            .plugin_db_id
            .as_deref()
            .or(failure.plugin_id.as_deref())
            .unwrap_or_default()
            .to_string();
        let key = if plugin_id.is_empty() {
            format!("name:{}", failure.plugin_name)
        } else {
            plugin_id.clone()
        };
        if !seen.insert(key) {
            continue;
        }
        blocks.push(PluginAuthBlock {
            plugin_id,
            plugin_name: failure.plugin_name.clone(),
            marketplace_name: failure.marketplace_name.clone(),
        });
    }
    blocks
}

fn is_within(parent: &Path, child: &Path) -> bool {
    child.strip_prefix(parent).is_ok()
}

pub fn prune_uninstalled_plugin_dirs(
    cache_root: &Path,
    listed: &[ListedCacheKey],
    indexed_file_paths: &[String],
) {
    let keep = listed
        .iter()
        .map(|key| cache_root.join(&key.marketplace_slug).join(&key.plugin_id))
        .collect::<HashSet<_>>();
    let Ok(slugs) = fs::read_dir(cache_root) else {
        return;
    };
    for slug in slugs.filter_map(Result::ok) {
        if !slug.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(plugins) = fs::read_dir(slug.path()) else {
            continue;
        };
        for plugin in plugins.filter_map(Result::ok) {
            if !plugin.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let dir = plugin.path();
            if keep.contains(&dir)
                || indexed_file_paths
                    .iter()
                    .any(|path| is_within(&dir, Path::new(path)))
            {
                continue;
            }
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSkillsSyncEvent {
    pub trigger: String,
    pub outcome: String,
    pub changed: bool,
    pub skill_count: usize,
    pub duration_ms: i64,
    pub error_class: Option<String>,
}

pub trait PluginSkillsLoader: Send + Sync {
    fn load(&self) -> Result<LoadedPlugins, String>;
}

type SyncResult = Result<Vec<PluginSkillRecord>, String>;

#[derive(Default)]
struct SyncPass {
    // Lock ordering: service state, then pass result. A pass outlives its slot
    // so every coalesced caller can observe the same outcome.
    result: Mutex<Option<SyncResult>>,
}

struct PendingSync {
    trigger: String,
    pass: Arc<SyncPass>,
}

#[derive(Default)]
struct ServiceState {
    disposed: bool,
    active: Option<Arc<SyncPass>>,
    active_thread: Option<ThreadId>,
    pending: Option<PendingSync>,
}

struct ActiveSyncGuard<'a> {
    service: &'a SandPluginSkillsService,
    pass: Arc<SyncPass>,
    completed: bool,
}

impl Drop for ActiveSyncGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            // A loader/reporter panic must not orphan this pass's other callers
            // or leave every future refresh waiting on a dead executor.
            self.service.complete_pass(
                &self.pass,
                Err("plugin skills sync aborted before completion".into()),
            );
        }
    }
}

pub struct SandPluginSkillsService {
    sand_root_dir: PathBuf,
    loader: Arc<dyn PluginSkillsLoader>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    report_sync: Option<Arc<dyn Fn(PluginSkillsSyncEvent) + Send + Sync>>,
    state: Mutex<ServiceState>,
    changed: Condvar,
}

impl SandPluginSkillsService {
    pub fn new(sand_root_dir: impl Into<PathBuf>, loader: Arc<dyn PluginSkillsLoader>) -> Self {
        Self {
            sand_root_dir: sand_root_dir.into(),
            loader,
            now: Arc::new(|| chrono::Utc::now().timestamp_millis()),
            report_sync: None,
            state: Mutex::new(ServiceState::default()),
            changed: Condvar::new(),
        }
    }

    pub fn with_now(mut self, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.now = now;
        self
    }

    pub fn with_reporter(
        mut self,
        reporter: Arc<dyn Fn(PluginSkillsSyncEvent) + Send + Sync>,
    ) -> Self {
        self.report_sync = Some(reporter);
        self
    }

    pub fn current_index(&self) -> Option<PluginSkillsCache> {
        read_plugin_skills_cache(get_plugin_skills_dir(&self.sand_root_dir))
    }

    pub fn current(&self) -> Vec<PluginSkillRecord> {
        self.current_index()
            .map(|index| index.skills)
            .unwrap_or_default()
    }

    pub fn current_auth_blocked(&self) -> Vec<PluginAuthBlock> {
        self.current_index()
            .map(|index| index.auth_blocked)
            .unwrap_or_default()
    }

    pub fn remove_live_references(&self, source_urls: &[String]) {
        let _ = remove_workflow_live_references(&self.sand_root_dir, source_urls);
    }

    pub fn dispose(&self) {
        let mut state = self.state.lock().expect("plugin skills state poisoned");
        state.disposed = true;
        state.pending = None;
        self.changed.notify_all();
    }

    pub fn sync(&self, trigger: &str) -> SyncResult {
        let mut state = self.state.lock().expect("plugin skills state poisoned");
        if state.disposed {
            drop(state);
            return Ok(self.current());
        }
        if state.active_thread == Some(std::thread::current().id()) {
            // Unlike a JS promise, this blocking API cannot await the callback
            // that is currently calling it. Fail explicitly instead of hanging.
            return Err("recursive plugin skills sync cannot wait on its own pass".into());
        }
        if state.active.is_none() && state.pending.is_none() {
            let pass = Arc::new(SyncPass::default());
            state.active = Some(Arc::clone(&pass));
            state.active_thread = Some(std::thread::current().id());
            drop(state);
            return self.execute_pass(pass, trigger);
        }

        // Exactly one follow-up is shared by all callers arriving during a
        // pass. New arrivals must not overtake a follow-up awaiting its worker.
        let pending = state.pending.get_or_insert_with(|| PendingSync {
            trigger: trigger.to_string(),
            pass: Arc::new(SyncPass::default()),
        });
        pending.trigger = trigger.to_string();
        let pass = Arc::clone(&pending.pass);
        self.changed.notify_all();
        loop {
            let result = pass
                .result
                .lock()
                .expect("plugin skills pass poisoned")
                .clone();
            if let Some(result) = result {
                return result;
            }
            if state.disposed {
                drop(state);
                return Ok(self.current());
            }
            if state.active.is_none()
                && state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| Arc::ptr_eq(&pending.pass, &pass))
            {
                let pending = state.pending.take().expect("pending pass exists");
                state.active = Some(Arc::clone(&pass));
                state.active_thread = Some(std::thread::current().id());
                drop(state);
                return self.execute_pass(pass, &pending.trigger);
            }
            state = self
                .changed
                .wait(state)
                .expect("plugin skills state poisoned");
        }
    }

    fn execute_pass(&self, pass: Arc<SyncPass>, trigger: &str) -> SyncResult {
        let mut guard = ActiveSyncGuard {
            service: self,
            pass,
            completed: false,
        };
        // All loading, cache IO and callbacks are outside the state/result locks.
        let result = self.run_pass(trigger);
        self.complete_pass(&guard.pass, result.clone());
        guard.completed = true;
        result
    }

    fn complete_pass(&self, pass: &Arc<SyncPass>, result: SyncResult) {
        let mut state = self.state.lock().expect("plugin skills state poisoned");
        *pass.result.lock().expect("plugin skills pass poisoned") = Some(result);
        if state
            .active
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, pass))
        {
            state.active = None;
            state.active_thread = None;
        }
        self.changed.notify_all();
    }

    fn run_pass(&self, trigger: &str) -> Result<Vec<PluginSkillRecord>, String> {
        let started = (self.now)();
        let loaded = match self.loader.load() {
            Ok(value) => value,
            Err(error) => {
                if let Some(report) = &self.report_sync {
                    report(PluginSkillsSyncEvent {
                        trigger: trigger.into(),
                        outcome: "failed".into(),
                        changed: false,
                        skill_count: self.current().len(),
                        duration_ms: (self.now)().saturating_sub(started),
                        error_class: Some("load_failed".into()),
                    });
                }
                return Err(error);
            }
        };
        if self
            .state
            .lock()
            .expect("plugin skills state poisoned")
            .disposed
        {
            return Ok(self.current());
        }
        let previous = self.current_index();
        let mut records =
            plugin_contents_to_skill_records(&loaded.plugins, &loaded.publisher_facts);
        let listed = loaded
            .listed_plugin_ids
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let loaded_ids = loaded
            .plugins
            .iter()
            .filter(|plugin| {
                plugin.load_error.is_none() && !plugin.install_path.as_os_str().is_empty()
            })
            .filter_map(|plugin| plugin_db_id(&plugin.identifier).map(str::to_string))
            .collect::<HashSet<_>>();
        if let Some(previous) = &previous {
            records.extend(
                previous
                    .skills
                    .iter()
                    .filter(|record| {
                        listed.contains(&record.plugin_id)
                            && !loaded_ids.contains(&record.plugin_id)
                    })
                    .cloned(),
            );
        }

        let values = records
            .iter()
            .map(|record| serde_json::to_value(record).expect("plugin skill record serializes"))
            .collect::<Vec<Value>>();
        write_plugin_skills_cache(
            get_plugin_skills_dir(&self.sand_root_dir),
            &PluginSkillsCacheWriteIndex {
                current_user_id: Some(
                    loaded
                        .current_user_id
                        .or_else(|| previous.as_ref().and_then(|value| value.current_user_id)),
                ),
                skills: values,
                auth_blocked: Some(loaded.auth_blocked.clone()),
            },
            || (self.now)() as f64,
        )?;
        prune_uninstalled_plugin_dirs(
            &get_plugins_root_dir(&self.sand_root_dir).join("cache"),
            &loaded.listed_cache_keys,
            &records
                .iter()
                .map(|record| record.file_path.clone())
                .collect::<Vec<_>>(),
        );
        if let Some(report) = &self.report_sync {
            let changed = previous.as_ref().is_none_or(|value| {
                skill_records_identity(&value.skills) != skill_records_identity(&records)
            });
            report(PluginSkillsSyncEvent {
                trigger: trigger.into(),
                outcome: "ok".into(),
                changed,
                skill_count: records.len(),
                duration_ms: (self.now)().saturating_sub(started),
                error_class: None,
            });
        }
        Ok(records)
    }
}

#[cfg(test)]
#[path = "plugin_skills_sync_tests.rs"]
mod sync_tests;
