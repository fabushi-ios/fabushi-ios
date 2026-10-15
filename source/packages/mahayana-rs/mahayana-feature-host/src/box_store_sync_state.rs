use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::box_store_manifest::{
    manifest_entries_value, parse_manifest_entries, BoxStoreManifestEntry,
};

const SYNC_STATE_VERSION: u64 = 1;
const SYNC_STATE_DIR: &str = ".box-store-sync-state";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStoreSyncBase {
    pub etag: String,
    pub entries: BTreeMap<String, BoxStoreManifestEntry>,
    pub fully_hydrated: Option<bool>,
}

fn safe_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id.len() <= 256
        && !agent_id.contains('/')
        && !agent_id.contains('\\')
        && agent_id != "."
        && agent_id != ".."
        && !agent_id.contains('\0')
}

fn state_path(account_agents_root: &Path, agent_id: &str) -> Result<PathBuf, String> {
    if !safe_agent_id(agent_id) {
        return Err("unsafe Agent Store sync-base identity".into());
    }
    Ok(account_agents_root
        .join(SYNC_STATE_DIR)
        .join(format!("{agent_id}.json")))
}

pub fn load_sync_base(
    account_agents_root: &Path,
    agent_id: &str,
) -> Result<Option<AgentStoreSyncBase>, String> {
    let path = state_path(account_agents_root, agent_id)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "Agent Store sync-base must be an object".to_string())?;
    if object.get("version").and_then(Value::as_u64) != Some(SYNC_STATE_VERSION) {
        return Err("unsupported Agent Store sync-base version".into());
    }
    let etag = object
        .get("etag")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Agent Store sync-base omitted ETag".to_string())?
        .to_string();
    let entries = parse_manifest_entries(
        object
            .get("entries")
            .ok_or_else(|| "Agent Store sync-base omitted entries".to_string())?,
    )?;
    let fully_hydrated = match object.get("fullyHydrated") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("Agent Store sync-base fullyHydrated is invalid".into()),
    };
    Ok(Some(AgentStoreSyncBase {
        etag,
        entries,
        fully_hydrated,
    }))
}

pub fn save_sync_base(
    account_agents_root: &Path,
    agent_id: &str,
    base: &AgentStoreSyncBase,
) -> Result<(), String> {
    if base.etag.is_empty() {
        return Err("Agent Store sync-base ETag must not be empty".into());
    }
    let path = state_path(account_agents_root, agent_id)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Agent Store sync-base path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;

    let value = json!({
        "version": SYNC_STATE_VERSION,
        "etag": base.etag.clone(),
        "entries": manifest_entries_value(&base.entries),
        "fullyHydrated": base.fully_hydrated,
    });
    let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    let temp = path.with_file_name(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("sync-base"),
        Uuid::new_v4()
    ));
    let write = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        fs::rename(&temp, &path).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if write.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("fabushi-agent-store-base-{}", Uuid::new_v4()))
    }

    #[test]
    fn sync_base_round_trips_outside_agent_payload() {
        let root = root();
        let entries = parse_manifest_entries(&json!({
            "memory/state.json": {
                "kind": "file",
                "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "size": 4,
                "mode": 384
            }
        }))
        .expect("entries");
        let base = AgentStoreSyncBase {
            etag: "sha256:manifest".into(),
            entries,
            fully_hydrated: Some(true),
        };
        save_sync_base(&root, "agent-1", &base).expect("save");
        assert_eq!(load_sync_base(&root, "agent-1").expect("load"), Some(base));
        assert!(root.join(SYNC_STATE_DIR).join("agent-1.json").is_file());
        assert!(!root.join("agent-1").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_base_rejects_unsafe_identity_and_corrupt_state() {
        let root = root();
        assert!(save_sync_base(
            &root,
            "../agent",
            &AgentStoreSyncBase {
                etag: "etag".into(),
                entries: BTreeMap::new(),
                fully_hydrated: Some(true),
            }
        )
        .is_err());

        fs::create_dir_all(root.join(SYNC_STATE_DIR)).unwrap();
        fs::write(
            root.join(SYNC_STATE_DIR).join("agent-1.json"),
            br#"{"version":99,"etag":"x","entries":{}}"#,
        )
        .unwrap();
        assert!(load_sync_base(&root, "agent-1").is_err());
        let _ = fs::remove_dir_all(root);
    }
}
