//! Execute-plan plan-file ownership for the native iOS Agent/Host.
//!
//! Desktop keeps the plan file as a durable resource resolved through the
//! executor resource accessor. iOS has no standalone Desktop-style
//! receiver-contract.rs owner, so the canonical Feature Host owns this
//! responsibility and delegates actual filesystem IO through this module.
//!
//! The module deliberately keeps three boundaries explicit:
//! 1. path validation is scoped to one account/agent root;
//! 2. resource probing/writing is injectable for focused tests;
//! 3. the plan registry is durable and account-scoped, never global.

use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const PLAN_REGISTRY_VERSION: u32 = 1;
const PLAN_REGISTRY_FILE_NAME: &str = "plan-registry.json";
const DEFAULT_PLAN_FILE_NAME: &str = "plan.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFileAvailability {
    OnDisk,
    Unknown,
    Unavailable,
}

impl PlanFileAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnDisk => "on_disk",
            Self::Unknown => "unknown",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutePlanPlanFileResolution {
    pub path: Option<PathBuf>,
    pub availability: PlanFileAvailability,
    pub recreated: bool,
    pub should_upsert_plan_registry_entry: bool,
    pub failure_reason: Option<String>,
}

impl ExecutePlanPlanFileResolution {
    fn on_disk(path: PathBuf, recreated: bool) -> Self {
        Self {
            path: Some(path),
            availability: PlanFileAvailability::OnDisk,
            recreated,
            should_upsert_plan_registry_entry: true,
            failure_reason: None,
        }
    }

