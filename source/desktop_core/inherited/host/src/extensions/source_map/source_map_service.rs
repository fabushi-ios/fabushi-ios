use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::host_paths::get_sand_root_dir;

pub const BOX_STORE_SOURCE_KEY: &str = "box-store";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandSourceMode {
    Local,
    AgentStore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandSourceEntry {
    #[serde(rename = "sourceId")]
    pub source_id: String,
    pub mode: SandSourceMode,
}

pub fn get_source_map_path() -> PathBuf {
    get_sand_root_dir().join("source-map.json")
}

fn parse_source_map(value: Value) -> BTreeMap<String, SandSourceEntry> {
    let Some(object) = value.as_object() else {
        return BTreeMap::new();
    };
    object
        .iter()
        .filter_map(|(key, value)| {
            let entry = serde_json::from_value::<SandSourceEntry>(value.clone()).ok()?;
            (!entry.source_id.is_empty()).then(|| (key.clone(), entry))
        })
        .collect()
}

type SourceIdFactory = Arc<dyn Fn() -> String + Send + Sync>;

pub struct SandSourceMap {
    path: PathBuf,
    create_id: SourceIdFactory,
    cache: Mutex<Option<BTreeMap<String, SandSourceEntry>>>,
}

impl std::fmt::Debug for SandSourceMap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SandSourceMap")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Default for SandSourceMap {
    fn default() -> Self {
        Self::new(None)
    }
}

impl SandSourceMap {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self::with_create_id(path.unwrap_or_else(get_source_map_path), || {
            Uuid::new_v4().to_string()
        })
    }

    pub fn with_create_id<F>(path: PathBuf, create_id: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        Self {
            path,
            create_id: Arc::new(create_id),
            cache: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_or_create(&self, agent_id: &str) -> io::Result<SandSourceEntry> {
        let mut map = self.load();
        if let Some(existing) = map.get(agent_id) {
            return Ok(existing.clone());
        }
        let entry = SandSourceEntry {
            source_id: (self.create_id)(),
            mode: SandSourceMode::Local,
        };
        map.insert(agent_id.to_string(), entry.clone());
        self.save(&map)?;
        Ok(entry)
    }

    pub fn get_or_create_box_store(&self) -> io::Result<SandSourceEntry> {
        self.get_or_create(BOX_STORE_SOURCE_KEY)
    }

    pub fn get_box_store(&self) -> Option<SandSourceEntry> {
        self.load().get(BOX_STORE_SOURCE_KEY).cloned()
    }

    pub fn set_mode(
        &self,
        agent_id: &str,
        mode: SandSourceMode,
    ) -> io::Result<SandSourceEntry> {
        let mut map = self.load();
        let source_id = map
            .get(agent_id)
            .map(|entry| entry.source_id.clone())
            .unwrap_or_else(|| (self.create_id)());
        let entry = SandSourceEntry { source_id, mode };
        map.insert(agent_id.to_string(), entry.clone());
        self.save(&map)?;
        Ok(entry)
    }

    fn load(&self) -> BTreeMap<String, SandSourceEntry> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(map) = cache.as_ref() {
            return map.clone();
        }
        let map = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .map(parse_source_map)
            .unwrap_or_default();
        *cache = Some(map.clone());
        map
    }

    fn save(&self, map: &BTreeMap<String, SandSourceEntry>) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = PathBuf::from(format!(
            "{}.{}.tmp",
            self.path.to_string_lossy(),
            std::process::id()
        ));
        let encoded = serde_json::to_vec_pretty(map)
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::write(&temp_path, encoded)?;
        fs::rename(&temp_path, &self.path)?;
        *self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(map.clone());
        Ok(())
    }
}
