use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAND_XUSER_PENDING_DEPARTURE_FILE_NAME: &str = "host-xuser-pending-departures.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum XuserDeparture {
    #[serde(rename = "leave-room")]
    LeaveRoom {
        #[serde(rename = "ownerAuthId", skip_serializing_if = "Option::is_none")]
        owner_auth_id: Option<String>,
        #[serde(rename = "roomId")]
        room_id: String,
    },
    #[serde(rename = "remove-agent")]
    RemoveAgent {
        #[serde(rename = "ownerAuthId", skip_serializing_if = "Option::is_none")]
        owner_auth_id: Option<String>,
        #[serde(rename = "agentId")]
        agent_id: String,
    },
}

pub fn departure_key(value: &XuserDeparture) -> String {
    match value {
        XuserDeparture::LeaveRoom { owner_auth_id, room_id } => {
            format!("leave-room\0{}\0{room_id}", owner_auth_id.as_deref().unwrap_or(""))
        }
        XuserDeparture::RemoveAgent { owner_auth_id, agent_id } => {
            format!("remove-agent\0{}\0{agent_id}", owner_auth_id.as_deref().unwrap_or(""))
        }
    }
}

fn load(path: &Path) -> Result<BTreeMap<String, XuserDeparture>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("read xuser departure store {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("decode xuser departure store: {error}"))?;
    let rows = value.get("pending").and_then(serde_json::Value::as_array)
        .ok_or_else(|| "decode xuser departure store: pending array is required".to_string())?;
    let mut map = BTreeMap::new();
    for row in rows {
        let departure: XuserDeparture = serde_json::from_value(row.clone())
            .map_err(|error| format!("decode xuser departure row: {error}"))?;
        map.insert(departure_key(&departure), departure);
    }
    Ok(map)
}

fn persist(path: &Path, map: &BTreeMap<String, XuserDeparture>) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "xuser departure store has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create xuser departure directory {}: {error}", parent.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let pending = map.values().cloned().collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({"version":2,"pending":pending}))
        .map_err(|error| format!("encode xuser departure store: {error}"))?;
    let mut file = File::create(&part)
        .map_err(|error| format!("create xuser departure temp {}: {error}", part.display()))?;
    file.write_all(&bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all())
        .map_err(|error| format!("write xuser departure temp {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("replace xuser departure store {}: {error}", path.display())
    })
}

pub struct SandXuserPendingDepartureStore {
    pub file_path: PathBuf,
    cache: Mutex<Option<BTreeMap<String, XuserDeparture>>>,
}

impl SandXuserPendingDepartureStore {
    pub fn new(root: &Path) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_PENDING_DEPARTURE_FILE_NAME),
            cache: Mutex::new(None),
        }
    }

    fn with_map<R>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, XuserDeparture>) -> Result<(R, bool), String>,
    ) -> Result<R, String> {
        let mut guard = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(load(&self.file_path)?);
        }
        let map = guard.as_mut().expect("departure cache initialized");
        let before = map.clone();
        let (output, changed) = operation(map)?;
        if changed {
            if let Err(error) = persist(&self.file_path, map) {
                *map = before;
                return Err(error);
            }
        }
        Ok(output)
    }

    pub fn list(&self) -> Result<Vec<XuserDeparture>, String> {
        self.with_map(|map| Ok((map.values().cloned().collect(), false)))
    }

    pub fn record(&self, value: XuserDeparture) -> Result<(), String> {
        self.with_map(|map| {
            let key = departure_key(&value);
            if map.contains_key(&key) {
                return Ok(((), false));
            }
            map.insert(key, value);
            Ok(((), true))
        })
    }

    pub fn clear(&self, value: &XuserDeparture) -> Result<(), String> {
        let key = departure_key(value);
        self.with_map(|map| Ok(((), map.remove(&key).is_some())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn departure_survives_restart_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-xuser-departure-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let departure = XuserDeparture::LeaveRoom {
            owner_auth_id: Some("account-a".into()),
            room_id: "room-a".into(),
        };
        {
            let store = SandXuserPendingDepartureStore::new(&root);
            store.record(departure.clone()).unwrap();
            store.record(departure.clone()).unwrap();
            assert_eq!(store.list().unwrap(), vec![departure.clone()]);
        }
        {
            let store = SandXuserPendingDepartureStore::new(&root);
            assert_eq!(store.list().unwrap(), vec![departure.clone()]);
            store.clear(&departure).unwrap();
            assert!(store.list().unwrap().is_empty());
        }
        let _ = fs::remove_dir_all(root);
    }
}