    fn unknown(reason: impl Into<String>) -> Self {
        Self {
            path: None,
            availability: PlanFileAvailability::Unknown,
            recreated: false,
            should_upsert_plan_registry_entry: false,
            failure_reason: Some(reason.into()),
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            path: None,
            availability: PlanFileAvailability::Unavailable,
            recreated: false,
            should_upsert_plan_registry_entry: false,
            failure_reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanFileResourceError {
    InvalidPath(String),
    NotFound,
    PermissionDenied(String),
    Io(String),
}

impl PlanFileResourceError {
    fn message(&self) -> String {
        match self {
            Self::InvalidPath(message) => message.clone(),
            Self::NotFound => "resource not found".to_string(),
            Self::PermissionDenied(message) => message.clone(),
            Self::Io(message) => message.clone(),
        }
    }
}

/// The Host-owned resource boundary used by execute-plan.
///
/// The Desktop implementation receives an executor resource accessor. iOS
/// keeps that boundary explicit even though the production accessor is local
/// sandbox storage rather than an Electron resource provider.
pub trait ExecutePlanPlanFileResourceAccessor {
    fn probe_readable(&self, path: &Path) -> Result<bool, PlanFileResourceError>;
    fn write_text(&self, path: &Path, content: &str) -> Result<(), PlanFileResourceError>;
}

#[derive(Debug, Clone, Copy)]
pub struct LocalPlanFileResourceAccessor<'a> {
    allowed_root: &'a Path,
}

impl<'a> LocalPlanFileResourceAccessor<'a> {
    pub fn new(allowed_root: &'a Path) -> Self {
        Self { allowed_root }
    }

    fn validate(&self, path: &Path) -> Result<PathBuf, PlanFileResourceError> {
        scoped_path(self.allowed_root, path.to_string_lossy().as_ref())
            .map_err(PlanFileResourceError::InvalidPath)
    }
}

impl ExecutePlanPlanFileResourceAccessor for LocalPlanFileResourceAccessor<'_> {
    fn probe_readable(&self, path: &Path) -> Result<bool, PlanFileResourceError> {
        let path = self.validate(path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(PlanFileResourceError::PermissionDenied(error.to_string()));
            }
            Err(error) => return Err(PlanFileResourceError::Io(error.to_string())),
        };
        if !metadata.file_type().is_file() {
            return Ok(false);
        }

        let mut file = File::open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                PlanFileResourceError::PermissionDenied(error.to_string())
            } else {
                PlanFileResourceError::Io(error.to_string())
            }
        })?;
        let mut probe = [0_u8; 1];
        match file.read(&mut probe) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(PlanFileResourceError::PermissionDenied(error.to_string()))
            }
            Err(error) => Err(PlanFileResourceError::Io(error.to_string())),
        }
    }

    fn write_text(&self, path: &Path, content: &str) -> Result<(), PlanFileResourceError> {
        let path = self.validate(path)?;
        atomic_write_text(&path, content).map_err(PlanFileResourceError::Io)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanRegistryEntry {
    pub id: String,
    pub path: String,
    #[serde(rename = "updatedAtMs")]
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PlanRegistryFile {
    version: u32,
    entries: Vec<PlanRegistryEntry>,
}

impl Default for PlanRegistryFile {
    fn default() -> Self {
        Self {
            version: PLAN_REGISTRY_VERSION,
            entries: Vec::new(),
        }
    }
}

pub fn plan_registry_path(agent_root: &Path) -> PathBuf {
    agent_root.join("artifacts").join(PLAN_REGISTRY_FILE_NAME)
}

/// Resolve the plan file exactly once before execution starts.
///
/// This mirrors the Desktop execute-plan helper:
/// - probe the supplied path through the resource accessor;
/// - if unavailable, safely recreate it under artifacts/plans;
/// - sanitize the fallback name;
/// - preserve unknown/unavailable semantics instead of fabricating success.
pub fn resolve_plan_file_path<A: ExecutePlanPlanFileResourceAccessor>(
    agent_root: &Path,
    artifacts_folder: Option<&Path>,
    provided_plan_file_path: Option<&str>,
    plan_id: Option<&str>,
    plan_file_content: &str,
    accessor: &A,
) -> ExecutePlanPlanFileResolution {
    let Some(artifacts_folder) = artifacts_folder else {
        return ExecutePlanPlanFileResolution::unknown(
            "artifacts folder is unavailable; plan file was not recreated",
        );
    };

    let artifacts_folder = match scoped_path(agent_root, &artifacts_folder.to_string_lossy()) {
        Ok(path) => path,
        Err(error) => {
            return ExecutePlanPlanFileResolution::unavailable(format!(
                "invalid artifacts folder: {error}"
            ));
        }
    };

    if let Some(raw_path) = provided_plan_file_path {
        match scoped_path(agent_root, raw_path) {
            Ok(path) => match accessor.probe_readable(&path) {
                Ok(true) => return ExecutePlanPlanFileResolution::on_disk(path, false),
                Ok(false) => {}
                Err(error) => {
                    // The Desktop action treats executor read failures as a
                    // recoverable miss and attempts the bounded reconstruction.
                    let _ = error.message();
                }
            },
            Err(_) => {
                // An unsafe supplied path must never become a write target.
                // Reconstruction below derives a fresh bounded path.
            }
        }
    }

    let fallback_name = sanitize_plan_file_name(provided_plan_file_path, plan_id);
    let plans_folder = artifacts_folder.join("plans");
    let fallback_path = plans_folder.join(fallback_name);

    if let Err(error) = validate_write_target(agent_root, &fallback_path) {
        return ExecutePlanPlanFileResolution::unavailable(format!(
            "invalid plan reconstruction path: {error}"
        ));
    }

    match accessor.write_text(&fallback_path, plan_file_content) {
        Ok(()) => ExecutePlanPlanFileResolution::on_disk(fallback_path, true),
        Err(error) => ExecutePlanPlanFileResolution::unavailable(format!(
            "plan file reconstruction failed: {}",
            error.message()
        )),
    }
}

/// Recover durable plan content after a restart.
///
/// Inline content wins when supplied by the current action. Otherwise the
/// current account/agent registry is consulted, followed by an explicitly
/// supplied in-scope path. No out-of-scope path is ever read.
pub fn load_plan_content_for_execution(
    agent_root: &Path,
    plan_id: Option<&str>,
    provided_plan_file_path: Option<&str>,
    inline_content: Option<&str>,
) -> Result<(String, Option<PathBuf>), String> {
    if let Some(content) = inline_content {
        if !content.trim().is_empty() {
            return Ok((content.to_string(), None));
        }
    }

    if let Some(raw_path) = provided_plan_file_path {
        if let Ok(path) = scoped_path(agent_root, raw_path) {
            if let Ok(content) = fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    return Ok((content, Some(path)));
                }
            }
        }
    }

    if let Some(plan_id) = plan_id.filter(|value| !value.trim().is_empty()) {
        if let Some(path) = registered_plan_path(agent_root, plan_id) {
            if let Ok(content) = fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    return Ok((content, Some(path)));
                }
            }
        }
    }

    Err(
        "plan content is unavailable; refusing to fabricate execute-plan success"
            .to_string(),
    )
}

