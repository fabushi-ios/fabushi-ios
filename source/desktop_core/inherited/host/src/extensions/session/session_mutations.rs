use std::fs;
use std::path::Path;

use crate::agents::agent_avatar::{
    CANONICAL_AVATAR_FILENAME, CONVENTIONAL_AVATAR_EXTENSIONS,
    invalidate_avatar_data_url_cache,
    list_conventional_avatar_filenames,
};

use super::agent_db::read_persisted_agent_serde_snapshot;
use super::agent_db::ProfileFileSnapshot;
use super::agent_db_serde::SandProfile;
use super::session_summaries::{
    AgentSummary, DurableFootprint, agent_has_durable_footprint, build_summary,
    file_mtime_ms,
};

fn allowed_profile_mutation_filename(filename: &str) -> bool {
    filename == "profile.json"
        || CONVENTIONAL_AVATAR_EXTENSIONS
            .iter()
            .any(|extension| filename == format!("avatar.{extension}"))
}

pub fn capture_profile_mutation_files(
    agent_dir: &Path,
    include_profile: bool,
) -> Result<Vec<ProfileFileSnapshot>, String> {
    let mut filenames = if include_profile {
        vec!["profile.json".to_string()]
    } else {
        Vec::new()
    };
    filenames.extend(
        CONVENTIONAL_AVATAR_EXTENSIONS
            .iter()
            .map(|extension| format!("avatar.{extension}")),
    );
    filenames
        .into_iter()
        .map(|filename| {
            let path = agent_dir.join(&filename);
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err("profile mutation path is not a regular file".into());
                }
            }
            let contents = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            Ok(ProfileFileSnapshot { filename, contents })
        })
        .collect()
}

pub fn profile_mutation_files_match(
    agent_dir: &Path,
    expected: &[ProfileFileSnapshot],
) -> bool {
    if expected
        .iter()
        .any(|snapshot| !allowed_profile_mutation_filename(&snapshot.filename))
    {
        return false;
    }
    expected.iter().all(|snapshot| {
        let path = agent_dir.join(&snapshot.filename);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return false;
            }
        }
        let current = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return false,
        };
        current == snapshot.contents
    })
}

pub fn restore_profile_mutation_files(
    agent_dir: &Path,
    snapshots: &[ProfileFileSnapshot],
) -> Result<(), String> {
    for snapshot in snapshots {
        if !allowed_profile_mutation_filename(&snapshot.filename) {
            return Err("profile mutation journal contains a non-allowlisted file".into());
        }
        let path = agent_dir.join(&snapshot.filename);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("profile mutation restore path is not a regular file".into());
            }
        }
        match snapshot.contents.as_deref() {
            Some(contents) => fs::write(path, contents).map_err(|error| error.to_string())?,
            None => match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            },
        }
    }
    invalidate_avatar_data_url_cache(agent_dir);
    Ok(())
}

pub fn set_agent_avatar_bytes<F>(
    db_path: &Path,
    busy_timeout_ms: u64,
    png_bytes: Option<&[u8]>,
    set_profile: F,
) -> Result<(), String>
where
    F: FnOnce(&SandProfile) -> Result<(), String>,
{
    let agent_dir = db_path
        .parent()
        .ok_or_else(|| "agent database has no parent directory".to_string())?;
    let prior_state = if png_bytes.is_none() {
        Some(
            read_persisted_agent_serde_snapshot(db_path, busy_timeout_ms)
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let previous_files = list_conventional_avatar_filenames(agent_dir)
        .into_iter()
        .filter_map(|name| {
            let path = agent_dir.join(name);
            fs::read(&path).ok().map(|bytes| (path, bytes))
        })
        .collect::<Vec<_>>();
    let restore_previous = || -> Result<(), String> {
        for (path, bytes) in &previous_files {
            fs::write(path, bytes).map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    for name in list_conventional_avatar_filenames(agent_dir) {
        let _ = fs::remove_file(agent_dir.join(name));
    }
    match png_bytes {
        None => {
            let state = prior_state
                .as_ref()
                .expect("prior avatar state is captured for avatar removal");
            if state.profile.avatar_path.is_some() {
                if let Err(error) = set_profile(&SandProfile {
                    description: state.profile.description.clone(),
                    avatar_path: None,
                }) {
                    let restore_error = restore_previous().err();
                    return Err(match restore_error {
                        Some(restore_error) => {
                            format!("{error}; avatar rollback failed: {restore_error}")
                        }
                        None => error,
                    });
                }
            }
        }
        Some(bytes) => {
            if let Err(error) = fs::create_dir_all(agent_dir) {
                let restore_error = restore_previous().err();
                return Err(match restore_error {
                    Some(restore_error) => {
                        format!("{error}; avatar rollback failed: {restore_error}")
                    }
                    None => error.to_string(),
                });
            }
            if let Err(error) = fs::write(agent_dir.join(CANONICAL_AVATAR_FILENAME), bytes) {
                let restore_error = restore_previous().err();
                return Err(match restore_error {
                    Some(restore_error) => {
                        format!("{error}; avatar rollback failed: {restore_error}")
                    }
                    None => error.to_string(),
                });
            }
        }
    }
    invalidate_avatar_data_url_cache(agent_dir);
    Ok(())
}

pub fn recover_agent_with_missing_db<F, D>(
    db_path: &Path,
    dir_name: &str,
    active_agent_id: Option<&str>,
    footprint: DurableFootprint,
    has_live_db_handle: bool,
    mut is_agent_being_deleted: D,
    mut reseed_minimal_store_db_if_missing: F,
) -> Result<Option<AgentSummary>, String>
where
    F: FnMut(&Path) -> Result<(), String>,
    D: FnMut() -> bool,
{
    if is_agent_being_deleted() {
        return Ok(None);
    }
    let agent_dir = db_path
        .parent()
        .ok_or_else(|| "agent database has no parent directory".to_string())?;
    let intact = agent_dir.join("profile.json").is_file()
        || agent_has_durable_footprint(agent_dir, footprint);
    if !intact || is_agent_being_deleted() {
        return Ok(None);
    }
    let mtime_ms = if has_live_db_handle {
        None
    } else {
        reseed_minimal_store_db_if_missing(db_path)?;
        file_mtime_ms(db_path)
    };
    build_summary(
        None,
        db_path,
        dir_name,
        mtime_ms,
        active_agent_id,
        true,
        footprint,
    )
}
