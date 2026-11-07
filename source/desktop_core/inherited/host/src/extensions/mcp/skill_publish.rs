use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use tar::{Builder, HeaderMode};
use uuid::Uuid;

use crate::workflows::workflow_library::{
    GlobalWorkflowLibrary, GlobalWorkflowRecord, LEGACY_WORKFLOW_FILENAME,
    get_global_workflows_dir, parse_workflow_file,
};
use super::plugin_skills::SandPluginSkillsService;
use super::plugin_skills_cache::{PluginSkillRecord, PluginSkillsCache};

pub const PUBLISH_SKILL_RPC_TIMEOUT_MS: u64 = 60_000;
pub const PUBLISH_TARGETS_RPC_TIMEOUT_MS: u64 = 10_000;
pub const CONFIRM_PUBLISH_MAX_ATTEMPTS: usize = 5;
pub const RUNS_FILENAME: &str = "runs.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPublishTeam { pub team_id: i32, pub name: String, pub is_direct_member: bool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishTarget { pub team_id: i32, pub name: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishTargetsResult { pub teams: Vec<PublishTarget>, pub unavailable_reason: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPluginInput {
    pub team_id: i32, pub name: String, pub display_name: String,
    pub description: String, pub plugin_tar_gz: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSkillResult { pub plugin_id: String, pub commit_sha: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishSkillResult { pub published: PublishedSkillResult, pub promoted_workflow_id: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpublishSkillResult { pub restored_workflow_id: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPublishEdgeFailure { pub stage: &'static str, pub error_class: String }

pub trait SkillPublishClient: Send + Sync {
    fn get_teams(&self, timeout_ms: u64) -> Result<Vec<SkillPublishTeam>, String>;
    fn publish_plugin(&self, request: PublishPluginInput, timeout_ms: u64) -> Result<PublishedSkillResult, String>;
    fn unpublish_plugin(&self, plugin_id: &str, team_id: i32, timeout_ms: u64) -> Result<(), String>;
}
pub trait SkillPublishPluginSkills: Send + Sync {
    fn current_index(&self) -> Option<PluginSkillsCache>;
    fn sync(&self, trigger: &str) -> Result<Vec<PluginSkillRecord>, String>;
}
impl SkillPublishPluginSkills for SandPluginSkillsService {
    fn current_index(&self) -> Option<PluginSkillsCache> { SandPluginSkillsService::current_index(self) }
    fn sync(&self, trigger: &str) -> Result<Vec<PluginSkillRecord>, String> { SandPluginSkillsService::sync(self, trigger) }
}

pub struct SandSkillPublishService {
    sand_root_dir: PathBuf,
    client: Arc<dyn SkillPublishClient>,
    plugin_skills: Arc<dyn SkillPublishPluginSkills>,
    library: GlobalWorkflowLibrary,
    log: Option<Arc<dyn Fn(String) + Send + Sync>>,
    report_edge_failed: Option<Arc<dyn Fn(SkillPublishEdgeFailure) + Send + Sync>>,
}

impl SandSkillPublishService {
    pub fn new(
        sand_root_dir: impl Into<PathBuf>,
        client: Arc<dyn SkillPublishClient>,
        plugin_skills: Arc<dyn SkillPublishPluginSkills>,
    ) -> Self {
        let sand_root_dir = sand_root_dir.into();
        let library = GlobalWorkflowLibrary::new(get_global_workflows_dir(&sand_root_dir));
        Self { sand_root_dir, client, plugin_skills, library, log: None, report_edge_failed: None }
    }
    pub fn with_log(mut self, log: Arc<dyn Fn(String) + Send + Sync>) -> Self { self.log = Some(log); self }
    pub fn with_edge_reporter(mut self, reporter: Arc<dyn Fn(SkillPublishEdgeFailure) + Send + Sync>) -> Self {
        self.report_edge_failed = Some(reporter); self
    }
    pub fn library(&self) -> &GlobalWorkflowLibrary { &self.library }

    pub fn list_targets(&self) -> PublishTargetsResult {
        let response = match self.client.get_teams(PUBLISH_TARGETS_RPC_TIMEOUT_MS) {
            Ok(value) => value,
            Err(error) => {
                self.log(format!("[sand:skill-publish] failed to resolve publishable teams: {error}"));
                self.report("list_targets", classify_error(&error));
                return PublishTargetsResult {
                    teams: Vec::new(),
                    unavailable_reason: Some("Could not reach Cursor to check your teams.".to_string()),
                };
            }
        };
        let teams = publishable_teams(&response);
        PublishTargetsResult {
            unavailable_reason: teams.is_empty().then(|| "Publishing a skill needs a Cursor team. Join or create one, then try again.".to_string()),
            teams,
        }
    }

    pub fn publish(&self, workflow_id: &str, team_id: i32) -> Result<PublishSkillResult, String> {
        let record = self.require_library_record(workflow_id)?;
        if team_id <= 0 { return Err("Choose a team to publish this skill to.".to_string()); }
        let skill_dir = record.file_path.parent().ok_or_else(|| "That skill's file could not be read.".to_string())?;
        let published = self.upload(UploadArgs {
            skill_dir, skill_relative_path: &record.id, name: &record.name,
            description: &record.description, team_id, plugin_name: &record.name, display_name: None,
        })?;
        let landed_id = self.confirm_publish_landed(&published);
        let promoted_workflow_id = match landed_id {
            Some(id) => match self.library.remove(&record.id) { Ok(true) => Some(id), _ => None },
            None => None,
        };
        if promoted_workflow_id.is_none() {
            self.log(format!("[sand:skill-publish] kept the library copy of {}: commit {} was not confirmed installed", record.id, published.commit_sha));
        }
        Ok(PublishSkillResult { published, promoted_workflow_id })
    }

    pub fn resync(&self, workflow_id: &str) -> Result<PublishSkillResult, String> {
        let (record, team_id) = self.require_published_plugin_skill(workflow_id)?;
        let parsed = read_skill_frontmatter(Path::new(&record.file_path))?;
        let name = if parsed.name.is_empty() { record.name.clone() } else { parsed.name };
        let plugin_name = require_manifest_name(Path::new(&record.install_path))?;
        let relative_path = skills_root_relative_path(&record.skill_relative_path)?;
        let skill_dir = Path::new(&record.file_path).parent().ok_or_else(|| "That skill's file could not be read.".to_string())?;
        let published = self.upload(UploadArgs {
            skill_dir, skill_relative_path: &relative_path, name: &name,
            description: &parsed.description, team_id, plugin_name: &plugin_name, display_name: Some(&name),
        })?;
        self.sync_best_effort();
        Ok(PublishSkillResult { published, promoted_workflow_id: None })
    }

    pub fn unpublish(&self, workflow_id: &str) -> Result<UnpublishSkillResult, String> {
        let (record, team_id) = self.require_published_plugin_skill(workflow_id)?;
        let restored_workflow_id = self.restore_to_library(&record)?;
        self.client.unpublish_plugin(&record.plugin_id, team_id, PUBLISH_SKILL_RPC_TIMEOUT_MS)?;
        self.sync_best_effort();
        Ok(UnpublishSkillResult { restored_workflow_id })
    }

    pub fn restore_to_library(&self, record: &PluginSkillRecord) -> Result<Option<String>, String> {
        let skill_dir = Path::new(&record.file_path).parent().ok_or_else(|| "That skill's file could not be read.".to_string())?;
        let library_id = skill_dir.file_name().and_then(|v| v.to_str()).ok_or_else(|| "That skill's file could not be read.".to_string())?.to_string();
        if self.library.get(&library_id).is_some() { return Ok(Some(library_id)); }
        let plugin_skills_root = Path::new(&record.install_path).join("skills");
        let relative = safe_relative_child(&plugin_skills_root, skill_dir).ok_or_else(|| format!("Skill folder \"{}\" is not inside the plugin's skills directory", skill_dir.display()))?;
        let skills_root = get_global_workflows_dir(&self.sand_root_dir);
        let target = skills_root.join(&relative);
        if target.exists() { return Err(format!("Cannot restore skill to \"{}\": that path already exists", target.display())); }
        copy_tree(skill_dir, &target, None)?;
        Ok((target.parent() == Some(skills_root.as_path())).then(|| target.file_name().unwrap().to_string_lossy().to_string()))
    }

    fn upload(&self, args: UploadArgs<'_>) -> Result<PublishedSkillResult, String> {
        if args.description.trim().is_empty() {
            return Err("Add a description before publishing — it is how teammates and agents know when to use this skill.".to_string());
        }
        let work_dir = std::env::temp_dir().join(format!("sand-publish-skill-{}", Uuid::new_v4()));
        fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
        let result = (|| {
            let staged_root = work_dir.join("staged");
            fs::create_dir_all(&staged_root).map_err(|e| e.to_string())?;
            let staged = stage_skill_dir_for_publish(args.skill_dir, &staged_root)?;
            let plugin_dir = synthesize_skill_plugin_dir(
                &staged, args.skill_relative_path, &work_dir.join("plugin"),
                args.plugin_name, args.display_name,
            )?;
            let manifest_name = read_manifest_name(&plugin_dir).unwrap_or_else(|| args.plugin_name.to_string());
            self.client.publish_plugin(PublishPluginInput {
                team_id: args.team_id,
                name: manifest_name,
                display_name: args.display_name.unwrap_or(args.name).to_string(),
                description: args.description.to_string(),
                plugin_tar_gz: pack_plugin_artifact(&plugin_dir)?,
            }, PUBLISH_SKILL_RPC_TIMEOUT_MS)
        })();
        if let Err(error) = fs::remove_dir_all(&work_dir) {
            self.log(format!("[sand:skill-publish] failed to clean up {}: {error}", work_dir.display()));
            self.report("cleanup", classify_error(&error.to_string()));
        }
        result
    }

    fn confirm_publish_landed(&self, published: &PublishedSkillResult) -> Option<String> {
        for _ in 0..CONFIRM_PUBLISH_MAX_ATTEMPTS {
            self.sync_best_effort();
            if let Some(index) = self.plugin_skills.current_index() {
                if let Some(record) = index.skills.into_iter().find(|record| {
                    record.plugin_id == published.plugin_id && record.plugin_version == published.commit_sha && Path::new(&record.file_path).exists()
                }) { return Some(record.id); }
            }
        }
        None
    }

    fn require_library_record(&self, workflow_id: &str) -> Result<GlobalWorkflowRecord, String> {
        self.library.get(workflow_id).ok_or_else(|| "That skill no longer exists in your library.".to_string())
    }
    fn require_published_plugin_skill(&self, workflow_id: &str) -> Result<(PluginSkillRecord, i32), String> {
        let index = self.plugin_skills.current_index().ok_or_else(|| "That skill is no longer installed.".to_string())?;
        let record = index.skills.into_iter().find(|candidate| candidate.id == workflow_id)
            .ok_or_else(|| "That skill is no longer installed.".to_string())?;
        if record.publisher_user_id.is_none() || record.publisher_user_id != index.current_user_id {
            return Err(format!("\"{}\" belongs to a plugin you did not publish.", record.name));
        }
        let team_id = record.marketplace_team_id.ok_or_else(|| format!("\"{}\" is not in a team marketplace, so there is nothing to sync it back to.", record.name))?;
        Ok((record, i32::try_from(team_id).map_err(|_| "That skill's team id is not supported.".to_string())?))
    }
    fn sync_best_effort(&self) {
        if let Err(error) = self.plugin_skills.sync("install") {
            self.log(format!("[sand:skill-publish] plugin-skills sync after publish failed: {error}"));
            self.report("sync", classify_error(&error));
        }
    }
    fn log(&self, message: String) { if let Some(log) = &self.log { log(message); } }
    fn report(&self, stage: &'static str, error_class: String) {
        if let Some(report) = &self.report_edge_failed { report(SkillPublishEdgeFailure { stage, error_class }); }
    }
}

struct UploadArgs<'a> {
    skill_dir: &'a Path, skill_relative_path: &'a str, name: &'a str,
    description: &'a str, team_id: i32, plugin_name: &'a str, display_name: Option<&'a str>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter { pub name: String, pub description: String }

pub fn publishable_teams(teams: &[SkillPublishTeam]) -> Vec<PublishTarget> {
    teams.iter().filter(|team| team.team_id > 0 && team.is_direct_member)
        .map(|team| PublishTarget { team_id: team.team_id, name: team.name.clone() }).collect()
}
pub fn read_skill_frontmatter(file_path: &Path) -> Result<SkillFrontmatter, String> {
    let raw = fs::read_to_string(file_path).map_err(|_| "That skill's file could not be read.".to_string())?;
    let parsed = parse_workflow_file(&raw).ok_or_else(|| "That skill's file could not be read.".to_string())?;
    Ok(SkillFrontmatter { name: parsed.spec.name, description: parsed.spec.description })
}
pub fn read_manifest_name(plugin_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(plugin_dir.join("plugin.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("name").and_then(serde_json::Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(ToOwned::to_owned)
}
pub fn require_manifest_name(install_path: &Path) -> Result<String, String> {
    read_manifest_name(install_path).ok_or_else(|| "Could not read that plugin's manifest, so syncing would risk publishing a duplicate. Reinstall the plugin and try again.".to_string())
}
pub fn skills_root_relative_path(skill_relative_path: &str) -> Result<String, String> {
    let parent = Path::new(skill_relative_path).parent().unwrap_or_else(|| Path::new(""));
    let mut segments = Vec::new();
    for component in parent.components() {
        match component {
            Component::Normal(value) => segments.push(value.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => return Err("That skill sits at a path Sand cannot re-pack.".to_string()),
        }
    }
    if segments.first().is_some_and(|value| value == "skills") { segments.remove(0); }
    if segments.is_empty() { return Err("That skill sits at a path Sand cannot re-pack.".to_string()); }
    Ok(segments.join("/"))
}
pub fn normalize_marketplace_name(value: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for byte in value.trim().to_ascii_lowercase().bytes() {
        if byte.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() { out.push('-'); }
            out.push(byte as char);
            pending_dash = false;
        } else { pending_dash = true; }
    }
    out
}
pub fn stage_skill_dir_for_publish(skill_dir: &Path, target_dir: &Path) -> Result<PathBuf, String> {
    let basename = skill_dir.file_name().ok_or_else(|| "That skill's file could not be read.".to_string())?;
    let staged = target_dir.join(basename);
    copy_tree(skill_dir, &staged, Some(&|source: &Path| {
        let root_child = source.parent() == Some(skill_dir);
        let name = source.file_name().and_then(|v| v.to_str());
        !(root_child && matches!(name, Some(LEGACY_WORKFLOW_FILENAME) | Some(RUNS_FILENAME)))
    }))?;
    Ok(staged)
}
pub fn synthesize_skill_plugin_dir(
    skill_dir: &Path, relative_path: &str, target_dir: &Path,
    plugin_name: &str, display_name: Option<&str>,
) -> Result<PathBuf, String> {
    let segments = safe_skill_path_segments(relative_path)?;
    let safe_name = normalize_marketplace_name(plugin_name);
    if safe_name.is_empty() { return Err(format!("Plugin name \"{plugin_name}\" has no usable characters")); }
    let plugin_dir = target_dir.join(&safe_name);
    let skills_root = plugin_dir.join("skills");
    fs::create_dir_all(&skills_root).map_err(|e| e.to_string())?;
    let destination = segments.iter().fold(skills_root, |path, segment| path.join(segment));
    copy_tree(skill_dir, &destination, None)?;
    #[derive(Serialize)] #[serde(rename_all = "camelCase")]
    struct Manifest<'a> { name: &'a str, display_name: &'a str, skills: Vec<String> }
    let manifest = Manifest {
        name: &safe_name,
        display_name: display_name.unwrap_or(plugin_name),
        skills: vec![format!("skills/{}", segments.join("/"))],
    };
    fs::write(plugin_dir.join("plugin.json"), serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(plugin_dir)
}
pub fn pack_plugin_artifact(dir_path: &Path) -> Result<Vec<u8>, String> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    builder.mode(HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    let mut entries = fs::read_dir(dir_path).map_err(|e| e.to_string())?.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path(); let name = entry.file_name();
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            builder.append_dir_all(Path::new(&name), &path).map_err(|e| e.to_string())?;
        } else {
            builder.append_path_with_name(&path, Path::new(&name)).map_err(|e| e.to_string())?;
        }
    }
    let encoder = builder.into_inner().map_err(|e| e.to_string())?;
    encoder.finish().map_err(|e| e.to_string())
}
fn safe_skill_path_segments(relative_path: &str) -> Result<Vec<String>, String> {
    let path = Path::new(relative_path);
    if path.is_absolute() { return Err(format!("Skill path \"{relative_path}\" must be relative to the skills root")); }
    let mut segments = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => segments.push(value.to_string_lossy().to_string()),
            _ => return Err(format!("Skill path \"{relative_path}\" resolves to an unsafe path")),
        }
    }
    if segments.is_empty() { return Err(format!("Skill path \"{relative_path}\" resolves to an unsafe path")); }
    Ok(segments)
}
fn safe_relative_child(parent: &Path, child: &Path) -> Option<PathBuf> {
    let parent = fs::canonicalize(parent).ok()?; let child = fs::canonicalize(child).ok()?;
    let relative = child.strip_prefix(parent).ok()?.to_path_buf();
    (!relative.as_os_str().is_empty()).then_some(relative)
}
fn copy_tree(source: &Path, target: &Path, filter: Option<&dyn Fn(&Path) -> bool>) -> Result<(), String> {
    if !fs::metadata(source).map_err(|e| e.to_string())?.is_dir() { return Err(format!("{} is not a directory", source.display())); }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?; let src = entry.path();
        if filter.is_some_and(|filter| !filter(&src)) { continue; }
        let dst = target.join(entry.file_name()); let meta = fs::metadata(&src).map_err(|e| e.to_string())?;
        if meta.is_dir() { copy_tree(&src, &dst, filter)?; }
        else if meta.is_file() { fs::copy(&src, &dst).map_err(|e| e.to_string())?; }
    }
    Ok(())
}
fn classify_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("timeout") { "timeout".into() }
    else if lower.contains("auth") || lower.contains("permission") { "auth".into() }
    else if lower.contains("network") || lower.contains("connect") { "network".into() }
    else { "unknown".into() }
}