pub fn upsert_plan_registry_entry(
    agent_root: &Path,
    plan_id: Option<&str>,
    plan_path: &Path,
    updated_at_ms: i64,
) -> Result<bool, String> {
    let Some(plan_id) = plan_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(false);
    };

    let plan_path = scoped_path(agent_root, &plan_path.to_string_lossy())?;
    let registry_path = plan_registry_path(agent_root);
    let mut registry = read_plan_registry(&registry_path)?;

    let entry = PlanRegistryEntry {
        id: plan_id.to_string(),
        path: plan_path.to_string_lossy().into_owned(),
        updated_at_ms,
    };

    if let Some(existing) = registry.entries.iter_mut().find(|item| item.id == entry.id) {
        *existing = entry;
    } else {
        registry.entries.push(entry);
    }

    registry.entries.sort_by(|left, right| left.id.cmp(&right.id));
    let bytes = serde_json::to_vec_pretty(&registry)
        .map_err(|error| format!("serialize plan registry: {error}"))?;
    atomic_write_bytes(&registry_path, &bytes)?;
    Ok(true)
}

fn registered_plan_path(agent_root: &Path, plan_id: &str) -> Option<PathBuf> {
    let registry_path = plan_registry_path(agent_root);
    let registry = read_plan_registry(&registry_path).ok()?;
    let entry = registry.entries.into_iter().find(|item| item.id == plan_id)?;
    let path = scoped_path(agent_root, &entry.path).ok()?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => Some(path),
        _ => None,
    }
}

fn read_plan_registry(path: &Path) -> Result<PlanRegistryFile, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PlanRegistryFile::default());
        }
        Err(error) => return Err(format!("read plan registry: {error}")),
    };

    let registry: PlanRegistryFile =
        serde_json::from_str(&content).map_err(|error| format!("decode plan registry: {error}"))?;
    if registry.version != PLAN_REGISTRY_VERSION {
        return Err(format!(
            "unsupported plan registry version {}",
            registry.version
        ));
    }
    Ok(registry)
}

fn sanitize_plan_file_name(
    provided_plan_file_path: Option<&str>,
    plan_id: Option<&str>,
) -> String {
    let source_name = provided_plan_file_path
        .and_then(last_path_component)
        .filter(|name| !name.is_empty())
        .or_else(|| plan_id.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or(DEFAULT_PLAN_FILE_NAME);

    let mut sanitized = String::with_capacity(source_name.len());
    for character in source_name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        DEFAULT_PLAN_FILE_NAME.to_string()
    } else if sanitized.ends_with(".md") {
        sanitized
    } else {
        format!("{sanitized}.md")
    }
}

fn last_path_component(path: &str) -> Option<&str> {
    path.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
}

fn validate_write_target(agent_root: &Path, target: &Path) -> Result<(), String> {
    let scoped = scoped_path(agent_root, &target.to_string_lossy())?;
    if let Ok(metadata) = fs::symlink_metadata(&scoped) {
        if metadata.file_type().is_symlink() {
            return Err("plan target is a symbolic link".to_string());
        }
        if !metadata.file_type().is_file() {
            return Err("plan target is not a regular file".to_string());
        }
    }
    Ok(())
}

fn scoped_path(root: &Path, raw_path: &str) -> Result<PathBuf, String> {
    if raw_path.trim().is_empty() {
        return Err("path is empty".to_string());
    }

    fs::create_dir_all(root).map_err(|error| format!("create scope root: {error}"))?;
    let canonical_root =
        fs::canonicalize(root).map_err(|error| format!("canonicalize scope root: {error}"))?;

    let raw = Path::new(raw_path);
    let candidate = if raw.is_absolute() {
        lexical_normalize(raw)?
    } else {
        lexical_normalize(&canonical_root.join(raw))?
    };

    if !candidate.starts_with(&canonical_root) {
        return Err("path escapes the account/agent scope".to_string());
    }

    // Validate every existing component, including symlinks, so an in-scope
    // lexical path cannot escape through a replaced directory or file.
    let relative = candidate
        .strip_prefix(&canonical_root)
        .map_err(|_| "path scope calculation failed".to_string())?;
    let mut current = canonical_root.clone();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current).is_ok() {
            let canonical_component = fs::canonicalize(&current)
                .map_err(|error| format!("canonicalize path component: {error}"))?;
            if !canonical_component.starts_with(&canonical_root) {
                return Err("path escapes the account/agent scope".to_string());
            }
        }
    }

    Ok(candidate)
}

fn lexical_normalize(path: &Path) -> Result<PathBuf, String> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err("path traversal escapes the scope".to_string());
                }
            }
            Component::RootDir | Component::Prefix(_) => result.push(component.as_os_str()),
            Component::Normal(value) => result.push(value),
        }
    }
    Ok(result)
}

