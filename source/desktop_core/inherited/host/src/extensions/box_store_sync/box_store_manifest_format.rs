use std::collections::BTreeMap;

use serde_json::{Map, Value};

pub const BOX_STORE_MANIFEST_REL_PATH: &str = "manifest.json";
pub const BOX_STORE_BLOBS_PREFIX: &str = "blobs";
pub const BOX_STORE_LEGACY_MANIFEST_VERSION: u64 = 1;
pub const BOX_STORE_MANIFEST_VERSION: u64 = 2;
pub const SAND_MANIFEST_V2_ENV: &str = "SAND_MANIFEST_V2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxStoreManifestEntry {
    LegacyFile { sha: String, size: u64 },
    File { sha: String, size: u64, mode: u32 },
    Symlink { target: String },
}

impl BoxStoreManifestEntry {
    pub fn is_file(&self) -> bool {
        !matches!(self, Self::Symlink { .. })
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreManifest {
    pub version: u64,
    pub updated_at_ms: u64,
    pub writer_window_id: Option<String>,
    pub fully_hydrated: Option<bool>,
    pub entries: BTreeMap<String, BoxStoreManifestEntry>,
}

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
        Some(Value::String(kind)) if kind == "symlink" => Some(
            BoxStoreManifestEntry::Symlink {
                target: non_empty_string(map, "target")?,
            },
        ),
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

pub fn is_symlink_manifest_value(value: &Value) -> bool {
    matches!(
        parse_manifest_entry(value),
        Some(BoxStoreManifestEntry::Symlink { .. })
    )
}

pub fn box_store_manifest_entries_equal(
    left: Option<&BoxStoreManifestEntry>,
    right: Option<&BoxStoreManifestEntry>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
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
