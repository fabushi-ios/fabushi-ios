//! Target Symbol: StorageUserPhotos
//! Telegram Desktop 1:1 Submodule: `StorageUserPhotos`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::{HashMap, VecDeque};

pub type PhotoId = u64;
pub type UserId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct UserPhotosKey {
    pub user_id: UserId,
    pub photo_id: PhotoId,
    pub back: bool,
}

impl UserPhotosKey {
    pub fn new(user_id: UserId, photo_id: PhotoId) -> Self {
        Self {
            user_id,
            photo_id,
            back: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserPhotosResult {
    pub count: Option<usize>,
    pub skipped_before: Option<usize>,
    pub skipped_after: usize,
    pub photo_ids: Vec<PhotoId>,
}

#[derive(Debug, Clone, Default)]
struct UserPhotoList {
    count: Option<usize>,
    photo_ids: VecDeque<PhotoId>,
    back_photo_id: Option<PhotoId>,
}

impl UserPhotoList {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            count: None,
            photo_ids: VecDeque::new(),
            back_photo_id: None,
        }
    }

    pub fn set_back(&mut self, photo_id: PhotoId) {
        self.back_photo_id = Some(photo_id);
    }

    pub fn add_new(&mut self, photo_id: PhotoId) {
        if !self.photo_ids.contains(&photo_id) {
            self.photo_ids.push_front(photo_id);
            if let Some(c) = self.count.as_mut() {
                *c += 1;
            }
        }
    }

    pub fn add_slice(&mut self, photo_ids: Vec<PhotoId>, count: Option<usize>) {
        if let Some(c) = count {
            self.count = Some(c);
        }
        for id in photo_ids {
            if !self.photo_ids.contains(&id) {
                self.photo_ids.push_back(id);
            }
        }
    }

    pub fn remove_one(&mut self, photo_id: PhotoId) {
        if let Some(pos) = self.photo_ids.iter().position(|&id| id == photo_id) {
            self.photo_ids.remove(pos);
            if let Some(c) = self.count.as_mut() {
                *c = c.saturating_sub(1);
            }
        }
    }

    pub fn replace(&mut self, old_id: PhotoId, new_id: PhotoId) {
        if let Some(pos) = self.photo_ids.iter().position(|&id| id == old_id) {
            self.photo_ids[pos] = new_id;
        }
    }

    pub fn query(
        &self,
        around_id: PhotoId,
        limit_before: usize,
        limit_after: usize,
    ) -> UserPhotosResult {
        if self.photo_ids.is_empty() {
            return UserPhotosResult {
                count: self.count,
                skipped_before: Some(0),
                skipped_after: 0,
                photo_ids: Vec::new(),
            };
        }

        let idx = self.photo_ids.iter().position(|&id| id == around_id).unwrap_or(0);
        let start = idx.saturating_sub(limit_before);
        let end = (idx + limit_after + 1).min(self.photo_ids.len());

        let slice: Vec<PhotoId> = self.photo_ids.range(start..end).copied().collect();
        let skipped_before = Some(start);
        let skipped_after = self.photo_ids.len().saturating_sub(end);

        UserPhotosResult {
            count: self.count,
            skipped_before,
            skipped_after,
            photo_ids: slice,
        }
    }
}

#[derive(Debug, Default)]
pub struct UserPhotos {
    lists: HashMap<UserId, UserPhotoList>,
}

impl UserPhotos {
    pub fn new() -> Self {
        Self {
            lists: HashMap::new(),
        }
    }

    pub fn add_new(&mut self, user_id: UserId, photo_id: PhotoId) {
        self.lists.entry(user_id).or_default().add_new(photo_id);
    }

    pub fn set_back(&mut self, user_id: UserId, photo_id: PhotoId) {
        self.lists.entry(user_id).or_default().set_back(photo_id);
    }

    pub fn add_slice(&mut self, user_id: UserId, photo_ids: Vec<PhotoId>, count: Option<usize>) {
        self.lists.entry(user_id).or_default().add_slice(photo_ids, count);
    }

    pub fn remove_one(&mut self, user_id: UserId, photo_id: PhotoId) {
        if let Some(list) = self.lists.get_mut(&user_id) {
            list.remove_one(photo_id);
        }
    }

    pub fn replace(&mut self, user_id: UserId, old_id: PhotoId, new_id: PhotoId) {
        if let Some(list) = self.lists.get_mut(&user_id) {
            list.replace(old_id, new_id);
        }
    }

    pub fn query(
        &self,
        user_id: UserId,
        around_id: PhotoId,
        limit_before: usize,
        limit_after: usize,
    ) -> UserPhotosResult {
        if let Some(list) = self.lists.get(&user_id) {
            list.query(around_id, limit_before, limit_after)
        } else {
            UserPhotosResult::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct StorageUserPhotos {
    pub user_photos: UserPhotos,
}

impl StorageUserPhotos {
    pub fn new() -> Self {
        Self {
            user_photos: UserPhotos::new(),
        }
    }

    pub fn add_photo(&mut self, user_id: UserId, photo_id: PhotoId) {
        self.user_photos.add_new(user_id, photo_id);
    }

    pub fn query_photos(
        &self,
        user_id: UserId,
        around_id: PhotoId,
        limit_before: usize,
        limit_after: usize,
    ) -> UserPhotosResult {
        self.user_photos.query(user_id, around_id, limit_before, limit_after)
    }
}
