use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SAND_XUSER_ROOM_TOMBSTONE_FILE_NAME: &str = "host-xuser-room-tombstones.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XuserRoomTombstone {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_auth_id: Option<String>,
    pub torn_down_at_ms: u64,
}

fn key(value: &XuserRoomTombstone) -> String {
    format!("{}\0{}", value.owner_auth_id.as_deref().unwrap_or(""), value.room_id)
}

fn load(path: &Path) -> Result<BTreeMap<String, XuserRoomTombstone>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("read xuser tombstone store {}: {error}", path.display())),
    };
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("decode xuser tombstone store: {error}"))?;
    let rows = value.get("tombstones").and_then(serde_json::Value::as_array)
        .ok_or_else(|| "decode xuser tombstone store: tombstones array is required".to_string())?;
    let mut map = BTreeMap::new();
    for row in rows {
        let tombstone: XuserRoomTombstone = serde_json::from_value(row.clone())
            .map_err(|error| format!("decode xuser tombstone row: {error}"))?;
        map.insert(key(&tombstone), tombstone);
    }
    Ok(map)
}

fn persist(path: &Path, map: &BTreeMap<String, XuserRoomTombstone>) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "xuser tombstone store has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create xuser tombstone directory {}: {error}", parent.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let tombstones = map.values().cloned().collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"tombstones":tombstones}))
        .map_err(|error| format!("encode xuser tombstones: {error}"))?;
    let mut file = File::create(&part)
        .map_err(|error| format!("create xuser tombstone temp {}: {error}", part.display()))?;
    file.write_all(&bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all())
        .map_err(|error| format!("write xuser tombstone temp {}: {error}", part.display()))?;
    fs::rename(&part, path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("replace xuser tombstone store {}: {error}", path.display())
    })
}

pub struct SandXuserRoomTombstoneStore {
    pub file_path: PathBuf,
    cache: Mutex<Option<BTreeMap<String, XuserRoomTombstone>>>,
}

impl SandXuserRoomTombstoneStore {
    pub fn new(root: &Path) -> Self {
        Self {
            file_path: root.join(SAND_XUSER_ROOM_TOMBSTONE_FILE_NAME),
            cache: Mutex::new(None),
        }
    }

    fn with_map<R>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, XuserRoomTombstone>) -> Result<(R, bool), String>,
    ) -> Result<R, String> {
        let mut guard = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(load(&self.file_path)?);
        }
        let map = guard.as_mut().expect("tombstone cache initialized");
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

    pub fn list(&self) -> Result<Vec<XuserRoomTombstone>, String> {
        self.with_map(|map| Ok((map.values().cloned().collect(), false)))
    }

    pub fn record(&self, value: XuserRoomTombstone) -> Result<(), String> {
        self.with_map(|map| {
            let key = key(&value);
            map.insert(key, value);
            Ok(((), true))
        })
    }

    pub fn clear(&self, room_id: &str, owner_auth_id: Option<&str>) -> Result<(), String> {
        let key = format!("{}\0{}", owner_auth_id.unwrap_or(""), room_id);
        self.with_map(|map| Ok(((), map.remove(&key).is_some())))
    }
}
