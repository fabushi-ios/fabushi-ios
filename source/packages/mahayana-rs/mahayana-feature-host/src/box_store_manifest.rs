use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value};

pub const BOX_STORE_MANIFEST_REL_PATH: &str = "manifest.json";
pub const BOX_STORE_LEGACY_MANIFEST_VERSION: u64 = 1;
pub const BOX_STORE_MANIFEST_VERSION: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxStoreManifestEntry {
    LegacyFile { sha: String, size: u64 },
    File { sha: String, size: u64, mode: u32 },
    Symlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreManifest {
    pub version: u64,
    pub updated_at_ms: u64,
    pub writer_window_id: Option<String>,
    pub fully_hydrated: Option<bool>,
    pub entries: BTreeMap<String, BoxStoreManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxStoreManifestParseError {
    UnsupportedVersion(u64),
    PathConflict,
}

impl std::fmt::Display for BoxStoreManifestParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported box store manifest version {version}")
            }
            Self::PathConflict => {
                write!(formatter, "box store manifest contains conflicting node paths")
            }
        }
    }
}

impl std::error::Error for BoxStoreManifestParseError {}

fn non_empty_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn nonnegative_integer(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key)?.as_u64()
}

pub fn parse_manifest_entry(value: &Value) -> Option<BoxStoreManifestEntry> {
    let map = value.as_object()?;
    match map.get("kind") {
        Some(Value::String(kind)) if kind == "file" => {
            let sha = non_empty_string(map, "sha")?;
            let size = nonnegative_integer(map, "size")?;
            let mode = nonnegative_integer(map, "mode")?;
            if mode > 0o777 {
                return None;
            }
            Some(BoxStoreManifestEntry::File {
                sha,
                size,
                mode: mode as u32,
            })
        }
        Some(Value::String(kind)) if kind == "symlink" => {
            Some(BoxStoreManifestEntry::Symlink {
                target: non_empty_string(map, "target")?,
            })
        }
        Some(_) => None,
        None => {
            if map.contains_key("mode") {
                return None;
            }
            Some(BoxStoreManifestEntry::LegacyFile {
                sha: non_empty_string(map, "sha")?,
                size: nonnegative_integer(map, "size")?,
            })
        }
    }
}

pub fn has_manifest_path_conflict<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    let paths = paths.into_iter().collect::<HashSet<_>>();
    for path in &paths {
        for (separator, _) in path.match_indices('/') {
            if paths.contains(&path[..separator]) {
                return true;
            }
        }
    }
    false
}

pub fn parse_box_store_manifest(value: &Value) -> Option<BoxStoreManifest> {
    let map = value.as_object()?;
    let version = map.get("version")?.as_u64()?;
    if version != BOX_STORE_LEGACY_MANIFEST_VERSION && version != BOX_STORE_MANIFEST_VERSION {
        return None;
    }
    let updated_at_ms = map.get("updatedAtMs")?.as_u64()?;
    let writer_window_id = match map.get("writerWindowId") {
        None => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return None,
    };
    let fully_hydrated = match map.get("fullyHydrated") {
        None => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return None,
    };
    let raw_entries = map.get("entries")?.as_object()?;
    let mut entries = BTreeMap::new();
    for (path, value) in raw_entries {
        let entry = parse_manifest_entry(value)?;
        if version == BOX_STORE_LEGACY_MANIFEST_VERSION
            && !matches!(entry, BoxStoreManifestEntry::LegacyFile { .. })
        {
            return None;
        }
        entries.insert(path.clone(), entry);
    }
    Some(BoxStoreManifest {
        version,
        updated_at_ms,
        writer_window_id,
        fully_hydrated,
        entries,
    })
}

pub fn parse_manifest_bytes(
    bytes: &[u8],
) -> Result<Option<BoxStoreManifest>, BoxStoreManifestParseError> {
    let value = match serde_json::from_slice::<Value>(bytes) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if let Some(version) = value
        .as_object()
        .and_then(|map| map.get("version"))
        .and_then(Value::as_u64)
    {
        if version != BOX_STORE_LEGACY_MANIFEST_VERSION && version != BOX_STORE_MANIFEST_VERSION {
            return Err(BoxStoreManifestParseError::UnsupportedVersion(version));
        }
    }
    let Some(manifest) = parse_box_store_manifest(&value) else {
        return Ok(None);
    };
    if has_manifest_path_conflict(manifest.entries.keys().map(String::as_str)) {
        return Err(BoxStoreManifestParseError::PathConflict);
    }
    Ok(Some(manifest))
}

pub fn parse_manifest_entries(value: &Value) -> Result<BTreeMap<String, BoxStoreManifestEntry>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "box store manifest entries must be an object".to_string())?;
    let mut entries = BTreeMap::new();
    for (path, value) in object {
        if path.is_empty() {
            return Err("box store manifest entry path must not be empty".into());
        }
        let entry = parse_manifest_entry(value)
            .ok_or_else(|| format!("invalid box store manifest entry: {path}"))?;
        entries.insert(path.clone(), entry);
    }
    if has_manifest_path_conflict(entries.keys().map(String::as_str)) {
        return Err(BoxStoreManifestParseError::PathConflict.to_string());
    }
    Ok(entries)
}

