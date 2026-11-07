use crate::message::MediaRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Platform-neutral cache bookkeeping for media already materialized on a client.
/// Actual file I/O remains owned by the platform adapter; this type enforces the
/// shared byte budget and deterministic eviction semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCacheEntry {
    pub media: MediaRef,
    pub local_path: String,
    pub size_bytes: u64,
    pub last_accessed_ms: i64,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCache {
    max_bytes: u64,
    entries: BTreeMap<String, MediaCacheEntry>,
}

impl MediaCache {
    pub fn new(max_bytes: u64) -> Result<Self, MediaCacheError> {
        if max_bytes == 0 {
            return Err(MediaCacheError::InvalidBudget);
        }
        Ok(Self {
            max_bytes,
            entries: BTreeMap::new(),
        })
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries
            .values()
            .fold(0u64, |total, entry| total.saturating_add(entry.size_bytes))
    }

    pub fn entries(&self) -> &BTreeMap<String, MediaCacheEntry> {
        &self.entries
    }

    pub fn get(&mut self, media_id: &str, now_ms: i64) -> Option<&MediaCacheEntry> {
        let entry = self.entries.get_mut(media_id)?;
        entry.last_accessed_ms = now_ms;
        Some(entry)
    }

    pub fn insert(
        &mut self,
        media: MediaRef,
        local_path: impl Into<String>,
        size_bytes: u64,
        now_ms: i64,
        pinned: bool,
    ) -> Result<Vec<MediaCacheEntry>, MediaCacheError> {
        self.insert_scoped(media, local_path, size_bytes, now_ms, pinned, None, None)
    }

    pub fn insert_scoped(
        &mut self,
        media: MediaRef,
        local_path: impl Into<String>,
        size_bytes: u64,
        now_ms: i64,
        pinned: bool,
        conversation_id: Option<String>,
        topic_id: Option<String>,
    ) -> Result<Vec<MediaCacheEntry>, MediaCacheError> {
        if media.id.trim().is_empty() {
            return Err(MediaCacheError::InvalidMediaId);
        }
        let local_path = local_path.into();
        if local_path.trim().is_empty() {
            return Err(MediaCacheError::InvalidLocalPath);
        }
        if size_bytes > self.max_bytes {
            return Err(MediaCacheError::EntryTooLarge {
                size_bytes,
                max_bytes: self.max_bytes,
            });
        }

        let key = media.id.clone();
        let previous = self.entries.insert(
            key.clone(),
            MediaCacheEntry {
                media,
                local_path,
                size_bytes,
                last_accessed_ms: now_ms,
                pinned,
                conversation_id,
                topic_id,
            },
        );

        match self.evict_to_budget(Some(&key)) {
            Ok(evicted) => Ok(evicted),
            Err(error) => {
                match previous {
                    Some(previous) => {
                        self.entries.insert(key, previous);
                    }
                    None => {
                        self.entries.remove(&key);
                    }
                }
                Err(error)
            }
        }
    }

    pub fn total_bytes_for_conversation(&self, conversation_id: &str) -> u64 {
        self.entries
            .values()
            .filter(|entry| entry.conversation_id.as_deref() == Some(conversation_id))
            .fold(0u64, |total, entry| total.saturating_add(entry.size_bytes))
    }

