use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const PENDING_WAKE_STALE_MAX_AGE_MS: f64 = 48.0 * 60.0 * 60.0 * 1_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsyncTaskKind {
    CloudAgent,
    Subagent,
    Shell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWakeMarker {
    pub agent_id: String,
    pub kind: AsyncTaskKind,
    pub work_id: String,
    pub marked_at_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub interrupted_by_restart: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncTask {
    pub kind: AsyncTaskKind,
    pub id: String,
    pub label: String,
    pub status: String,
    pub started_at_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingWakeFile {
    version: u8,
    #[serde(default)]
    pending: Vec<PendingWakeMarker>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn valid_marker(marker: &PendingWakeMarker) -> bool {
    !marker.agent_id.trim().is_empty()
        && !marker.work_id.trim().is_empty()
        && marker.marked_at_ms.is_finite()
        && marker.marked_at_ms >= 0.0
}

fn same_identity(marker: &PendingWakeMarker, agent_id: &str, kind: AsyncTaskKind, work_id: &str) -> bool {
    marker.agent_id == agent_id && marker.kind == kind && marker.work_id == work_id
}

fn marker_label(marker: &PendingWakeMarker) -> String {
    if let Some(title) = marker.title.as_deref().filter(|value| !value.trim().is_empty()) {
        return title.to_string();
    }
    match marker.kind {
        AsyncTaskKind::CloudAgent => format!("Cloud agent {}", marker.work_id),
        AsyncTaskKind::Shell => format!("Background command {}", marker.work_id),
        AsyncTaskKind::Subagent => format!("Background task {}", marker.work_id),
    }
}

fn marker_to_task(marker: &PendingWakeMarker) -> AsyncTask {
    AsyncTask {
        kind: marker.kind,
        id: marker.work_id.clone(),
        label: marker_label(marker),
        status: "running".to_string(),
        started_at_ms: marker.marked_at_ms,
        detail: if marker.interrupted_by_restart {
            Some("rearmed after a Host restart".to_string())
        } else if marker.kind == AsyncTaskKind::Subagent {
            marker.subagent_type.clone()
        } else {
            None
        },
        subagent_type: (marker.kind == AsyncTaskKind::Subagent)
            .then(|| marker.subagent_type.clone())
            .flatten(),
    }
}

pub fn merge_async_tasks(live: &[AsyncTask], markers: &[PendingWakeMarker]) -> Vec<AsyncTask> {
    let mut seen = live
        .iter()
        .map(|task| (task.kind, task.id.clone()))
        .collect::<HashSet<_>>();
    let mut merged = live.to_vec();
    for marker in markers {
        let key = (marker.kind, marker.work_id.clone());
        if !seen.insert(key.clone()) {
            if marker.interrupted_by_restart {
                if let Some(task) = merged
                    .iter_mut()
                    .find(|task| task.kind == key.0 && task.id == key.1)
                {
                    task.detail = Some("rearmed after a Host restart".to_string());
                }
            }
            continue;
        }
        merged.push(marker_to_task(marker));
    }
    merged.sort_by(|left, right| {
        left.started_at_ms
            .partial_cmp(&right.started_at_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    merged
}

pub struct AsyncTasksRuntime {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AsyncTasksRuntime {
    pub fn new(app_data_dir: impl AsRef<Path>) -> Self {
        Self {
            path: app_data_dir.as_ref().join("pending-wakes.json"),
            lock: Mutex::new(()),
        }
    }

    pub fn mark_pending(&self, marker: PendingWakeMarker) -> Result<(), String> {
        if !valid_marker(&marker) {
            return Err("invalid pending wake marker".into());
        }
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending = self.read_unlocked();
        pending.retain(|existing| {
            !same_identity(existing, &marker.agent_id, marker.kind, &marker.work_id)
        });
        pending.push(marker);
        self.write_unlocked(&pending)
    }

    pub fn settle(&self, agent_id: &str, kind: AsyncTaskKind, work_id: &str) -> Result<bool, String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending = self.read_unlocked();
        let before = pending.len();
        pending.retain(|entry| !same_identity(entry, agent_id, kind, work_id));
        if pending.len() == before {
            return Ok(false);
        }
        self.write_unlocked(&pending)?;
        Ok(true)
    }

    pub fn rearm_after_restart(&self, now_ms: f64) -> Result<Vec<PendingWakeMarker>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending = self.read_unlocked();
        pending.retain(|marker| now_ms - marker.marked_at_ms <= PENDING_WAKE_STALE_MAX_AGE_MS);
        for marker in &mut pending {
            marker.interrupted_by_restart = true;
        }
        self.write_unlocked(&pending)?;
        Ok(pending)
    }

    pub fn snapshot(&self, agent_id: &str, live: &[AsyncTask], now_ms: f64) -> Result<Vec<AsyncTask>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending = self.read_unlocked();
        let before = pending.len();
        pending.retain(|marker| now_ms - marker.marked_at_ms <= PENDING_WAKE_STALE_MAX_AGE_MS);
        if pending.len() != before {
            self.write_unlocked(&pending)?;
        }
        let markers = pending
            .into_iter()
            .filter(|marker| marker.agent_id == agent_id)
            .collect::<Vec<_>>();
        Ok(merge_async_tasks(live, &markers))
    }

    pub fn handle_rpc(&self, method: &str, params: &Value, now_ms: f64) -> Option<Result<Value, String>> {
        match method {
            "getAsyncTasks" => {
                let agent_id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| "getAsyncTasks requires id".to_string());
                Some(agent_id.and_then(|agent_id| {
                    self.snapshot(agent_id, &[], now_ms)
                        .and_then(|tasks| serde_json::to_value(tasks).map_err(|error| error.to_string()))
                }))
            }
            _ => None,
        }
    }

    fn read_unlocked(&self) -> Vec<PendingWakeMarker> {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PendingWakeFile>(&raw).ok())
            .map(|file| file.pending.into_iter().filter(valid_marker).collect())
            .unwrap_or_default()
    }

    fn write_unlocked(&self, pending: &[PendingWakeMarker]) -> Result<(), String> {
        if pending.is_empty() {
            match fs::remove_file(&self.path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let part = PathBuf::from(format!("{}.part", self.path.display()));
        let bytes = serde_json::to_vec(&PendingWakeFile {
            version: 1,
            pending: pending.to_vec(),
        })
        .map_err(|error| error.to_string())?;
        fs::write(&part, bytes).map_err(|error| error.to_string())?;
        if self.path.exists() {
            fs::remove_file(&self.path).map_err(|error| error.to_string())?;
        }
        fs::rename(part, &self.path).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-ios-async-tasks-{name}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ))
    }

    fn marker(agent: &str, kind: AsyncTaskKind, work: &str, started: f64) -> PendingWakeMarker {
        PendingWakeMarker {
            agent_id: agent.into(),
            kind,
            work_id: work.into(),
            marked_at_ms: started,
            title: None,
            subagent_type: None,
            interrupted_by_restart: false,
        }
    }

    #[test]
    fn durable_identity_upserts_and_settlement_clears_exact_marker() {
        let root = temp_root("settle");
        let runtime = AsyncTasksRuntime::new(&root);
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::Shell, "work-1", 10.0)).unwrap();
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::Shell, "work-1", 20.0)).unwrap();
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::Subagent, "work-1", 30.0)).unwrap();

        let tasks = runtime.snapshot("agent-a", &[], 40.0).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].started_at_ms, 20.0);
        assert_eq!(tasks[1].started_at_ms, 30.0);

        assert!(runtime.settle("agent-a", AsyncTaskKind::Shell, "work-1").unwrap());
        assert_eq!(runtime.snapshot("agent-a", &[], 40.0).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restart_rearm_prunes_stale_and_projects_restart_detail_without_duplicates() {
        let root = temp_root("restart");
        let runtime = AsyncTasksRuntime::new(&root);
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::Shell, "shell-1", 100.0)).unwrap();
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::CloudAgent, "cloud-1", 101.0)).unwrap();
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::Subagent, "stale", 0.0)).unwrap();

        let now = PENDING_WAKE_STALE_MAX_AGE_MS + 1.0;
        let carried = runtime.rearm_after_restart(now).unwrap();
        assert_eq!(carried.len(), 2);

        let live = vec![AsyncTask {
            kind: AsyncTaskKind::Shell,
            id: "shell-1".into(),
            label: "live shell".into(),
            status: "running".into(),
            started_at_ms: 100.0,
            detail: None,
            subagent_type: None,
        }];
        let tasks = runtime.snapshot("agent-a", &live, now).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "shell-1");
        assert_eq!(tasks[0].detail.as_deref(), Some("rearmed after a Host restart"));
        assert_eq!(tasks[1].id, "cloud-1");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rpc_projection_is_agent_scoped_and_fail_closed() {
        let root = temp_root("rpc");
        let runtime = AsyncTasksRuntime::new(&root);
        runtime.mark_pending(marker("agent-a", AsyncTaskKind::CloudAgent, "cloud-1", 10.0)).unwrap();
        runtime.mark_pending(marker("agent-b", AsyncTaskKind::Shell, "shell-1", 11.0)).unwrap();

        let value = runtime
            .handle_rpc("getAsyncTasks", &json!({"id":"agent-a"}), 20.0)
            .expect("handled")
            .expect("valid rpc");
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["id"], "cloud-1");
        assert!(runtime
            .handle_rpc("getAsyncTasks", &json!({}), 20.0)
            .expect("handled")
            .is_err());
        let _ = fs::remove_dir_all(root);
    }
}
