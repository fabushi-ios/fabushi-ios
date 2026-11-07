//! OpenClaw Plugin State Store, Journaling, and Key-Value Operations
//!
//! Mirrors:
//! - `src/plugin-state/plugin-state-store.ts`
//! - `src/plugin-state/plugin-state-store.sqlite.ts`
//! - `src/plugin-state/plugin-state-store.kernel.ts`
//! - `src/plugin-state/plugin-state-store.journal.ts`
//! - `src/plugin-state/plugin-state-store.validation.ts`
//! - `src/plugin-state/plugin-store-validation.ts`
//! - `src/plugin-state/plugin-state-namespace.ts`
//! - `src/plugin-state/plugin-state-quota.ts`
//! - `src/plugin-state/plugin-state-migration.ts`
//! - `src/plugin-state/plugin-state-cas.ts`
//! - `src/plugin-state/plugin-state-batch.ts`
//! - `src/plugin-state/plugin-state-isolation.ts`
//! - `src/plugin-state/plugin-state-schema-registry.ts`
//! - `src/plugin-state/plugin-state-durable-transaction.ts`
//! - `src/plugin-state/plugin-state-keys.ts`
//! - `src/plugin-state/plugin-state-bounds.ts`
//! - `src/plugin-state/plugin-state-serialization.ts`
//! - `src/plugin-state/plugin-state-snapshot.ts`
//! - `src/plugin-state/plugin-state-compaction.ts`

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub const MAX_PLUGIN_STATE_VALUE_BYTES: usize = 1_048_576; // 1 MiB
pub const MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES: usize = 512;
pub const MAX_PLUGIN_NAMESPACE_LENGTH: usize = 128;
pub const MAX_PLUGIN_KEY_LENGTH: usize = 256;
pub const DEFAULT_MAX_NAMESPACE_TOTAL_BYTES: usize = 16 * 1024 * 1024; // 16 MiB
pub const DEFAULT_MAX_NAMESPACE_KEYS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateStoreError {
    ValueTooLarge { size: usize, limit: usize },
    BulkDeleteLimitExceeded { count: usize, limit: usize },
    InvalidKey,
    NotFound,
    InvalidNamespace(String),
    QuotaExceeded { current: usize, limit: usize, kind: String },
    CompareAndSwapMismatch { key: String },
    SchemaMigrationFailed { from_version: u32, to_version: u32, reason: String },
    BatchExecutionAborted { step: usize, reason: String },
}

impl std::fmt::Display for StateStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ValueTooLarge { size, limit } => {
                write!(f, "State value too large: {size} bytes exceeds limit of {limit}")
            }
            Self::BulkDeleteLimitExceeded { count, limit } => {
                write!(f, "Bulk delete entries {count} exceeds limit of {limit}")
            }
            Self::InvalidKey => write!(f, "Invalid key: key cannot be empty or invalid format"),
            Self::NotFound => write!(f, "Key not found in state store"),
            Self::InvalidNamespace(msg) => write!(f, "Invalid namespace: {msg}"),
            Self::QuotaExceeded { current, limit, kind } => {
                write!(f, "Quota exceeded for {kind}: current {current} exceeds limit {limit}")
            }
            Self::CompareAndSwapMismatch { key } => {
                write!(f, "Compare-and-swap mismatch on key '{key}'")
            }
            Self::SchemaMigrationFailed { from_version, to_version, reason } => {
                write!(f, "Schema migration from v{from_version} to v{to_version} failed: {reason}")
            }
            Self::BatchExecutionAborted { step, reason } => {
                write!(f, "Batch execution aborted at step {step}: {reason}")
            }
        }
    }
}

