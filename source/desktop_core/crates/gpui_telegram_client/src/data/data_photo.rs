//! Target Symbol: DataPhoto
//! Telegram Desktop 1:1 Submodule: `data/data_photo`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PhotoSize {
    Small,
    Thumbnail,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PhotoSizeVariant {
    pub width: u32,
    pub height: u32,
    pub byte_size: usize,
    pub local_cache_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataPhoto {
    pub id: u64,
    pub date: i64,
    pub dc_id: i32,
    pub access_hash: u64,
    pub file_reference: Vec<u8>,
    pub has_video: bool,
    pub video_start_ts: i64,
    pub video_duration_seconds: Option<u32>,
    pub small: Option<PhotoSizeVariant>,
    pub thumbnail: Option<PhotoSizeVariant>,
    pub large: Option<PhotoSizeVariant>,
    pub inline_thumbnail_bytes: Option<Vec<u8>>,
    pub is_spoiler_revealed: bool,
}

impl Default for DataPhoto {
    fn default() -> Self {
        Self {
            id: 0,
            date: 0,
            dc_id: 1,
            access_hash: 0,
            file_reference: Vec::new(),
            has_video: false,
            video_start_ts: 0,
            video_duration_seconds: None,
            small: None,
            thumbnail: None,
            large: None,
            inline_thumbnail_bytes: None,
            is_spoiler_revealed: false,
        }
    }
}

impl DataPhoto {
    pub fn new(id: u64, date: i64) -> Self {
        Self {
            id,
            date,
            ..Default::default()
        }
    }

    pub fn is_null(&self) -> bool {
        self.id == 0
    }

    pub fn with_remote(mut self, dc_id: i32, access_hash: u64, file_reference: Vec<u8>) -> Self {
        self.dc_id = dc_id;
        self.access_hash = access_hash;
        self.file_reference = file_reference;
        self
    }

    pub fn set_dimensions(&mut self, size: PhotoSize, width: u32, height: u32, byte_size: usize) {
        let variant = PhotoSizeVariant {
            width,
            height,
            byte_size,
            local_cache_path: None,
        };
        match size {
            PhotoSize::Small => self.small = Some(variant),
            PhotoSize::Thumbnail => self.thumbnail = Some(variant),
            PhotoSize::Large => self.large = Some(variant),
        }
    }

    pub fn width(&self) -> u32 {
        if let Some(l) = &self.large {
            l.width
        } else if let Some(t) = &self.thumbnail {
            t.width
        } else if let Some(s) = &self.small {
            s.width
        } else {
            0
        }
    }

    pub fn height(&self) -> u32 {
        if let Some(l) = &self.large {
            l.height
        } else if let Some(t) = &self.thumbnail {
            t.height
        } else if let Some(s) = &self.small {
            s.height
        } else {
            0
        }
    }

    pub fn has_exact(&self, size: PhotoSize) -> bool {
        match size {
            PhotoSize::Small => self.small.is_some(),
            PhotoSize::Thumbnail => self.thumbnail.is_some(),
            PhotoSize::Large => self.large.is_some(),
        }
    }

    pub fn size_dimensions(&self, size: PhotoSize) -> Option<(u32, u32)> {
        match size {
            PhotoSize::Small => self.small.as_ref().map(|v| (v.width, v.height)),
            PhotoSize::Thumbnail => self.thumbnail.as_ref().map(|v| (v.width, v.height)),
            PhotoSize::Large => self.large.as_ref().map(|v| (v.width, v.height)),
        }
    }

    pub fn set_video(&mut self, start_ts: i64, duration: Option<u32>) {
        self.has_video = true;
        self.video_start_ts = start_ts;
        self.video_duration_seconds = duration;
    }
}
