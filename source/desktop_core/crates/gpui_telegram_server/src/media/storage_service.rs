use crate::media::cas_store::CasStore;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_media::range_stream::ByteRange;
use std::path::PathBuf;

/// High-level media storage service integrating Content-Addressable Storage (CAS).
#[derive(Debug, Clone)]
pub struct MediaStorageService {
    cas: CasStore,
}

impl MediaStorageService {
    /// Initializes a media storage service with default path `./data`.
    pub fn new() -> Self {
        let base_dir = std::env::var("MEDIA_STORAGE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data"));
        Self {
            cas: CasStore::new(base_dir),
        }
    }

    /// Initializes with an explicit CAS store instance.
    pub fn with_cas_store(cas: CasStore) -> Self {
        Self { cas }
    }

    /// Accessor to the underlying CAS store.
    pub fn cas(&self) -> &CasStore {
        &self.cas
    }

    /// Stores raw media blob under its SHA-256 hash.
    pub fn store(&self, _sha256_hash: String, data: Vec<u8>) -> Result<()> {
        self.cas.store_blob_sync(&data)?;
        Ok(())
    }

    /// Fetches a byte range [start..end] from a stored blob.
    pub fn fetch_range(&self, sha256_hash: &str, start: usize, end: usize) -> Result<Vec<u8>> {
        let path = self
            .cas
            .resolve_cas_path(sha256_hash)
            .ok_or_else(|| TelegramError::NotFound(format!("Blob {sha256_hash} not found in CAS")))?;
        let blob = std::fs::read(&path)
            .map_err(|e| TelegramError::Internal(format!("Failed to read CAS blob: {e}")))?;

        let actual_end = end.min(blob.len());
        if start > actual_end {
            return Err(TelegramError::Validation("Start exceeds range".into()));
        }
        Ok(blob[start..actual_end].to_vec())
    }

    /// Async fetch of a RFC 7233 byte range.
    pub async fn fetch_byte_range(&self, sha256_hash: &str, range: &ByteRange) -> Result<Vec<u8>> {
        self.cas.read_range(sha256_hash, range).await
    }
}

impl Default for MediaStorageService {
    fn default() -> Self {
        Self::new()
    }
}
