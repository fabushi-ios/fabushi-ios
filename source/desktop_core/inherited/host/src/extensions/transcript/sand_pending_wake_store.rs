use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;

use crate::durable_file_policy::SAND_PENDING_WAKE_FILE_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingWakeKind {
    CloudAgent,
    Subagent,
    Shell,
    ChannelDelivery,
}

impl PendingWakeKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cloud-agent" => Some(Self::CloudAgent),
            "subagent" => Some(Self::Subagent),
            "shell" => Some(Self::Shell),
            "channel-delivery" => Some(Self::ChannelDelivery),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationQuietOrigin {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuietWakeOrigin {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation: Option<AutomationQuietOrigin>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DurablePendingWakeMarker {
    pub agent_id: String,
    pub kind: PendingWakeKind,
    pub work_id: String,
    pub marked_at_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet_origin: Option<QuietWakeOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub interrupted_by_recreate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn coerce_quiet_origin(value: &Value) -> Option<QuietWakeOrigin> {
    let object = value.as_object()?;
    let automation = object
        .get("automation")
        .and_then(Value::as_object)
        .and_then(|automation| {
            let id = automation.get("id").and_then(Value::as_str)?;
            let name = automation.get("name").and_then(Value::as_str)?;
            (!id.is_empty()).then(|| AutomationQuietOrigin {
                id: id.to_string(),
                name: name.to_string(),
            })
        });
    Some(QuietWakeOrigin { automation })
}

pub fn coerce_marker(entry: &Value) -> Option<DurablePendingWakeMarker> {
    let object = entry.as_object()?;
    let agent_id = object
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let work_id = object
        .get("workId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())?;
    let kind = PendingWakeKind::parse(object.get("kind")?.as_str()?)?;
    let marked_at_ms = object
        .get("markedAtMs")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0);
    let quiet_origin = object.get("quietOrigin").and_then(coerce_quiet_origin);
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let subagent_type = object
        .get("subagentType")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    Some(DurablePendingWakeMarker {
        agent_id: agent_id.to_string(),
        kind,
        work_id: work_id.to_string(),
        marked_at_ms,
        quiet_origin,
        title,
        subagent_type,
        interrupted_by_recreate: object
            .get("interruptedByRecreate")
            .and_then(Value::as_bool)
            == Some(true),
        delivery_operation_id: object
            .get("deliveryOperationId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        address_token: object
            .get("addressToken")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        failure_reason: object
            .get("failureReason")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        session_key: object
            .get("sessionKey")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    })
}

pub fn coerce_pending_wake_markers(value: &Value) -> Vec<DurablePendingWakeMarker> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(coerce_marker).collect())
        .unwrap_or_default()
}

pub fn parse_pending_wake_file(raw: Option<&str>) -> Vec<DurablePendingWakeMarker> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    value
        .get("pending")
        .map(coerce_pending_wake_markers)
        .unwrap_or_default()
}

pub fn marker_key_matches(
    marker: &DurablePendingWakeMarker,
    agent_id: &str,
    kind: PendingWakeKind,
    work_id: &str,
) -> bool {
    marker.agent_id == agent_id && marker.kind == kind && marker.work_id == work_id
}

pub fn upsert_pending_wake_marker(
    existing: &[DurablePendingWakeMarker],
    marker: DurablePendingWakeMarker,
) -> Vec<DurablePendingWakeMarker> {
    let mut next = existing
        .iter()
        .filter(|entry| {
            !marker_key_matches(entry, &marker.agent_id, marker.kind, &marker.work_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    next.push(marker);
    next
}

#[derive(Clone)]
pub struct SandPendingWakeStore {
    file_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl SandPendingWakeStore {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        Self {
            file_path: root_dir.as_ref().join(SAND_PENDING_WAKE_FILE_NAME),
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    pub fn mark_pending(&self, marker: DurablePendingWakeMarker) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let pending = upsert_pending_wake_marker(&self.read_pending_unlocked(), marker);
        self.write_unlocked(&pending).is_ok()
    }

    pub fn list_pending(&self) -> Vec<DurablePendingWakeMarker> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.read_pending_unlocked()
    }

    pub fn has_pending(
        &self,
        agent_id: &str,
        kind: PendingWakeKind,
        work_id: &str,
    ) -> bool {
        self.list_pending()
            .iter()
            .any(|entry| marker_key_matches(entry, agent_id, kind, work_id))
    }

    pub fn clear_one(
        &self,
        agent_id: &str,
        kind: PendingWakeKind,
        work_id: &str,
    ) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let existing = self.read_pending_unlocked();
        let remaining = existing
            .iter()
            .filter(|entry| !marker_key_matches(entry, agent_id, kind, work_id))
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == existing.len() {
            return false;
        }
        if remaining.is_empty() {
            let _ = self.delete_unlocked();
            true
        } else {
            self.write_unlocked(&remaining).is_ok()
        }
    }

    pub fn clear_if_matches(&self, marker: &DurablePendingWakeMarker) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let existing = self.read_pending_unlocked();
        let remaining = existing
            .iter()
            .filter(|entry| *entry != marker)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == existing.len() {
            return false;
        }
        if remaining.is_empty() {
            self.delete_unlocked().is_ok()
        } else {
            self.write_unlocked(&remaining).is_ok()
        }
    }

    pub fn mark_if_absent_or_matches(&self, marker: &DurablePendingWakeMarker) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let existing = self.read_pending_unlocked();
        if existing.iter().any(|entry| entry == marker) {
            return true;
        }
        if existing.iter().any(|entry| {
            marker_key_matches(entry, &marker.agent_id, marker.kind, &marker.work_id)
        }) {
            return false;
        }
        let mut next = existing;
        next.push(marker.clone());
        self.write_unlocked(&next).is_ok()
    }

    pub fn clear_agent(&self, agent_id: &str) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let existing = self.read_pending_unlocked();
        let remaining = existing
            .iter()
            .filter(|entry| entry.agent_id != agent_id)
            .cloned()
            .collect::<Vec<_>>();
        if remaining.len() == existing.len() {
            return;
        }
        if remaining.is_empty() {
            let _ = self.delete_unlocked();
        } else {
            let _ = self.write_unlocked(&remaining);
        }
    }

    pub fn clear_all(&self) {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = self.delete_unlocked();
    }

    pub fn prune_stale(
        &self,
        max_age_ms: f64,
        now_ms: f64,
    ) -> Vec<DurablePendingWakeMarker> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let existing = self.read_pending_unlocked();
        let (pruned, remaining): (Vec<_>, Vec<_>) = existing
            .into_iter()
            .partition(|entry| now_ms - entry.marked_at_ms > max_age_ms);
        if pruned.is_empty() {
            return Vec::new();
        }
        if remaining.is_empty() {
            let _ = self.delete_unlocked();
        } else {
            let _ = self.write_unlocked(&remaining);
        }
        pruned
    }

    fn read_pending_unlocked(&self) -> Vec<DurablePendingWakeMarker> {
        fs::read_to_string(&self.file_path)
            .ok()
            .map(|raw| parse_pending_wake_file(Some(&raw)))
            .unwrap_or_default()
    }

    fn write_unlocked(&self, pending: &[DurablePendingWakeMarker]) -> std::io::Result<()> {
        #[derive(Serialize)]
        struct File<'a> {
            version: u8,
            pending: &'a [DurablePendingWakeMarker],
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
