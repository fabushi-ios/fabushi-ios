//! Target Symbol: DataPhotoMedia
//! Telegram Desktop 1:1 Submodule: `data/data_photo_media`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhotoMediaStage {
    None,
    BlurhashReady,
    ThumbnailReady,
    FullReady,
    Failed,
}

impl Default for PhotoMediaStage {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataPhotoMedia {
    pub photo_id: u64,
    pub stage: PhotoMediaStage,
    pub image_bytes: Option<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub blurhash: Option<String>,
}

impl Default for DataPhotoMedia {
    fn default() -> Self {
        Self {
            photo_id: 0,
            stage: PhotoMediaStage::None,
            image_bytes: None,
            width: 0,
            height: 0,
            blurhash: None,
        }
    }
}

impl DataPhotoMedia {
    pub fn new(photo_id: u64) -> Self {
        Self {
            photo_id,
            ..Default::default()
        }
    }

    pub fn set_blurhash(&mut self, hash: impl Into<String>) {
        self.blurhash = Some(hash.into());
        if self.stage == PhotoMediaStage::None {
            self.stage = PhotoMediaStage::BlurhashReady;
        }
    }

    pub fn set_image(&mut self, bytes: Vec<u8>, width: u32, height: u32, is_thumbnail: bool) {
        self.image_bytes = Some(bytes);
        self.width = width;
        self.height = height;
        self.stage = if is_thumbnail {
            PhotoMediaStage::ThumbnailReady
        } else {
            PhotoMediaStage::FullReady
        };
    }

    pub fn set_failed(&mut self) {
        self.stage = PhotoMediaStage::Failed;
    }

    pub fn is_ready(&self) -> bool {
        self.stage == PhotoMediaStage::FullReady || self.stage == PhotoMediaStage::ThumbnailReady
    }
}