fn manifest_entry_value(entry: &BoxStoreManifestEntry) -> Value {
    let mut value = Map::new();
    match entry {
        BoxStoreManifestEntry::LegacyFile { sha, size } => {
            value.insert("sha".into(), Value::String(sha.clone()));
            value.insert("size".into(), Value::from(*size));
        }
        BoxStoreManifestEntry::File { sha, size, mode } => {
            value.insert("kind".into(), Value::String("file".into()));
            value.insert("sha".into(), Value::String(sha.clone()));
            value.insert("size".into(), Value::from(*size));
            value.insert("mode".into(), Value::from(*mode));
        }
        BoxStoreManifestEntry::Symlink { target } => {
            value.insert("kind".into(), Value::String("symlink".into()));
            value.insert("target".into(), Value::String(target.clone()));
        }
    }
    Value::Object(value)
}

pub fn manifest_value(manifest: &BoxStoreManifest) -> Value {
    let mut serialized_entries = Map::new();
    for (path, entry) in &manifest.entries {
        serialized_entries.insert(path.clone(), manifest_entry_value(entry));
    }
    let mut payload = Map::new();
    payload.insert("version".into(), Value::from(manifest.version));
    payload.insert("updatedAtMs".into(), Value::from(manifest.updated_at_ms));
    if let Some(writer_window_id) = manifest.writer_window_id.as_deref() {
        payload.insert("writerWindowId".into(), Value::String(writer_window_id.to_string()));
    }
    if let Some(fully_hydrated) = manifest.fully_hydrated {
        payload.insert("fullyHydrated".into(), Value::Bool(fully_hydrated));
    }
    payload.insert("entries".into(), Value::Object(serialized_entries));
    Value::Object(payload)
}

pub fn serialize_manifest_v2_bytes(
    entries: &BTreeMap<String, BoxStoreManifestEntry>,
    writer_window_id: Option<&str>,
    fully_hydrated: Option<bool>,
    updated_at_ms: u64,
) -> Result<Vec<u8>, String> {
    if has_manifest_path_conflict(entries.keys().map(String::as_str)) {
        return Err(BoxStoreManifestParseError::PathConflict.to_string());
    }
    serde_json::to_vec(&manifest_value(&BoxStoreManifest {
        version: BOX_STORE_MANIFEST_VERSION,
        updated_at_ms,
        writer_window_id: writer_window_id.map(str::to_string),
        fully_hydrated,
        entries: entries.clone(),
    }))
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_v1_and_v2_parse_with_desktop_contract() {
        let v1 = parse_manifest_bytes(
            br#"{"version":1,"updatedAtMs":7,"entries":{"a.txt":{"sha":"abc","size":3}}}"#,
        )
        .expect("v1 parse")
        .expect("v1 manifest");
        assert_eq!(v1.version, 1);

        let v2 = parse_manifest_bytes(
            br#"{"version":2,"updatedAtMs":8,"writerWindowId":"window-1","fullyHydrated":false,"entries":{"a.txt":{"kind":"file","sha":"abc","size":3,"mode":420},"link":{"kind":"symlink","target":"a.txt"}}}"#,
        )
        .expect("v2 parse")
        .expect("v2 manifest");
        assert_eq!(v2.version, 2);
        assert_eq!(v2.writer_window_id.as_deref(), Some("window-1"));
        assert_eq!(v2.fully_hydrated, Some(false));
    }

    #[test]
    fn manifest_rejects_unsupported_versions_and_path_conflicts() {
        assert!(matches!(
            parse_manifest_bytes(br#"{"version":99,"updatedAtMs":1,"entries":{}}"#),
            Err(BoxStoreManifestParseError::UnsupportedVersion(99))
        ));
        assert!(matches!(
            parse_manifest_bytes(
                br#"{"version":2,"updatedAtMs":1,"entries":{"a":{"kind":"file","sha":"a","size":1,"mode":420},"a/b":{"kind":"file","sha":"b","size":1,"mode":420}}}"#
            ),
            Err(BoxStoreManifestParseError::PathConflict)
        ));
    }

    #[test]
    fn canonical_v2_serialization_is_strict_and_round_trips() {
        let entries = parse_manifest_entries(&json!({
            "memory/state.json": {"kind":"file","sha":"deadbeef","size":4,"mode":384},
            "memory/latest": {"kind":"symlink","target":"state.json"}
        }))
        .expect("entries");
        let bytes = serialize_manifest_v2_bytes(
            &entries,
            Some("ios-window-1"),
            Some(true),
            42,
        )
        .expect("serialize");
        let manifest = parse_manifest_bytes(&bytes)
            .expect("parse")
            .expect("manifest");
        assert_eq!(manifest.version, BOX_STORE_MANIFEST_VERSION);
        assert_eq!(manifest.updated_at_ms, 42);
        assert_eq!(manifest.fully_hydrated, Some(true));
        assert_eq!(manifest.entries, entries);
    }
}
