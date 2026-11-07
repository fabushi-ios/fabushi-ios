//! Target Symbol: DataUserPhotos
//! Telegram Desktop 1:1 Submodule: `DataUserPhotos`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::UserId;

/// Key identifying a user photo pagination boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct UserPhotosKey {
    pub user_id: UserId,
    pub photo_id: u64,
}

/// Paginated slice of a user's avatar history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UserPhotosSlice {
    pub key: UserPhotosKey,
    pub photo_ids: Vec<u64>,
    pub full_count: usize,
    pub skipped_before: usize,
    pub skipped_after: usize,
}

impl UserPhotosSlice {
    pub fn new(key: UserPhotosKey, photo_ids: Vec<u64>, full_count: usize) -> Self {
        Self {
            key,
            photo_ids,
            full_count,
            skipped_before: 0,
            skipped_after: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.photo_ids.is_empty()
    }

    pub fn count(&self) -> usize {
        self.photo_ids.len()
    }
}

/// Target Symbol for Module 03 Ledger: DataUserPhotos coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataUserPhotos {
    pub user_slices: Vec<UserPhotosSlice>,
}

impl DataUserPhotos {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_slice(&mut self, slice: UserPhotosSlice) {
        self.user_slices.push(slice);
    }

    pub fn find_for_user(&self, user_id: UserId) -> Option<&UserPhotosSlice> {
        self.user_slices.iter().find(|s| s.key.user_id == user_id)
    }
}