    pub fn clear_conversation(
        &mut self,
        conversation_id: &str,
        include_pinned: bool,
    ) -> Vec<MediaCacheEntry> {
        let keys_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.conversation_id.as_deref() == Some(conversation_id)
                    && (include_pinned || !entry.pinned)
            })
            .map(|(key, _)| key.clone())
            .collect();

        let mut removed = Vec::with_capacity(keys_to_remove.len());
        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
                removed.push(entry);
            }
        }
        removed
    }

    pub fn clear_topic(
        &mut self,
        conversation_id: &str,
        topic_id: &str,
        include_pinned: bool,
    ) -> Vec<MediaCacheEntry> {
        let keys_to_remove: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.conversation_id.as_deref() == Some(conversation_id)
                    && entry.topic_id.as_deref() == Some(topic_id)
                    && (include_pinned || !entry.pinned)
            })
            .map(|(key, _)| key.clone())
            .collect();

        let mut removed = Vec::with_capacity(keys_to_remove.len());
        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
                removed.push(entry);
            }
        }
        removed
    }

    pub fn remove(&mut self, media_id: &str) -> Option<MediaCacheEntry> {
        self.entries.remove(media_id)
    }

    pub fn set_pinned(&mut self, media_id: &str, pinned: bool) -> Result<(), MediaCacheError> {
        let entry = self
            .entries
            .get_mut(media_id)
            .ok_or_else(|| MediaCacheError::NotFound(media_id.to_string()))?;
        entry.pinned = pinned;
        Ok(())
    }

    fn evict_to_budget(
        &mut self,
        protected_media_id: Option<&str>,
    ) -> Result<Vec<MediaCacheEntry>, MediaCacheError> {
        let mut evicted = Vec::new();
        while self.total_bytes() > self.max_bytes {
            let candidate = self
                .entries
                .values()
                .filter(|entry| !entry.pinned)
                .filter(|entry| protected_media_id != Some(entry.media.id.as_str()))
                .min_by_key(|entry| (entry.last_accessed_ms, entry.media.id.clone()))
                .map(|entry| entry.media.id.clone());
            let Some(candidate) = candidate else {
                return Err(MediaCacheError::PinnedEntriesExceedBudget);
            };
            if let Some(entry) = self.entries.remove(&candidate) {
                evicted.push(entry);
            }
        }
        Ok(evicted)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MediaCacheError {
    #[error("media cache byte budget must be greater than zero")]
    InvalidBudget,
    #[error("media cache entry requires a media id")]
    InvalidMediaId,
    #[error("media cache entry requires a local path")]
    InvalidLocalPath,
    #[error("media cache entry is {size_bytes} bytes but budget is {max_bytes} bytes")]
    EntryTooLarge { size_bytes: u64, max_bytes: u64 },
    #[error("media cache pinned/protected entries exceed the configured byte budget")]
    PinnedEntriesExceedBudget,
    #[error("media cache entry {0} was not found")]
    NotFound(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_media(id: &str) -> MediaRef {
        MediaRef {
            id: id.to_string(),
            file_name: Some(format!("{id}.bin")),
            mime_type: Some("application/octet-stream".into()),
            size_bytes: Some(100),
            width: None,
            height: None,
            duration_ms: None,
            thumbnail_id: None,
            local_path: None,
            remote_url: None,
            content_hash: None,
        }
    }

    #[test]
    fn scoped_insert_and_accounting_by_conversation_and_topic() {
        let mut cache = MediaCache::new(1000).unwrap();

        cache
            .insert_scoped(
                test_media("m1"),
                "/cache/m1",
                100,
                1000,
                false,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m2"),
                "/cache/m2",
                200,
                1001,
                false,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m3"),
                "/cache/m3",
                300,
                1002,
                true,
                Some("conv1".into()),
                Some("topicB".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m4"),
                "/cache/m4",
                150,
                1003,
                false,
                Some("conv2".into()),
                None,
            )
            .unwrap();
        cache
            .insert(test_media("m5"), "/cache/m5", 50, 1004, false)
            .unwrap();

        assert_eq!(cache.total_bytes(), 800);
        assert_eq!(cache.total_bytes_for_conversation("conv1"), 600);
        assert_eq!(cache.total_bytes_for_conversation("conv2"), 150);
        assert_eq!(cache.total_bytes_for_conversation("conv_unknown"), 0);

        // Verify entry properties
        let m1 = cache.entries().get("m1").unwrap();
        assert_eq!(m1.conversation_id.as_deref(), Some("conv1"));
        assert_eq!(m1.topic_id.as_deref(), Some("topicA"));
        assert!(!m1.pinned);

        let m5 = cache.entries().get("m5").unwrap();
        assert_eq!(m5.conversation_id, None);
        assert_eq!(m5.topic_id, None);
    }

    #[test]
    fn clear_conversation_partitions_pinned_and_unpinned() {
        let mut cache = MediaCache::new(1000).unwrap();

        cache
            .insert_scoped(
                test_media("m1"),
                "/cache/m1",
                100,
                1,
                false,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m2"),
                "/cache/m2",
                200,
                2,
                true,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m3"),
                "/cache/m3",
                300,
                3,
                false,
                Some("conv2".into()),
                None,
            )
            .unwrap();

        // Clear conv1 without including pinned
        let cleared = cache.clear_conversation("conv1", false);
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].media.id, "m1");
        assert!(!cache.entries().contains_key("m1"));
        assert!(cache.entries().contains_key("m2"));
        assert!(cache.entries().contains_key("m3"));
        assert_eq!(cache.total_bytes_for_conversation("conv1"), 200);

        // Clear conv1 including pinned
        let cleared_pinned = cache.clear_conversation("conv1", true);
        assert_eq!(cleared_pinned.len(), 1);
        assert_eq!(cleared_pinned[0].media.id, "m2");
        assert!(!cache.entries().contains_key("m2"));
        assert_eq!(cache.total_bytes_for_conversation("conv1"), 0);

        // conv2 still remains intact
        assert!(cache.entries().contains_key("m3"));
        assert_eq!(cache.total_bytes_for_conversation("conv2"), 300);
    }

    #[test]
    fn clear_topic_partitions_pinned_and_unpinned() {
        let mut cache = MediaCache::new(1000).unwrap();

        cache
            .insert_scoped(
                test_media("m1"),
                "/cache/m1",
                100,
                1,
                false,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m2"),
                "/cache/m2",
                150,
                2,
                true,
                Some("conv1".into()),
                Some("topicA".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m3"),
                "/cache/m3",
                200,
                3,
                false,
                Some("conv1".into()),
                Some("topicB".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m4"),
                "/cache/m4",
                250,
                4,
                false,
                Some("conv2".into()),
                Some("topicA".into()),
            )
            .unwrap();

        // Clear topicA under conv1 with include_pinned = false
        let cleared = cache.clear_topic("conv1", "topicA", false);
        assert_eq!(cleared.len(), 1);
        assert_eq!(cleared[0].media.id, "m1");
        assert!(!cache.entries().contains_key("m1"));
        assert!(cache.entries().contains_key("m2"));
        assert!(cache.entries().contains_key("m3"));
        assert!(cache.entries().contains_key("m4"));

        // Clear topicA under conv1 with include_pinned = true
        let cleared_pinned = cache.clear_topic("conv1", "topicA", true);
        assert_eq!(cleared_pinned.len(), 1);
        assert_eq!(cleared_pinned[0].media.id, "m2");
        assert!(!cache.entries().contains_key("m2"));

        // topicB in conv1 and topicA in conv2 remain
        assert!(cache.entries().contains_key("m3"));
        assert!(cache.entries().contains_key("m4"));
    }

    #[test]
    fn lru_eviction_with_scoped_entries() {
        let mut cache = MediaCache::new(300).unwrap();

        cache
            .insert_scoped(
                test_media("m1"),
                "/cache/m1",
                100,
                10,
                false,
                Some("conv1".into()),
                Some("topic1".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m2"),
                "/cache/m2",
                100,
                20,
                true, // pinned!
                Some("conv1".into()),
                Some("topic2".into()),
            )
            .unwrap();
        cache
            .insert_scoped(
                test_media("m3"),
                "/cache/m3",
                100,
                30,
                false,
                Some("conv2".into()),
                None,
            )
            .unwrap();

        assert_eq!(cache.total_bytes(), 300);

        // Inserting m4 (100 bytes) should trigger eviction.
        // Candidates: m1 (accessed 10), m2 (pinned - protected), m3 (accessed 30).
        // m1 is LRU unpinned, so it must be evicted.
        let evicted = cache
            .insert_scoped(
                test_media("m4"),
                "/cache/m4",
                100,
                40,
                false,
                Some("conv2".into()),
                None,
            )
            .unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].media.id, "m1");
        assert_eq!(evicted[0].conversation_id.as_deref(), Some("conv1"));
        assert_eq!(evicted[0].topic_id.as_deref(), Some("topic1"));
        assert!(!cache.entries().contains_key("m1"));
        assert!(cache.entries().contains_key("m2"));
        assert!(cache.entries().contains_key("m3"));
        assert!(cache.entries().contains_key("m4"));

        // Touch m3 to update its last_accessed_ms to 50
        cache.get("m3", 50);

        // Insert m5 (100 bytes, accessed at 60).
        // Unpinned candidates: m4 (accessed 40), m3 (accessed 50).
        // m4 is now older than m3, so m4 must be evicted.
        let evicted2 = cache
            .insert_scoped(
                test_media("m5"),
                "/cache/m5",
                100,
                60,
                false,
                Some("conv3".into()),
                None,
            )
            .unwrap();
        assert_eq!(evicted2.len(), 1);
        assert_eq!(evicted2[0].media.id, "m4");
        assert_eq!(cache.total_bytes(), 300);
        // m2 (pinned) remained untouched throughout
        assert!(cache.entries().contains_key("m2"));
    }

    #[test]
    fn serde_backward_compatibility() {
        // Old JSON without conversationId and topicId
        let legacy_json = r#"{
            "media": { "id": "legacy_m" },
            "localPath": "/path/to/legacy",
            "sizeBytes": 1024,
            "lastAccessedMs": 1234567,
            "pinned": false
        }"#;
        let entry: MediaCacheEntry = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(entry.conversation_id, None);
        assert_eq!(entry.topic_id, None);

        // Serialize entry without conversationId/topicId: fields should be skipped
        let serialized = serde_json::to_string(&entry).unwrap();
        assert!(!serialized.contains("conversationId"));
        assert!(!serialized.contains("topicId"));

        // New JSON with conversationId and topicId
        let new_entry = MediaCacheEntry {
            media: test_media("new_m"),
            local_path: "/path/to/new".into(),
            size_bytes: 2048,
            last_accessed_ms: 7654321,
            pinned: true,
            conversation_id: Some("conv_123".into()),
            topic_id: Some("topic_456".into()),
        };
        let serialized_new = serde_json::to_string(&new_entry).unwrap();
        assert!(serialized_new.contains(r#""conversationId":"conv_123""#));
        assert!(serialized_new.contains(r#""topicId":"topic_456""#));

        let deserialized: MediaCacheEntry = serde_json::from_str(&serialized_new).unwrap();
        assert_eq!(deserialized, new_entry);
    }
}
