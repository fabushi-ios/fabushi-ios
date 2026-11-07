//! Target Symbol: FileUpload
//! Telegram Desktop 1:1 Submodule: `FileUpload`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::HashMap;

pub const USE_BIG_FILES_FROM: i64 = 30 * 1024 * 1024;
pub const UPLOAD_PART_SIZE: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UploadedMedia {
    pub file_id: u64,
    pub parts_count: usize,
    pub md5_checksum: Vec<u8>,
    pub is_big: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadProgress {
    pub file_id: u64,
    pub offset: i64,
    pub total_size: i64,
}

#[derive(Debug, Clone)]
pub struct UploadEntry {
    pub file_id: u64,
    pub total_size: i64,
    pub uploaded_size: i64,
    pub is_big: bool,
    pub data: Vec<u8>,
    pub parts: Vec<Vec<u8>>,
    pub next_part_index: usize,
    pub paused: bool,
}

impl UploadEntry {
    pub fn new(file_id: u64, data: Vec<u8>) -> Self {
        let total_size = data.len() as i64;
        let is_big = total_size >= USE_BIG_FILES_FROM;
        let parts: Vec<Vec<u8>> = data.chunks(UPLOAD_PART_SIZE).map(|c| c.to_vec()).collect();
        Self {
            file_id,
            total_size,
            uploaded_size: 0,
            is_big,
            data,
            parts,
            next_part_index: 0,
            paused: false,
        }
    }

    pub fn take_next_part(&mut self) -> Option<(usize, Vec<u8>)> {
        if self.paused || self.next_part_index >= self.parts.len() {
            None
        } else {
            let idx = self.next_part_index;
            let part = self.parts[idx].clone();
            self.next_part_index += 1;
            self.uploaded_size += part.len() as i64;
            Some((idx, part))
        }
    }

    pub fn is_finished(&self) -> bool {
        self.next_part_index >= self.parts.len()
    }
}

#[derive(Debug, Default)]
pub struct FileUpload {
    uploads: HashMap<u64, UploadEntry>,
    next_file_id: u64,
}

impl FileUpload {
    pub fn new() -> Self {
        Self {
            uploads: HashMap::new(),
            next_file_id: 1,
        }
    }

    pub fn queue_upload(&mut self, data: Vec<u8>) -> u64 {
        let id = self.next_file_id;
        self.next_file_id += 1;
        self.uploads.insert(id, UploadEntry::new(id, data));
        id
    }

    pub fn pause(&mut self, file_id: u64) {
        if let Some(entry) = self.uploads.get_mut(&file_id) {
            entry.paused = true;
        }
    }

    pub fn resume(&mut self, file_id: u64) {
        if let Some(entry) = self.uploads.get_mut(&file_id) {
            entry.paused = false;
        }
    }

    pub fn cancel(&mut self, file_id: u64) {
        self.uploads.remove(&file_id);
    }

    pub fn progress(&self, file_id: u64) -> Option<UploadProgress> {
        self.uploads.get(&file_id).map(|entry| UploadProgress {
            file_id,
            offset: entry.uploaded_size,
            total_size: entry.total_size,
        })
    }

    pub fn take_part(&mut self, file_id: u64) -> Option<(usize, Vec<u8>)> {
        self.uploads.get_mut(&file_id)?.take_next_part()
    }

    pub fn is_finished(&self, file_id: u64) -> bool {
        self.uploads
            .get(&file_id)
            .map(|e| e.is_finished())
            .unwrap_or(false)
    }
}
