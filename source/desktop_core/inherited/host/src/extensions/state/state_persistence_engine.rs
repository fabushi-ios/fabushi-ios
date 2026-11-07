use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatePartitionEntry {
    pub key: String,
    pub version: u64,
    pub payload: Value,
}

#[derive(Clone, Default)]
pub struct StatePersistenceEngine {
    partitions: Arc<Mutex<HashMap<String, HashMap<String, StatePartitionEntry>>>>,
    migration_version: Arc<Mutex<u32>>,
}

impl StatePersistenceEngine {
    pub fn new(initial_migration_version: u32) -> Self {
        Self {
            partitions: Arc::new(Mutex::new(HashMap::new())),
            migration_version: Arc::new(Mutex::new(initial_migration_version)),
        }
    }

    pub fn migration_version(&self) -> u32 {
        *self.migration_version.lock().unwrap()
    }

    pub fn apply_migration(&self, target_version: u32) -> Result<(), String> {
        let mut ver = self.migration_version.lock().unwrap();
        if target_version <= *ver {
            return Err(format!(
                "Target migration version {target_version} must be strictly greater than current {ver}"
            ));
        }
        *ver = target_version;
        Ok(())
    }

    pub fn put_partition_entry(&self, partition: &str, key: &str, payload: Value) -> u64 {
        let mut partitions = self.partitions.lock().unwrap();
        let part = partitions.entry(partition.to_string()).or_default();

        let next_version = part.get(key).map(|e| e.version + 1).unwrap_or(1);
        part.insert(
            key.to_string(),
            StatePartitionEntry {
                key: key.to_string(),
                version: next_version,
                payload,
            },
        );

        next_version
    }

    pub fn get_partition_entry(&self, partition: &str, key: &str) -> Option<StatePartitionEntry> {
        let partitions = self.partitions.lock().unwrap();
        partitions.get(partition).and_then(|part| part.get(key).cloned())
    }
}
