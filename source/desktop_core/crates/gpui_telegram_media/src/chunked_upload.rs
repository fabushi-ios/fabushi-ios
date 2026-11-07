use gpui_telegram_core::error::{Result, TelegramError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const BIG_FILE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10MB in Telegram Desktop
pub const PART_SIZE_32KB: u32 = 32 * 1024;
pub const PART_SIZE_64KB: u32 = 64 * 1024;
pub const PART_SIZE_128KB: u32 = 128 * 1024;
pub const PART_SIZE_256KB: u32 = 256 * 1024;
pub const PART_SIZE_512KB: u32 = 512 * 1024;
pub const PART_SIZE_1MB: u32 = 1024 * 1024;

/// Recommends the optimal Telegram upload chunk part size based on file size.
/// Upstream reference: `Telegram/SourceFiles/storage/file_upload.cpp` lines 40–100.
pub fn recommend_part_size(file_size: u64) -> u32 {
    const MB: u64 = 1024 * 1024;
    if file_size < 1 * MB {
        PART_SIZE_32KB
    } else if file_size < 32 * MB {
        PART_SIZE_64KB
    } else if file_size < 375 * MB {
        PART_SIZE_128KB
    } else if file_size < 750 * MB {
        PART_SIZE_256KB
    } else if file_size < 2000 * MB {
        PART_SIZE_512KB
    } else {
        PART_SIZE_1MB
    }
}

/// Represents a resilient, chunked media upload session with bitmask receipt tracking,
/// out-of-order part ingestion, integrity validation, and pause/resume persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkedUploadSession {
    pub file_id: String,
    pub total_size: u64,
    pub part_size: u32,
    pub total_parts: u32,
    pub uploaded_parts: Vec<bool>,
    pub parts_data: BTreeMap<u32, Vec<u8>>,
    pub expected_sha256: Option<String>,
    pub is_big_file: bool,
}

impl ChunkedUploadSession {
    /// Creates a new chunked upload session for the given file.
    pub fn new(file_id: impl Into<String>, total_size: u64, part_size: u32) -> Result<Self> {
        if part_size == 0 {
            return Err(TelegramError::Validation("part_size must be > 0".into()));
        }

        let total_parts = if total_size == 0 {
            1
        } else {
            ((total_size as f64) / (part_size as f64)).ceil() as u32
        };

        let is_big_file = total_size > BIG_FILE_THRESHOLD;
        let file_id_str = file_id.into();

        Ok(Self {
            file_id: file_id_str,
            total_size,
            part_size,
            total_parts,
            uploaded_parts: vec![false; total_parts as usize],
            parts_data: BTreeMap::new(),
            expected_sha256: None,
            is_big_file,
        })
    }

    /// Sets the expected SHA-256 hex digest for end-to-end verification.
    pub fn with_expected_sha256(mut self, sha256_hex: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256_hex.into());
        self
    }

    /// Alias for file_id for backward compatibility with upload_id.
    pub fn upload_id(&self) -> &str {
        &self.file_id
    }

    /// Returns the expected byte count for a given part index.
    pub fn expected_part_size(&self, part_index: u32) -> usize {
        if part_index >= self.total_parts {
            return 0;
        }
        if self.total_size == 0 {
            return 0;
        }
        if part_index == self.total_parts - 1 {
            let remainder = (self.total_size % (self.part_size as u64)) as usize;
            if remainder == 0 {
                self.part_size as usize
            } else {
                remainder
            }
        } else {
            self.part_size as usize
        }
    }

    /// Ingests a media part out-of-order or in-order, validating bounds and byte count.
    pub fn receive_part(&mut self, part_index: u32, data: &[u8]) -> Result<()> {
        if part_index >= self.total_parts {
            return Err(TelegramError::Validation(format!(
                "Part index {} out of bounds for total {}",
                part_index, self.total_parts
            )));
        }

        let expected_len = self.expected_part_size(part_index);
        if data.len() != expected_len {
            return Err(TelegramError::Validation(format!(
                "Part index {} has incorrect size: got {} bytes, expected {} bytes",
                part_index,
                data.len(),
                expected_len
            )));
        }

        self.uploaded_parts[part_index as usize] = true;
        self.parts_data.insert(part_index, data.to_vec());
        Ok(())
    }

    /// Marks a part index as received without buffering raw payload (e.g. streaming to disk).
    pub fn mark_part_received(&mut self, part_index: u32) -> Result<()> {
        if part_index >= self.total_parts {
            return Err(TelegramError::Validation(format!(
                "Part index {} out of bounds for total {}",
                part_index, self.total_parts
            )));
        }
        self.uploaded_parts[part_index as usize] = true;
        Ok(())
    }

    /// Legacy alias for mark_part_received.
    pub fn mark_chunk_received(&mut self, chunk_index: u32) -> Result<()> {
        self.mark_part_received(chunk_index)
    }

    /// Checks if a specific part has been successfully received.
    pub fn is_part_received(&self, part_index: u32) -> bool {
        if (part_index as usize) < self.uploaded_parts.len() {
            self.uploaded_parts[part_index as usize]
        } else {
            false
        }
    }

    /// Legacy alias for is_part_received.
    pub fn is_chunk_received(&self, chunk_index: u32) -> bool {
        self.is_part_received(chunk_index)
    }

    /// Returns a list of all missing part indices that must be requested/re-sent.
    pub fn missing_parts(&self) -> Vec<u32> {
        let mut missing = Vec::new();
        for i in 0..self.total_parts {
            if !self.is_part_received(i) {
                missing.push(i);
            }
        }
        missing
    }

    /// Legacy alias for missing_parts.
    pub fn missing_chunks(&self) -> Vec<u32> {
        self.missing_parts()
    }

    /// Returns whether all parts have been received.
    pub fn is_complete(&self) -> bool {
        self.uploaded_parts.iter().all(|&r| r)
    }

    /// Assembles all received parts into the complete file byte vector.
    pub fn assemble_payload(&self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            let missing = self.missing_parts();
            return Err(TelegramError::Validation(format!(
                "Cannot assemble incomplete payload: missing {} parts ({:?})",
                missing.len(),
                missing
            )));
        }

        let mut assembled = Vec::with_capacity(self.total_size as usize);
        for i in 0..self.total_parts {
            match self.parts_data.get(&i) {
                Some(chunk) => assembled.extend_from_slice(chunk),
                None => {
                    return Err(TelegramError::Validation(format!(
                        "Buffered part {} missing during payload assembly",
                        i
                    )))
                }
            }
        }

        if assembled.len() != self.total_size as usize {
            return Err(TelegramError::Validation(format!(
                "Assembled size mismatch: got {} bytes, expected {} bytes",
                assembled.len(),
                self.total_size
            )));
        }

        Ok(assembled)
    }

    /// Computes the SHA-256 hash of the fully assembled payload.
    pub fn compute_sha256(&self) -> Result<String> {
        let assembled = self.assemble_payload()?;
        let mut hasher = Sha256::new();
        hasher.update(&assembled);
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Validates completion and checks against expected SHA-256 if present.
    pub fn finalize_and_verify(&self) -> Result<bool> {
        if !self.is_complete() {
            return Ok(false);
        }
        if let Some(ref expected) = self.expected_sha256 {
            let computed = self.compute_sha256()?;
            Ok(computed.eq_ignore_ascii_case(expected))
        } else {
            Ok(true)
        }
    }

    /// Standalone SHA-256 verification utility.
    pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let result = hasher.finalize();
        let computed = format!("{:x}", result);
        computed.eq_ignore_ascii_case(expected_hex)
    }
}
