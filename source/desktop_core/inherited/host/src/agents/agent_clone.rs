use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::agents::agent_avatar::{
    CANONICAL_AVATAR_FILENAME, list_conventional_avatar_filenames,
    resolve_derived_avatar_filename,
};
use crate::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, read_legacy_profile_avatar_field,
    read_sand_profile_file, write_sand_profile_file,
};
use crate::agents::agent_workflow_enablement::get_agent_workflow_enablement_path;
use crate::agents::settings_file::{get_sand_settings_path, write_sand_settings_file};
use crate::extensions::session::agent_db::{
    SandAgentDb,
};
use crate::extensions::session::agent_db_serde::SandProfile;
use crate::storage::store_db::checkpoint_sand_agent_db;

pub const STORE_FILENAME: &str = "store.db";
pub const AUTOMATIONS_DIRNAME: &str = "automations";
pub const AUTOMATION_CONFIG_FILENAME: &str = "automation.json";

#[derive(Debug, thiserror::Error)]
pub enum SandAgentCloneError {
    #[error("This agent's data is missing and can't be duplicated.")]
    MissingStore,
    #[error("could not checkpoint source agent store before duplication")]
    Checkpoint,
    #[error("agent clone filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("agent clone database rewrite failed: {0}")]
    Database(String),
}

pub fn clone_agent_display_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "copy".to_string()
    } else {
        format!("{trimmed} copy")
    }
}

pub fn get_agent_automations_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join(AUTOMATIONS_DIRNAME)
}

pub fn list_agent_automation_config_files(
    automations_dir: &Path,
) -> Vec<(String, PathBuf)> {
    fs::read_dir(automations_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry)
        })
        .filter_map(|entry| {
            let folder_name = entry.file_name().into_string().ok()?;
            let config_path = entry.path().join(AUTOMATION_CONFIG_FILENAME);
            config_path.is_file().then_some((folder_name, config_path))
        })
        .collect()
}

fn copy_if_present(source: &Path, target: &Path) -> io::Result<()> {
    if source.exists() {
        fs::copy(source, target)?;
    }
    Ok(())
}

fn rewrite_identity(
    target_dir: &Path,
    new_agent_id: &str,
    busy_timeout_ms: u64,
) -> Result<(), SandAgentCloneError> {
    let db_path = target_dir.join(STORE_FILENAME);
    let owner = SandAgentDb::open(&db_path, busy_timeout_ms)
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    let profile = owner
        .get_sand_profile()
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;

    owner
        .set_metadata(
            "agentId",
            Value::String(new_agent_id.to_string()),
        )
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .reset_session_work_admission_for_clone(new_agent_id)
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .clear_clone_mutation_journals()
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .set_agent_origin("user")
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .clear_agent_purpose()
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .clear_transient_state()
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    owner
        .clear_conversation()
        .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;

    let avatar_path = target_dir
        .join(CANONICAL_AVATAR_FILENAME)
        .is_file()
        .then(|| {
            target_dir
                .join(CANONICAL_AVATAR_FILENAME)
                .to_string_lossy()
                .into_owned()
        });
    owner
        .set_sand_profile(&SandProfile {
            description: profile.description,
            avatar_path,
        })
    .map_err(|error| SandAgentCloneError::Database(error.to_string()))?;
    Ok(())
}

pub fn clone_agent_dir(
    source_dir: &Path,
    target_dir: &Path,
    new_agent_id: &str,
    clone_name: &str,
    busy_timeout_ms: u64,
) -> Result<(), SandAgentCloneError> {
    fs::create_dir_all(target_dir)?;
    let result = (|| {
        let source_store = source_dir.join(STORE_FILENAME);
        if !source_store.is_file() {
            return Err(SandAgentCloneError::MissingStore);
        }
        if !checkpoint_sand_agent_db(&source_store, busy_timeout_ms) {
            return Err(SandAgentCloneError::Checkpoint);
        }
        fs::copy(&source_store, target_dir.join(STORE_FILENAME))?;

        let source_profile_path = get_sand_profile_path(source_dir);
        let profile = read_sand_profile_file(&source_profile_path);
        write_sand_profile_file(
            get_sand_profile_path(target_dir),
            &SandAgentProfile {
                name: clone_name.to_string(),
                description: profile
                    .as_ref()
                    .map(|profile| profile.description.clone())
                    .unwrap_or_default(),
                title: profile
                    .as_ref()
                    .map(|profile| profile.title.clone())
                    .unwrap_or_default(),
                avatar_shape: profile
                    .as_ref()
                    .map(|profile| profile.avatar_shape.clone())
                    .unwrap_or_default(),
                avatar_color: profile
                    .as_ref()
                    .map(|profile| profile.avatar_color.clone())
                    .unwrap_or_default(),
            },
        )?;

        copy_if_present(
            &get_sand_settings_path(source_dir),
            &get_sand_settings_path(target_dir),
        )?;
        let mut settings = Map::new();
        settings.insert("hiddenFromSidebar".into(), Value::Bool(false));
        write_sand_settings_file(get_sand_settings_path(target_dir), &settings)?;

        copy_if_present(
            &get_agent_workflow_enablement_path(source_dir),
            &get_agent_workflow_enablement_path(target_dir),
        )?;

        let legacy_avatar =
            read_legacy_profile_avatar_field(&source_profile_path);
        let _ = resolve_derived_avatar_filename(
            source_dir,
            legacy_avatar.as_deref(),
        );
        for name in list_conventional_avatar_filenames(source_dir) {
            fs::copy(source_dir.join(&name), target_dir.join(&name))?;
        }

        for (folder_name, config_path) in
            list_agent_automation_config_files(&get_agent_automations_dir(source_dir))
        {
            let destination =
                get_agent_automations_dir(target_dir).join(folder_name);
            fs::create_dir_all(&destination)?;
            fs::copy(
                config_path,
                destination.join(AUTOMATION_CONFIG_FILENAME),
            )?;
        }

        rewrite_identity(target_dir, new_agent_id, busy_timeout_ms)
    })();

    if result.is_err() {
        let _ = fs::remove_dir_all(target_dir);
    }
    result
}
