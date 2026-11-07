//! OpenClaw Plugin Blob Store, Storage Bounds, and Expiration Eviction
//!
//! Mirrors:
//! - `src/plugin-state/plugin-blob-store.ts`
//! - `src/plugin-state/plugin-blob-store.sqlite.ts`
//! - `src/plugin-state/plugin-blob-store.types.ts`
//! - `src/plugin-state/plugin-blob-worker-client.ts`
//! - `src/plugin-state/plugin-blob-store.admission.test.ts`
//! - `src/plugin-state/plugin-blob-store.eviction.test.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

pub const MAX_PLUGIN_BLOB_BYTES_PER_ENTRY: usize = 100 * 1024 * 1024; // 100 MiB
pub const MAX_PLUGIN_BLOB_BYTES_PER_PLUGIN: usize = 512 * 1024 * 1024; // 512 MiB
pub const MAX_PLUGIN_BLOB_ENTRIES_PER_PLUGIN: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginBlobEntry {
    pub namespace: String,
    pub key: String,
    pub data: Vec<u8>,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobStoreError {
    EntryTooLarge { size: usize, limit: usize },
    PluginBytesQuotaExceeded { current: usize, added: usize, limit: usize },
    PluginEntryCountQuotaExceeded { current: usize, limit: usize },
    NotFound,
    InvalidKey,
}

#[derive(Debug, Default)]
pub struct PluginBlobStore {
    // plugin_id -> map of (namespace:key) -> PluginBlobEntry
    stores: HashMap<String, HashMap<String, PluginBlobEntry>>,
}

impl PluginBlobStore {
    pub fn new() -> Self {
        Self {
            stores: HashMap::new(),
        }
    }

    fn composite_key(namespace: &str, key: &str) -> String {
        format!("{}:{}", namespace.trim(), key.trim())
    }

    pub fn put_blob(
        &mut self,
        plugin_id: &str,
        namespace: &str,
        key: &str,
        data: Vec<u8>,
        ttl_ms: Option<u64>,
        now_ms: u64,
    ) -> Result<(), BlobStoreError> {
        if key.trim().is_empty() || namespace.trim().is_empty() {
            return Err(BlobStoreError::InvalidKey);
        }

        if data.len() > MAX_PLUGIN_BLOB_BYTES_PER_ENTRY {
            return Err(BlobStoreError::EntryTooLarge {
                size: data.len(),
                limit: MAX_PLUGIN_BLOB_BYTES_PER_ENTRY,
            });
        }

        let plugin_store = self.stores.entry(plugin_id.to_string()).or_default();
        let comp_key = Self::composite_key(namespace, key);

        let existing_bytes = plugin_store.get(&comp_key).map_or(0, |e| e.data.len());
        let current_total_bytes: usize = plugin_store.values().map(|e| e.data.len()).sum();
        let new_total_bytes = current_total_bytes.saturating_sub(existing_bytes) + data.len();

        if new_total_bytes > MAX_PLUGIN_BLOB_BYTES_PER_PLUGIN {
            return Err(BlobStoreError::PluginBytesQuotaExceeded {
                current: current_total_bytes,
                added: data.len(),
                limit: MAX_PLUGIN_BLOB_BYTES_PER_PLUGIN,
            });
        }

        if !plugin_store.contains_key(&comp_key) && plugin_store.len() >= MAX_PLUGIN_BLOB_ENTRIES_PER_PLUGIN {
            return Err(BlobStoreError::PluginEntryCountQuotaExceeded {
                current: plugin_store.len(),
                limit: MAX_PLUGIN_BLOB_ENTRIES_PER_PLUGIN,
            });
        }

        let expires_at_ms = ttl_ms.map(|ttl| now_ms.saturating_add(ttl));
        plugin_store.insert(
            comp_key,
            PluginBlobEntry {
                namespace: namespace.to_string(),
                key: key.to_string(),
                data,
                created_at_ms: now_ms,
                expires_at_ms,
            },
        );

        Ok(())
    }

    pub fn get_blob(
        &self,
        plugin_id: &str,
        namespace: &str,
        key: &str,
        now_ms: u64,
    ) -> Result<&[u8], BlobStoreError> {
        let plugin_store = self.stores.get(plugin_id).ok_or(BlobStoreError::NotFound)?;
        let comp_key = Self::composite_key(namespace, key);
        let entry = plugin_store.get(&comp_key).ok_or(BlobStoreError::NotFound)?;

        if let Some(exp) = entry.expires_at_ms {
            if now_ms >= exp {
                return Err(BlobStoreError::NotFound);
            }
        }

        Ok(&entry.data)
    }

    pub fn delete_blob(&mut self, plugin_id: &str, namespace: &str, key: &str) -> bool {
        if let Some(plugin_store) = self.stores.get_mut(plugin_id) {
            let comp_key = Self::composite_key(namespace, key);
            plugin_store.remove(&comp_key).is_some()
        } else {
            false
        }
    }

    pub fn delete_expired_blobs(&mut self, plugin_id: &str, now_ms: u64) -> usize {
        if let Some(plugin_store) = self.stores.get_mut(plugin_id) {
            let before = plugin_store.len();
            plugin_store.retain(|_, entry| {
                entry.expires_at_ms.map_or(true, |exp| now_ms < exp)
            });
            before - plugin_store.len()
        } else {
            0
        }
    }
}
