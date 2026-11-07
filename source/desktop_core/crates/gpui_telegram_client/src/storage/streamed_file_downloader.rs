//! Target Symbol: StreamedFileDownloader
//! Telegram Desktop 1:1 Submodule: `StreamedFileDownloader`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::collections::BTreeMap;
use std::sync::Arc;
use parking_lot::RwLock;

#[derive(Debug, Clone, Default)]
pub struct StreamedFileDownloader {
    total_size: i64,
    parts: Arc<RwLock<BTreeMap<i64, Vec<u8>>>>,
    downloaded_bytes: Arc<RwLock<i64>>,
}

impl StreamedFileDownloader {
    pub fn new(total_size: i64) -> Self {
        Self {
            total_size,
            parts: Arc::new(RwLock::new(BTreeMap::new())),
            downloaded_bytes: Arc::new(RwLock::new(0)),
        }
    }

    pub fn feed_part(&self, offset: i64, bytes: Vec<u8>) {
        let len = bytes.len() as i64;
        let mut parts = self.parts.write();
        if !parts.contains_key(&offset) {
            parts.insert(offset, bytes);
            let mut downloaded = self.downloaded_bytes.write();
            *downloaded += len;
        }
    }

    pub fn is_range_ready(&self, offset: i64, size: usize) -> bool {
        let target_end = offset + size as i64;
        let parts = self.parts.read();
        let mut current_offset = offset;

        while current_offset < target_end {
            let mut found = false;
            for (&part_offset, part) in parts.iter() {
                let part_end = part_offset + part.len() as i64;
                if part_offset <= current_offset && current_offset < part_end {
                    current_offset = part_end;
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
        true
    }

    pub fn read_stream(&self, offset: i64, size: usize) -> Option<Vec<u8>> {
        if !self.is_range_ready(offset, size) {
            return None;
        }

        let mut result = Vec::with_capacity(size);
        let parts = self.parts.read();
        let mut needed = size;
        let mut cur = offset;

        while needed > 0 {
            for (&part_offset, part) in parts.iter() {
                let part_end = part_offset + part.len() as i64;
                if part_offset <= cur && cur < part_end {
                    let start_in_part = (cur - part_offset) as usize;
                    let available = part.len() - start_in_part;
                    let to_copy = available.min(needed);
                    result.extend_from_slice(&part[start_in_part..start_in_part + to_copy]);
                    cur += to_copy as i64;
                    needed -= to_copy;
                    break;
                }
            }
        }

        Some(result)
    }

    pub fn total_size(&self) -> i64 {
        self.total_size
    }

    pub fn downloaded_bytes(&self) -> i64 {
        *self.downloaded_bytes.read()
    }
}
