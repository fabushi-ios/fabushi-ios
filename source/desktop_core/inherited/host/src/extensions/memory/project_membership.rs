use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::storage::folder_id::is_safe_folder_id;

pub const MEMBERSHIP_FILENAME: &str = "projects.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProjectMembership {
    agent_dir: PathBuf,
}

impl AgentProjectMembership {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        get_agent_projects_path(&self.agent_dir)
    }

    pub fn read(&self) -> BTreeSet<String> {
        let Ok(raw) = fs::read_to_string(self.path()) else {
            return BTreeSet::new();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return BTreeSet::new();
        };
        to_safe_slug_set(value.get("projects"))
    }

    pub fn write(&self, slugs: &BTreeSet<String>) -> io::Result<()> {
        fs::create_dir_all(&self.agent_dir)?;
        let body = serde_json::to_string_pretty(&json!({
            "projects": slugs.iter().cloned().collect::<Vec<_>>()
        }))
        .map_err(io::Error::other)?;
        write_atomic(&self.path(), format!("{body}\n").as_bytes())
    }

    pub fn join(&self, slug: &str) -> io::Result<bool> {
        if !is_safe_folder_id(slug) {
            return Ok(false);
        }
        let mut slugs = self.read();
        if slugs.insert(slug.to_string()) {
            self.write(&slugs)?;
        }
        Ok(true)
    }

    pub fn leave(&self, slug: &str) -> io::Result<bool> {
        if !is_safe_folder_id(slug) {
            return Ok(false);
        }
        let mut slugs = self.read();
        if slugs.remove(slug) {
            self.write(&slugs)?;
        }
        Ok(true)
    }

    pub fn prune_missing(
        &self,
        mut project_exists: impl FnMut(&str) -> bool,
    ) -> io::Result<bool> {
        let mut slugs = self.read();
        let before = slugs.len();
        slugs.retain(|slug| project_exists(slug));
        if slugs.len() == before {
            return Ok(false);
        }
        self.write(&slugs)?;
        Ok(true)
    }
}

pub fn get_agent_projects_path(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(MEMBERSHIP_FILENAME)
}

pub fn to_safe_slug_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|slug| is_safe_folder_id(slug))
        .map(ToOwned::to_owned)
        .collect()
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!(
        "{}.{}.tmp",
        path.display(),
        std::process::id()
    ));
    fs::write(&temporary, contents)?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(first) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&temporary, path).map_err(|_| first)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}
