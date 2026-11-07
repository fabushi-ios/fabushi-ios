//! Target Symbol: StorageSharedMedia
//! Telegram Desktop 1:1 Submodule: `StorageSharedMedia`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::HashMap;
use super::storage_sparse_ids_list::{MsgId, MsgRange, SparseIdsList, SparseIdsListQuery, SparseIdsListResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SharedMediaType {
    Photo = 0,
    Video = 1,
    PhotoVideo = 2,
    MusicFile = 3,
    File = 4,
    VoiceFile = 5,
    Link = 6,
    ChatPhoto = 7,
    RoundVoiceFile = 8,
    GIF = 9,
    RoundFile = 10,
    Pinned = 11,
    Poll = 12,
}

pub const SHARED_MEDIA_TYPE_COUNT: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SharedMediaKey {
    pub peer_id: i64,
    pub topic_root_id: i32,
    pub monoforum_peer_id: i64,
    pub media_type: SharedMediaType,
    pub message_id: MsgId,
}

impl SharedMediaKey {
    pub fn new(
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        message_id: MsgId,
    ) -> Self {
        Self {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
            media_type,
            message_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerTopicKey {
    pub peer_id: i64,
    pub topic_root_id: i32,
    pub monoforum_peer_id: i64,
}

#[derive(Debug, Default)]
pub struct SharedMedia {
    indices: HashMap<PeerTopicKey, [SparseIdsList; SHARED_MEDIA_TYPE_COUNT]>,
}

impl SharedMedia {
    pub fn new() -> Self {
        Self {
            indices: HashMap::new(),
        }
    }

    fn get_or_create(
        &mut self,
        key: &PeerTopicKey,
    ) -> &mut [SparseIdsList; SHARED_MEDIA_TYPE_COUNT] {
        self.indices.entry(key.clone()).or_insert_with(|| {
            std::array::from_fn(|_| SparseIdsList::new())
        })
    }

    pub fn add_new(
        &mut self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        message_id: MsgId,
    ) {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        let lists = self.get_or_create(&key);
        lists[media_type as usize].add_new(message_id);
    }

    pub fn add_slice(
        &mut self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        message_ids: Vec<MsgId>,
        no_skip_range: MsgRange,
        count: Option<usize>,
    ) {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        let lists = self.get_or_create(&key);
        lists[media_type as usize].add_slice(message_ids, no_skip_range, count);
    }

    pub fn remove_one(
        &mut self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        message_id: MsgId,
    ) {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        if let Some(lists) = self.indices.get_mut(&key) {
            lists[media_type as usize].remove_one(message_id);
        }
    }

    pub fn remove_all(&mut self, peer_id: i64, topic_root_id: i32, monoforum_peer_id: i64) {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        if let Some(lists) = self.indices.get_mut(&key) {
            for list in lists.iter_mut() {
                list.remove_all();
            }
        }
    }

    pub fn snapshot(
        &self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        around_id: MsgId,
        limit_before: usize,
        limit_after: usize,
    ) -> SparseIdsListResult {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        if let Some(lists) = self.indices.get(&key) {
            lists[media_type as usize].snapshot(&SparseIdsListQuery::new(
                around_id,
                limit_before,
                limit_after,
            ))
        } else {
            SparseIdsListResult::default()
        }
    }

    pub fn is_empty(
        &self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
    ) -> bool {
        let key = PeerTopicKey {
            peer_id,
            topic_root_id,
            monoforum_peer_id,
        };
        self.indices
            .get(&key)
            .map(|lists| lists[media_type as usize].is_empty())
            .unwrap_or(true)
    }
}

#[derive(Debug, Default)]
pub struct StorageSharedMedia {
    pub media: SharedMedia,
}

impl StorageSharedMedia {
    pub fn new() -> Self {
        Self {
            media: SharedMedia::new(),
        }
    }

    pub fn add(&mut self, peer_id: i64, media_type: SharedMediaType, message_id: MsgId) {
        self.media.add_new(peer_id, 0, 0, media_type, message_id);
    }

    pub fn query(
        &self,
        peer_id: i64,
        media_type: SharedMediaType,
        around_id: MsgId,
        limit_before: usize,
        limit_after: usize,
    ) -> SparseIdsListResult {
        self.media.snapshot(peer_id, 0, 0, media_type, around_id, limit_before, limit_after)
    }
}