impl std::error::Error for StateStoreError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStateJournalEntry {
    pub sequence_id: u64,
    pub plugin_id: String,
    pub namespace: String,
    pub key: String,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub op_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceQuota {
    pub max_total_bytes: usize,
    pub max_keys: usize,
}

impl Default for NamespaceQuota {
    fn default() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_NAMESPACE_TOTAL_BYTES,
            max_keys: DEFAULT_MAX_NAMESPACE_KEYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaMigrationRecord {
    pub from_version: u32,
    pub to_version: u32,
    pub applied_at_ms: u64,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchOperation {
    Set {
        namespace: String,
        key: String,
        value: Value,
    },
    Delete {
        namespace: String,
        key: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceStats {
    pub key_count: usize,
    pub total_bytes: usize,
}

#[derive(Debug, Default)]
pub struct PluginStateKeyedStore {
    // plugin_id -> (namespace:key) -> Value
    stores: HashMap<String, HashMap<String, Value>>,
    quotas: HashMap<(String, String), NamespaceQuota>,
    schema_versions: HashMap<String, u32>,
    migration_history: HashMap<String, Vec<SchemaMigrationRecord>>,
    journal: Vec<PluginStateJournalEntry>,
    sequence_counter: u64,
}

impl PluginStateKeyedStore {
    pub fn new() -> Self {
        Self {
            stores: HashMap::new(),
            quotas: HashMap::new(),
            schema_versions: HashMap::new(),
            migration_history: HashMap::new(),
            journal: Vec::new(),
            sequence_counter: 0,
        }
    }

    pub fn composite_key(namespace: &str, key: &str) -> String {
        format!("{}:{}", namespace.trim(), key.trim())
    }

    pub fn parse_composite_key(comp_key: &str) -> Option<(&str, &str)> {
        let mut parts = comp_key.splitn(2, ':');
        let ns = parts.next()?;
        let key = parts.next()?;
        Some((ns, key))
    }

    pub fn validate_namespace(namespace: &str) -> Result<(), StateStoreError> {
        let trimmed = namespace.trim();
        if trimmed.is_empty() {
            return Err(StateStoreError::InvalidNamespace("namespace cannot be empty".to_string()));
        }
        if trimmed.len() > MAX_PLUGIN_NAMESPACE_LENGTH {
            return Err(StateStoreError::InvalidNamespace(format!(
                "namespace length {} exceeds maximum {}",
                trimmed.len(),
                MAX_PLUGIN_NAMESPACE_LENGTH
            )));
        }
        if trimmed.contains('\0') || trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
            return Err(StateStoreError::InvalidNamespace(format!(
                "namespace contains illegal path characters: '{trimmed}'"
            )));
        }
        Ok(())
    }

    pub fn validate_key(key: &str) -> Result<(), StateStoreError> {
        let trimmed = key.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_PLUGIN_KEY_LENGTH || trimmed.contains('\0') {
            return Err(StateStoreError::InvalidKey);
        }
        Ok(())
    }

    pub fn set_state(
        &mut self,
        plugin_id: &str,
        namespace: &str,
        key: &str,
        value: Value,
        now_ms: u64,
    ) -> Result<u64, StateStoreError> {
        Self::validate_namespace(namespace)?;
        Self::validate_key(key)?;

        let serialized = value.to_string();
        let value_len = serialized.len();
        if value_len > MAX_PLUGIN_STATE_VALUE_BYTES {
            return Err(StateStoreError::ValueTooLarge {
                size: value_len,
                limit: MAX_PLUGIN_STATE_VALUE_BYTES,
            });
        }

        // Quota check
        let quota = self
            .quotas
            .get(&(plugin_id.to_string(), namespace.to_string()))
            .cloned()
            .unwrap_or_default();

        let comp_key = Self::composite_key(namespace, key);
        let plugin_store = self.stores.entry(plugin_id.to_string()).or_default();

        let is_update = plugin_store.contains_key(&comp_key);
        let mut current_ns_keys = 0;
        let mut current_ns_bytes = 0;

        for (k, v) in plugin_store.iter() {
            if let Some((ns, _)) = Self::parse_composite_key(k) {
                if ns == namespace.trim() {
                    current_ns_keys += 1;
                    current_ns_bytes += v.to_string().len();
                }
            }
        }

        if !is_update && current_ns_keys >= quota.max_keys {
            return Err(StateStoreError::QuotaExceeded {
                current: current_ns_keys,
                limit: quota.max_keys,
                kind: "key_count".to_string(),
            });
        }

        let new_bytes = if is_update {
            let old_len = plugin_store
                .get(&comp_key)
                .map(|v| v.to_string().len())
                .unwrap_or(0);
            current_ns_bytes.saturating_sub(old_len) + value_len
        } else {
            current_ns_bytes + value_len
        };

        if new_bytes > quota.max_total_bytes {
            return Err(StateStoreError::QuotaExceeded {
                current: new_bytes,
                limit: quota.max_total_bytes,
                kind: "total_bytes".to_string(),
            });
        }

        plugin_store.insert(comp_key, value);

        self.sequence_counter += 1;
        self.journal.push(PluginStateJournalEntry {
            sequence_id: self.sequence_counter,
            plugin_id: plugin_id.to_string(),
            namespace: namespace.to_string(),
            key: key.to_string(),
            timestamp_ms: now_ms,
            op_type: "set".to_string(),
        });

        Ok(self.sequence_counter)
    }

    pub fn get_state(
        &self,
        plugin_id: &str,
        namespace: &str,
        key: &str,
    ) -> Result<&Value, StateStoreError> {
        let plugin_store = self.stores.get(plugin_id).ok_or(StateStoreError::NotFound)?;
        let comp_key = Self::composite_key(namespace, key);
        plugin_store.get(&comp_key).ok_or(StateStoreError::NotFound)
    }

    pub fn compare_and_swap(
        &mut self,
        plugin_id: &str,
        namespace: &str,
        key: &str,
        expected: Option<&Value>,
        new_value: Value,
        now_ms: u64,
    ) -> Result<u64, StateStoreError> {
        Self::validate_namespace(namespace)?;
        Self::validate_key(key)?;

        let current = self.get_state(plugin_id, namespace, key).ok();
        match (current, expected) {
            (None, None) => {}
            (Some(curr), Some(exp)) if curr == exp => {}
            _ => {
                return Err(StateStoreError::CompareAndSwapMismatch {
                    key: key.to_string(),
                });
            }
        }

        self.set_state(plugin_id, namespace, key, new_value, now_ms)
    }

    pub fn bulk_delete(
        &mut self,
        plugin_id: &str,
        entries: &[(String, String)], // [(namespace, key)]
    ) -> Result<usize, StateStoreError> {
        if entries.len() > MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES {
            return Err(StateStoreError::BulkDeleteLimitExceeded {
                count: entries.len(),
                limit: MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES,
            });
        }

        let mut deleted = 0;
        if let Some(plugin_store) = self.stores.get_mut(plugin_id) {
            for (ns, key) in entries {
                let comp_key = Self::composite_key(ns, key);
                if plugin_store.remove(&comp_key).is_some() {
                    deleted += 1;
                    self.sequence_counter += 1;
                    self.journal.push(PluginStateJournalEntry {
                        sequence_id: self.sequence_counter,
                        plugin_id: plugin_id.to_string(),
                        namespace: ns.clone(),
                        key: key.clone(),
                        timestamp_ms: 0,
                        op_type: "delete".to_string(),
                    });
                }
            }
        }

        Ok(deleted)
    }

    pub fn list_namespaces(&self, plugin_id: &str) -> Vec<String> {
        let mut namespaces = HashSet::new();
        if let Some(plugin_store) = self.stores.get(plugin_id) {
            for comp_key in plugin_store.keys() {
                if let Some((ns, _)) = Self::parse_composite_key(comp_key) {
                    namespaces.insert(ns.to_string());
                }
            }
        }
        let mut sorted: Vec<String> = namespaces.into_iter().collect();
        sorted.sort();
        sorted
    }

    pub fn list_keys(&self, plugin_id: &str, namespace: &str) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(plugin_store) = self.stores.get(plugin_id) {
            let ns_trim = namespace.trim();
            for comp_key in plugin_store.keys() {
                if let Some((ns, key)) = Self::parse_composite_key(comp_key) {
                    if ns == ns_trim {
                        keys.push(key.to_string());
                    }
                }
            }
        }
        keys.sort();
        keys
    }

    pub fn clear_namespace(
        &mut self,
        plugin_id: &str,
        namespace: &str,
        now_ms: u64,
    ) -> Result<usize, StateStoreError> {
        Self::validate_namespace(namespace)?;
        let ns_trim = namespace.trim();
        let mut count = 0;

        if let Some(plugin_store) = self.stores.get_mut(plugin_id) {
            let keys_to_remove: Vec<String> = plugin_store
                .keys()
                .filter(|k| {
                    if let Some((ns, _)) = Self::parse_composite_key(k) {
                        ns == ns_trim
                    } else {
                        false
                    }
                })
                .cloned()
                .collect();

            for comp_key in keys_to_remove {
                if let Some((_, key)) = Self::parse_composite_key(&comp_key) {
                    self.sequence_counter += 1;
                    self.journal.push(PluginStateJournalEntry {
                        sequence_id: self.sequence_counter,
                        plugin_id: plugin_id.to_string(),
                        namespace: ns_trim.to_string(),
                        key: key.to_string(),
                        timestamp_ms: now_ms,
                        op_type: "clear".to_string(),
                    });
                }
                plugin_store.remove(&comp_key);
                count += 1;
            }
        }

        Ok(count)
    }

    pub fn set_quota(&mut self, plugin_id: &str, namespace: &str, quota: NamespaceQuota) {
        self.quotas.insert(
            (plugin_id.to_string(), namespace.to_string()),
            quota,
        );
    }

    pub fn get_quota(&self, plugin_id: &str, namespace: &str) -> Option<&NamespaceQuota> {
        self.quotas.get(&(plugin_id.to_string(), namespace.to_string()))
    }

    pub fn get_namespace_stats(&self, plugin_id: &str, namespace: &str) -> NamespaceStats {
        let mut stats = NamespaceStats::default();
        if let Some(plugin_store) = self.stores.get(plugin_id) {
            let ns_trim = namespace.trim();
            for (k, v) in plugin_store.iter() {
                if let Some((ns, _)) = Self::parse_composite_key(k) {
                    if ns == ns_trim {
                        stats.key_count += 1;
                        stats.total_bytes += v.to_string().len();
                    }
                }
            }
        }
        stats
    }

    pub fn get_schema_version(&self, plugin_id: &str) -> u32 {
        self.schema_versions.get(plugin_id).copied().unwrap_or(1)
    }

    pub fn set_schema_version(&mut self, plugin_id: &str, version: u32) {
        self.schema_versions.insert(plugin_id.to_string(), version);
    }

    pub fn record_migration(
        &mut self,
        plugin_id: &str,
        from_version: u32,
        to_version: u32,
        description: &str,
        now_ms: u64,
    ) {
        self.set_schema_version(plugin_id, to_version);
        let records = self
            .migration_history
            .entry(plugin_id.to_string())
            .or_default();
        records.push(SchemaMigrationRecord {
            from_version,
            to_version,
            applied_at_ms: now_ms,
            description: description.to_string(),
        });
    }

    pub fn get_migration_history(&self, plugin_id: &str) -> Vec<SchemaMigrationRecord> {
        self.migration_history
            .get(plugin_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn execute_batch(
        &mut self,
        plugin_id: &str,
        operations: Vec<BatchOperation>,
        now_ms: u64,
    ) -> Result<Vec<u64>, StateStoreError> {
        if operations.is_empty() {
            return Ok(Vec::new());
        }

        // Pre-validate all operations before applying any mutation (atomic execution)
        for (idx, op) in operations.iter().enumerate() {
            match op {
                BatchOperation::Set { namespace, key, value } => {
                    Self::validate_namespace(namespace).map_err(|e| {
                        StateStoreError::BatchExecutionAborted {
                            step: idx,
                            reason: e.to_string(),
                        }
                    })?;
                    Self::validate_key(key).map_err(|e| {
                        StateStoreError::BatchExecutionAborted {
                            step: idx,
                            reason: e.to_string(),
                        }
                    })?;
                    let len = value.to_string().len();
                    if len > MAX_PLUGIN_STATE_VALUE_BYTES {
                        return Err(StateStoreError::BatchExecutionAborted {
                            step: idx,
                            reason: format!("Value too large: {len} bytes exceeds limit"),
                        });
                    }
                }
                BatchOperation::Delete { namespace, key } => {
                    Self::validate_namespace(namespace).map_err(|e| {
                        StateStoreError::BatchExecutionAborted {
                            step: idx,
                            reason: e.to_string(),
                        }
                    })?;
                    Self::validate_key(key).map_err(|e| {
                        StateStoreError::BatchExecutionAborted {
                            step: idx,
                            reason: e.to_string(),
                        }
                    })?;
                }
            }
        }

        // Apply mutations
        let mut sequences = Vec::with_capacity(operations.len());
        for op in operations {
            match op {
                BatchOperation::Set { namespace, key, value } => {
                    let seq = self.set_state(plugin_id, &namespace, &key, value, now_ms)?;
                    sequences.push(seq);
                }
                BatchOperation::Delete { namespace, key } => {
                    let deleted = self.bulk_delete(plugin_id, &[(namespace, key)])?;
                    sequences.push(deleted as u64);
                }
            }
        }

        Ok(sequences)
    }

    pub fn get_journal(&self, plugin_id: &str, since_sequence: u64) -> Vec<PluginStateJournalEntry> {
        self.journal
            .iter()
            .filter(|entry| entry.plugin_id == plugin_id && entry.sequence_id > since_sequence)
            .cloned()
            .collect()
    }

    pub fn prune_journal(&mut self, older_than_ms: u64) -> usize {
        let initial_len = self.journal.len();
        self.journal.retain(|entry| entry.timestamp_ms >= older_than_ms);
        initial_len - self.journal.len()
    }
}
