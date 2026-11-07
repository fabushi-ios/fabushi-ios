use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::durable_file_policy::SAND_UPGRADE_RESUME_FILE_NAME;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeResumeMarker {
    pub agent_id: String,
    pub marked_at_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_run_id: Option<String>,
}

pub fn coerce_upgrade_resume_marker(entry: &Value) -> Option<UpgradeResumeMarker> {
    let object = entry.as_object()?;
    let agent_id = object
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    Some(UpgradeResumeMarker {
        agent_id: agent_id.to_string(),
        marked_at_ms: object
            .get("markedAtMs")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or(0.0),
        source: object.get("source").and_then(Value::as_str).map(ToOwned::to_owned),
        automation_id: object
            .get("automationId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        automation_run_id: object
            .get("automationRunId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

pub fn parse_upgrade_resume_file(raw: Option<&str>) -> Vec<UpgradeResumeMarker> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    value
        .get("pending")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(coerce_upgrade_resume_marker).collect())
        .unwrap_or_default()
}

pub fn upgrade_resume_marker_identity_is_well_formed(marker: &UpgradeResumeMarker) -> bool {
    if marker.agent_id.trim().is_empty()
        || marker
            .automation_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || marker
            .automation_run_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
    {
        return false;
    }
    marker.automation_run_id.is_none() || marker.automation_id.is_some()
}

pub fn upsert_resume_marker(
    existing: &[UpgradeResumeMarker],
    marker: UpgradeResumeMarker,
) -> Vec<UpgradeResumeMarker> {
    let mut next = existing
        .iter()
        .filter(|entry| entry.agent_id != marker.agent_id)
        .cloned()
        .collect::<Vec<_>>();
    next.push(marker);
    next
}

#[derive(Clone)]
pub struct SandUpgradeResumeStore {
    file_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl SandUpgradeResumeStore {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            file_path: root_dir.as_ref().join(SAND_UPGRADE_RESUME_FILE_NAME),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn mark_pending(&self, marker: UpgradeResumeMarker) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending = upsert_resume_marker(&self.read_pending_unlocked(), marker);
        let _ = self.write_unlocked(&pending);
    }

    pub fn list_pending(&self) -> Vec<UpgradeResumeMarker> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.read_pending_unlocked()
    }

    pub fn clear(&self, agent_id: &str) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let remaining = self
            .read_pending_unlocked()
            .into_iter()
            .filter(|entry| entry.agent_id != agent_id)
            .collect::<Vec<_>>();
        if remaining.is_empty() {
            let _ = self.delete_unlocked();
        } else {
            let _ = self.write_unlocked(&remaining);
        }
    }

    /// Remove only the marker that was observed by a recovery pass.
    ///
    /// A recovery pass may race with a new marker for the same agent.  An
    /// agent-only clear would delete that newer work, so the whole marker is
    /// compared while holding the store lock and the new marker is retained.
    pub fn clear_if_matches(&self, marker: &UpgradeResumeMarker) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending = self.read_pending_unlocked();
        let remaining = pending
            .iter()
            .filter(|entry| *entry != marker)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == pending.len() {
            return false;
        }
        if remaining.is_empty() {
            self.delete_unlocked().is_ok()
        } else {
            self.write_unlocked(&remaining).is_ok()
        }
    }

    pub fn clear_all(&self) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = self.delete_unlocked();
    }

    fn read_pending_unlocked(&self) -> Vec<UpgradeResumeMarker> {
        fs::read_to_string(&self.file_path)
            .ok()
            .map(|raw| parse_upgrade_resume_file(Some(&raw)))
            .unwrap_or_default()
    }

    fn write_unlocked(&self, pending: &[UpgradeResumeMarker]) -> std::io::Result<()> {
        #[derive(Serialize)]
        struct File<'a> {
            version: u8,
            pending: &'a [UpgradeResumeMarker],
        }
        if let Some(parent) = self.file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = PathBuf::from(format!("{}.part", self.file_path.display()));
        fs::write(
            &part,
            serde_json::to_vec(&File {
                version: 1,
                pending,
            })
            .map_err(std::io::Error::other)?,
        )?;
        replace_file(&part, &self.file_path)
    }

    fn delete_unlocked(&self) -> std::io::Result<()> {
        match fs::remove_file(&self.file_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn replace_file(part: &Path, target: &Path) -> std::io::Result<()> {
    match fs::rename(part, target) {
        Ok(()) => Ok(()),
        Err(first) if target.exists() => {
            fs::remove_file(target)?;
            fs::rename(part, target).map_err(|_| first)
        }
        Err(error) => Err(error),
    }
}
