use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_media::chunked_upload::ChunkedUploadSession;
use gpui_telegram_media::range_stream::ByteRange;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

/// Content-Addressable Storage (CAS) engine.
///
/// Directory layout:
/// ```text
/// {base_dir}/
/// ├── staging/
/// │   ├── {upload_id}.part
/// │   └── {upload_id}.meta.json
/// └── cas/
///     └── {hash[0..2]}/
///         └── {hash[2..4]}/
///             └── {hash}
/// ```
#[derive(Debug, Clone)]
pub struct CasStore {
    base_dir: PathBuf,
}

impl CasStore {
    /// Creates a CAS store rooted at the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        Self { base_dir }
    }

    /// Creates an isolated temporary CAS store (ideal for tests).
    pub fn temp(prefix: &str) -> Result<Self> {
        let tmp = std::env::temp_dir().join(format!("gpui_telegram_cas_{}_{}", prefix, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp)
            .map_err(|e| TelegramError::Internal(format!("Failed to create temp CAS dir: {e}")))?;
        Ok(Self::new(tmp))
    }

    /// Base storage directory path.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Staging directory where chunked uploads reside before promotion.
    pub fn staging_dir(&self) -> PathBuf {
        self.base_dir.join("staging")
    }

    /// CAS root directory.
    pub fn cas_dir(&self) -> PathBuf {
        self.base_dir.join("cas")
    }

    /// Returns the deterministic filesystem path for a given SHA-256 hash.
    /// Hierarchy: `cas/{hash[0..2]}/{hash[2..4]}/{hash}`
    pub fn cas_path(&self, sha256_hash: &str) -> PathBuf {
        let clean = sha256_hash.to_ascii_lowercase();
        if clean.len() < 4 {
            return self.cas_dir().join(&clean);
        }
        let prefix1 = &clean[0..2];
        let prefix2 = &clean[2..4];
        self.cas_dir().join(prefix1).join(prefix2).join(&clean)
    }

    /// Resolves the file path if the blob exists in CAS.
    pub fn resolve_cas_path(&self, sha256_hash: &str) -> Option<PathBuf> {
        let path = self.cas_path(sha256_hash);
        if path.is_file() {
            Some(path)
        } else {
            None
        }
    }

    /// Checks if a blob exists in the CAS hierarchy.
    pub fn cas_file_exists(&self, sha256_hash: &str) -> bool {
        self.resolve_cas_path(sha256_hash).is_some()
    }

    /// Staging part file path: `staging/{upload_id}.part`.
    pub fn staging_part_path(&self, upload_id: &str) -> PathBuf {
        self.staging_dir().join(format!("{upload_id}.part"))
    }

    /// Staging metadata path: `staging/{upload_id}.meta.json`.
    pub fn staging_meta_path(&self, upload_id: &str) -> PathBuf {
        self.staging_dir().join(format!("{upload_id}.meta.json"))
    }

    /// Directly stores a raw byte slice into CAS and returns its SHA-256 digest.
    pub async fn store_blob(&self, data: &[u8]) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = format!("{:x}", hasher.finalize());

        let target_path = self.cas_path(&hash);
        if target_path.is_file() {
            // Deduplication: already exists in CAS
            return Ok(hash);
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| TelegramError::Internal(format!("Failed to create CAS parent dir: {e}")))?;
        }

        // Write atomically via a temporary file in staging
        let staging_dir = self.staging_dir();
        fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to create staging dir: {e}")))?;

        let temp_path = staging_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        fs::write(&temp_path, data)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to write blob to temp file: {e}")))?;

        // Atomic rename into CAS
        fs::rename(&temp_path, &target_path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to promote temp file to CAS: {e}")))?;

        Ok(hash)
    }

    /// Synchronously stores a raw byte slice into CAS (useful in tests and sync contexts).
    pub fn store_blob_sync(&self, data: &[u8]) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = format!("{:x}", hasher.finalize());

        let target_path = self.cas_path(&hash);
        if target_path.is_file() {
            return Ok(hash);
        }

        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TelegramError::Internal(format!("Failed to create CAS parent dir: {e}")))?;
        }

        let staging_dir = self.staging_dir();
        std::fs::create_dir_all(&staging_dir)
            .map_err(|e| TelegramError::Internal(format!("Failed to create staging dir: {e}")))?;

        let temp_path = staging_dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&temp_path, data)
            .map_err(|e| TelegramError::Internal(format!("Failed to write blob to temp file: {e}")))?;

        std::fs::rename(&temp_path, &target_path)
            .map_err(|e| TelegramError::Internal(format!("Failed to promote temp file to CAS: {e}")))?;

        Ok(hash)
    }

    /// Reads an entire blob from CAS into memory.
    pub async fn read_blob(&self, sha256_hash: &str) -> Result<Vec<u8>> {
        let path = self
            .resolve_cas_path(sha256_hash)
            .ok_or_else(|| TelegramError::NotFound(format!("Blob {sha256_hash} not found in CAS")))?;
        fs::read(&path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to read CAS blob: {e}")))
    }

    /// Initializes a new chunked upload session and creates staging files.
    pub async fn init_chunked_upload(
        &self,
        upload_id: &str,
        total_size: u64,
        part_size: u32,
        expected_sha256: Option<String>,
    ) -> Result<ChunkedUploadSession> {
        let mut session = ChunkedUploadSession::new(upload_id, total_size, part_size)?;
        if let Some(exp) = expected_sha256 {
            session = session.with_expected_sha256(exp);
        }

        let staging_dir = self.staging_dir();
        fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to create staging dir: {e}")))?;

        // Pre-allocate or create empty staging part file
        let part_path = self.staging_part_path(upload_id);
        let file = File::create(&part_path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to create staging part file: {e}")))?;
        if total_size > 0 {
            file.set_len(total_size)
                .await
                .map_err(|e| TelegramError::Internal(format!("Failed to pre-allocate part file: {e}")))?;
        }

        // Save serialized session metadata
        let meta_path = self.staging_meta_path(upload_id);
        let meta_json = serde_json::to_vec_pretty(&session)
            .map_err(|e| TelegramError::Internal(format!("Failed to serialize session meta: {e}")))?;
        fs::write(&meta_path, meta_json)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to write session meta: {e}")))?;

        Ok(session)
    }

    /// Retrieves an active chunked upload session from staging metadata.
    pub async fn get_upload_session(&self, upload_id: &str) -> Result<ChunkedUploadSession> {
        let meta_path = self.staging_meta_path(upload_id);
        if !meta_path.is_file() {
            return Err(TelegramError::NotFound(format!("Upload session {upload_id} not found")));
        }

        let bytes = fs::read(&meta_path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to read session meta: {e}")))?;
        let session: ChunkedUploadSession = serde_json::from_slice(&bytes)
            .map_err(|e| TelegramError::Internal(format!("Failed to deserialize session meta: {e}")))?;

        Ok(session)
    }

    /// Ingests a single chunk at `chunk_index`, writing directly to the seekable `.part` file.
    pub async fn write_chunk(&self, upload_id: &str, chunk_index: u32, data: &[u8]) -> Result<bool> {
        let mut session = self.get_upload_session(upload_id).await?;

        if chunk_index >= session.total_parts {
            return Err(TelegramError::Validation(format!(
                "Chunk index {chunk_index} out of bounds for total {}",
                session.total_parts
            )));
        }

        let expected_size = session.expected_part_size(chunk_index);
        if data.len() != expected_size {
            return Err(TelegramError::Validation(format!(
                "Chunk index {chunk_index} size mismatch: got {} bytes, expected {} bytes",
                data.len(),
                expected_size
            )));
        }

        let part_path = self.staging_part_path(upload_id);
        let mut file = OpenOptions::new()
            .write(true)
            .open(&part_path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to open staging part file: {e}")))?;

        let offset = (chunk_index as u64) * (session.part_size as u64);
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to seek part file: {e}")))?;
        file.write_all(data)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to write chunk: {e}")))?;
        file.flush()
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to flush part file: {e}")))?;

        session.mark_part_received(chunk_index)?;

        // Update staging metadata
        let meta_path = self.staging_meta_path(upload_id);
        let meta_json = serde_json::to_vec_pretty(&session)
            .map_err(|e| TelegramError::Internal(format!("Failed to serialize session meta: {e}")))?;
        fs::write(&meta_path, meta_json)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to update session meta: {e}")))?;

        Ok(session.is_complete())
    }

    /// Finalizes a completed chunked upload: computes SHA-256, verifies expected hash,
    /// promotes the `.part` file atomically to the CAS directory, and cleans up staging metadata.
    pub async fn complete_chunked_upload(&self, upload_id: &str) -> Result<String> {
        let session = self.get_upload_session(upload_id).await?;

        if !session.is_complete() {
            let missing = session.missing_parts();
            return Err(TelegramError::Validation(format!(
                "Upload session {upload_id} incomplete: missing {} parts ({:?})",
                missing.len(),
                missing
            )));
        }

        let part_path = self.staging_part_path(upload_id);
        let mut file = File::open(&part_path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to open staging file for hashing: {e}")))?;

        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = file
                .read(&mut buffer)
                .await
                .map_err(|e| TelegramError::Internal(format!("Failed to stream staging file: {e}")))?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        let computed_hash = format!("{:x}", hasher.finalize());

        // Validate against expected SHA-256 if provided by the client
        if let Some(ref expected) = session.expected_sha256 {
            if !computed_hash.eq_ignore_ascii_case(expected) {
                return Err(TelegramError::Validation(format!(
                    "SHA-256 checksum mismatch: computed {}, expected {}",
                    computed_hash, expected
                )));
            }
        }

        // Promote to CAS directory atomically
        let target_path = self.cas_path(&computed_hash);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| TelegramError::Internal(format!("Failed to create CAS target parent dir: {e}")))?;
        }

        if target_path.is_file() {
            // Already exists in CAS (deduplication) - remove staging file
            let _ = fs::remove_file(&part_path).await;
        } else {
            fs::rename(&part_path, &target_path)
                .await
                .map_err(|e| TelegramError::Internal(format!("Failed to promote staging part to CAS: {e}")))?;
        }

        // Clean up metadata
        let meta_path = self.staging_meta_path(upload_id);
        let _ = fs::remove_file(&meta_path).await;

        Ok(computed_hash)
    }

    /// Reads a specific byte range directly from a CAS blob.
    pub async fn read_range(&self, sha256_hash: &str, range: &ByteRange) -> Result<Vec<u8>> {
        let path = self
            .resolve_cas_path(sha256_hash)
            .ok_or_else(|| TelegramError::NotFound(format!("Blob {sha256_hash} not found in CAS")))?;

        let mut file = File::open(&path)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to open CAS file: {e}")))?;

        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to seek in CAS file: {e}")))?;

        let length = range.length() as usize;
        let mut buffer = vec![0u8; length];
        file.read_exact(&mut buffer)
            .await
            .map_err(|e| TelegramError::Internal(format!("Failed to read byte range from CAS: {e}")))?;

        Ok(buffer)
    }
}
