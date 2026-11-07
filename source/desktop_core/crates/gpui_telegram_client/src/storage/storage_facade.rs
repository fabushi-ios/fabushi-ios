//! Target Symbol: StorageFacade
//! Telegram Desktop 1:1 Submodule: `StorageFacade`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use super::storage_shared_media::{SharedMedia, SharedMediaType};
use super::storage_sparse_ids_list::{MsgId, SparseIdsListResult};
use super::storage_user_photos::{PhotoId, UserId, UserPhotos, UserPhotosResult};

#[derive(Debug, Default)]
pub struct StorageFacade {
    shared_media: SharedMedia,
    user_photos: UserPhotos,
}

impl StorageFacade {
    pub fn new() -> Self {
        Self {
            shared_media: SharedMedia::new(),
            user_photos: UserPhotos::new(),
        }
    }

    pub fn shared_media(&self) -> &SharedMedia {
        &self.shared_media
    }

    pub fn shared_media_mut(&mut self) -> &mut SharedMedia {
        &mut self.shared_media
    }

    pub fn user_photos(&self) -> &UserPhotos {
        &self.user_photos
    }

    pub fn user_photos_mut(&mut self) -> &mut UserPhotos {
        &mut self.user_photos
    }

    pub fn query_shared_media(
        &self,
        peer_id: i64,
        topic_root_id: i32,
        monoforum_peer_id: i64,
        media_type: SharedMediaType,
        around_id: MsgId,
        limit_before: usize,
        limit_after: usize,
    ) -> SparseIdsListResult {
        self.shared_media.snapshot(
            peer_id,
            topic_root_id,
            monoforum_peer_id,
            media_type,
            around_id,
            limit_before,
            limit_after,
        )
    }

    pub fn query_user_photos(
        &self,
        user_id: UserId,
        around_id: PhotoId,
        limit_before: usize,
        limit_after: usize,
    ) -> UserPhotosResult {
        self.user_photos.query(user_id, around_id, limit_before, limit_after)
    }
}