fn nearest_existing_path(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if fs::symlink_metadata(current).is_ok() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn atomic_write_text(path: &Path, content: &str) -> Result<(), String> {
    atomic_write_bytes(path, content.as_bytes())
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "plan target has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("create plan directory: {error}"))?;

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err("refusing to write through a symbolic link".to_string());
        }
        if !metadata.file_type().is_file() {
            return Err("plan target is not a regular file".to_string());
        }
    }

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("plan.md");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{nonce}",
        Uuid::new_v4()
    ));

    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("create temporary plan file: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("write temporary plan file: {error}"))?;
        file.flush()
            .map_err(|error| format!("flush temporary plan file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync temporary plan file: {error}"))?;
        drop(file);

        fs::rename(&temporary, path).map_err(|error| {
            format!("publish plan file {}: {error}", path.display())
        })?;

        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeResourceAccessor {
        probes: RefCell<Vec<Result<bool, PlanFileResourceError>>>,
        writes: RefCell<Vec<Result<(), PlanFileResourceError>>>,
        written: RefCell<Vec<(PathBuf, String)>>,
    }

    impl FakeResourceAccessor {
        fn new(
            probes: Vec<Result<bool, PlanFileResourceError>>,
            writes: Vec<Result<(), PlanFileResourceError>>,
        ) -> Self {
            Self {
                probes: RefCell::new(probes),
                writes: RefCell::new(writes),
                written: RefCell::new(Vec::new()),
            }
        }
    }

    impl ExecutePlanPlanFileResourceAccessor for FakeResourceAccessor {
        fn probe_readable(&self, _path: &Path) -> Result<bool, PlanFileResourceError> {
            self.probes.borrow_mut().remove(0)
        }

        fn write_text(&self, path: &Path, content: &str) -> Result<(), PlanFileResourceError> {
            self.written
                .borrow_mut()
                .push((path.to_path_buf(), content.to_string()));
            self.writes.borrow_mut().remove(0)
        }
    }

    fn temp_scope() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-execute-plan-plan-file-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&path).expect("create test scope");
        path
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn path_traversal_is_reconstructed_only_inside_artifacts_plans() {
        let root = temp_scope();
        let artifacts = root.join("artifacts");
        let accessor = FakeResourceAccessor::new(
            vec![Ok(false)],
            vec![Ok(())],
        );

        let result = resolve_plan_file_path(
            &root,
            Some(&artifacts),
            Some("../../outside/evil plan.md"),
            Some("plan-1"),
            "# Safe plan",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::OnDisk);
        let path = result.path.expect("recreated path");
        assert!(path.starts_with(artifacts.join("plans")));
        assert_eq!(
            path.file_name().and_then(|value| value.to_str()),
            Some("evil_plan.md")
        );
        assert!(!root.join("outside/evil plan.md").exists());
        cleanup(&root);
    }

    #[test]
    fn illegal_name_is_sanitized_and_plan_id_is_used_when_name_missing() {
        assert_eq!(
            sanitize_plan_file_name(Some("C:\\unsafe folder\\plan name?.md"), Some("ignored")),
            "plan_name_.md"
        );
        assert_eq!(
            sanitize_plan_file_name(Some("../"), Some("plan:42")),
            "plan_42.md"
        );
        assert_eq!(
            sanitize_plan_file_name(None, None),
            DEFAULT_PLAN_FILE_NAME
        );
    }

    #[test]
    fn existing_file_is_kept_on_disk_without_recreation() {
        let root = temp_scope();
        let path = root.join("artifacts/plans/existing.md");
        fs::create_dir_all(path.parent().unwrap()).expect("create plans");
        fs::write(&path, "# existing").expect("write existing");

        let accessor = LocalPlanFileResourceAccessor::new(&root);
        let result = resolve_plan_file_path(
            &root,
            Some(&root.join("artifacts")),
            Some(path.to_str().unwrap()),
            Some("plan-existing"),
            "# replacement",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::OnDisk);
        assert!(!result.recreated);
        assert_eq!(fs::read_to_string(&path).unwrap(), "# existing");
        cleanup(&root);
    }

    #[test]
    fn missing_file_is_recreated_atomically() {
        let root = temp_scope();
        let accessor = LocalPlanFileResourceAccessor::new(&root);
        let result = resolve_plan_file_path(
            &root,
            Some(&root.join("artifacts")),
            Some("/not/inside/sandbox/missing.md"),
            Some("plan-recreated"),
            "# recreated",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::OnDisk);
        assert!(result.recreated);
        let path = result.path.expect("recreated path");
        assert_eq!(fs::read_to_string(path).unwrap(), "# recreated");
        cleanup(&root);
    }

    #[test]
    fn missing_artifacts_folder_returns_unknown_without_writing() {
        let root = temp_scope();
        let accessor = FakeResourceAccessor::new(vec![], vec![]);
        let result = resolve_plan_file_path(
            &root,
            None,
            Some("missing.md"),
            Some("plan-unknown"),
            "# plan",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::Unknown);
        assert!(!result.should_upsert_plan_registry_entry);
        assert!(accessor.written.borrow().is_empty());
        cleanup(&root);
    }

    #[test]
    fn read_failure_falls_back_to_reconstruction() {
        let root = temp_scope();
        let accessor = FakeResourceAccessor::new(
            vec![Err(PlanFileResourceError::PermissionDenied(
                "denied".into(),
            ))],
            vec![Ok(())],
        );

        let result = resolve_plan_file_path(
            &root,
            Some(&root.join("artifacts")),
            Some("unreadable.md"),
            Some("plan-read-failure"),
            "# recovered",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::OnDisk);
        assert!(result.recreated);
        assert_eq!(accessor.written.borrow().len(), 1);
        cleanup(&root);
    }

    #[test]
    fn write_failure_is_unavailable_and_never_claims_registry_update() {
        let root = temp_scope();
        let accessor = FakeResourceAccessor::new(
            vec![Ok(false)],
            vec![Err(PlanFileResourceError::Io("disk full".into()))],
        );

        let result = resolve_plan_file_path(
            &root,
            Some(&root.join("artifacts")),
            Some("missing.md"),
            Some("plan-write-failure"),
            "# plan",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::Unavailable);
        assert!(!result.should_upsert_plan_registry_entry);
        cleanup(&root);
    }

    #[test]
    fn restart_recovers_plan_from_durable_registry() {
        let root = temp_scope();
        let plan_path = root.join("artifacts/plans/restart.md");
        fs::create_dir_all(plan_path.parent().unwrap()).expect("create plans");
        fs::write(&plan_path, "# durable").expect("write plan");
        assert!(
            upsert_plan_registry_entry(
                &root,
                Some("restart-plan"),
                &plan_path,
                1234,
            )
            .expect("update registry")
        );

        let recovered = load_plan_content_for_execution(
            &root,
            Some("restart-plan"),
            None,
            None,
        )
        .expect("recover plan");
        assert_eq!(recovered.0, "# durable");
        assert_eq!(recovered.1, Some(plan_path));
        cleanup(&root);
    }

    #[test]
    fn cross_account_roots_are_isolated() {
        let account_a = temp_scope();
        let account_b = temp_scope();
        let plan_a = account_a.join("artifacts/plans/same-id.md");
        let plan_b = account_b.join("artifacts/plans/same-id.md");
        fs::create_dir_all(plan_a.parent().unwrap()).expect("create account a");
        fs::create_dir_all(plan_b.parent().unwrap()).expect("create account b");
        fs::write(&plan_a, "# account a").expect("write account a");
        fs::write(&plan_b, "# account b").expect("write account b");

        upsert_plan_registry_entry(&account_a, Some("same-id"), &plan_a, 1)
            .expect("registry a");
        upsert_plan_registry_entry(&account_b, Some("same-id"), &plan_b, 2)
            .expect("registry b");

        let recovered_a =
            load_plan_content_for_execution(&account_a, Some("same-id"), None, None)
                .expect("recover a");
        let recovered_b =
            load_plan_content_for_execution(&account_b, Some("same-id"), None, None)
                .expect("recover b");
        assert_eq!(recovered_a.0, "# account a");
        assert_eq!(recovered_b.0, "# account b");
        cleanup(&account_a);
        cleanup(&account_b);
    }

    #[test]
    fn cancellation_style_no_write_path_does_not_claim_success() {
        let root = temp_scope();
        let accessor = FakeResourceAccessor::new(
            vec![Ok(false)],
            vec![Err(PlanFileResourceError::Io("cancelled before write".into()))],
        );

        let result = resolve_plan_file_path(
            &root,
            Some(&root.join("artifacts")),
            Some("cancelled.md"),
            Some("cancelled"),
            "# plan",
            &accessor,
        );

        assert_eq!(result.availability, PlanFileAvailability::Unavailable);
        assert!(!result.should_upsert_plan_registry_entry);
        cleanup(&root);
    }
}
