use std::{collections::BTreeMap, env, path::{Path, PathBuf}};

use crate::{host_paths::get_sand_root_dir_with, storage::folder_id::is_safe_folder_id};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid Sand agent id: {0}")]
pub struct SandInvalidAgentIdError(pub String);

pub fn get_sand_agents_root_dir(home_dir: Option<&Path>) -> PathBuf {
    let home = home_dir.map(Path::to_path_buf).unwrap_or_else(|| env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")));
    let env_map = env::vars().collect::<BTreeMap<_, _>>();
    let argv = env::args().collect::<Vec<_>>();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    get_sand_root_dir_with(&home, &argv, &env_map, &cwd).join("agents")
}

pub fn assert_valid_sand_agent_id(agent_id: &str) -> Result<(), SandInvalidAgentIdError> {
    if !is_safe_folder_id(agent_id) || agent_id.trim() != agent_id {
        return Err(SandInvalidAgentIdError(agent_id.to_owned()));
    }
    Ok(())
}

pub fn resolve_sand_agent_dir(agent_id: &str, home_dir: Option<&Path>) -> Result<PathBuf, SandInvalidAgentIdError> {
    assert_valid_sand_agent_id(agent_id)?;
    let root = get_sand_agents_root_dir(home_dir);
    let target = root.join(agent_id);
    let relative = target.strip_prefix(&root).map_err(|_| SandInvalidAgentIdError(agent_id.to_owned()))?;
    if relative.as_os_str().is_empty() || relative.components().count() != 1 {
        return Err(SandInvalidAgentIdError(agent_id.to_owned()));
    }
    Ok(target)
}
