use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tar::Archive;
use tokio::sync::Mutex;

pub const ARCHIVE_LIMIT_ERROR_CODE: &str = "ARCHIVE_LIMIT_EXCEEDED";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveLimitError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for ArchiveLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ArchiveLimitError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub path: String,
    pub kind: ArchiveEntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveExtractLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_file_size: u64,
}

impl Default for ArchiveExtractLimits {
    fn default() -> Self {
        Self {
            max_files: 10_000,
            max_total_bytes: 100 * 1024 * 1024, // 100 MB
            max_file_size: 50 * 1024 * 1024,    // 50 MB
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractArchiveOptions {
    pub dest_dir: PathBuf,
    pub limits: Option<ArchiveExtractLimits>,
    pub strip_components: usize,
}

/// Helper to decode optionally gzipped tar data
fn get_tar_archive<'a>(data: &'a [u8]) -> Archive<Box<dyn Read + 'a>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let gz = GzDecoder::new(data);
        Archive::new(Box::new(gz) as Box<dyn Read>)
    } else {
        Archive::new(Box::new(data) as Box<dyn Read>)
    }
}

/// Inspect a tar archive, returning summary entries.
pub fn inspect_tar_archive(data: &[u8]) -> Result<Vec<ArchiveEntry>, String> {
    let mut archive = get_tar_archive(data);
    let mut entries_out = Vec::new();

    let entries = archive.entries().map_err(|e| format!("Tar read error: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let path = entry.path().map_err(|e| format!("Path error: {e}"))?;
        let path_str = path.to_string_lossy().to_string();

        let header = entry.header();
        let kind = if header.entry_type().is_dir() {
            ArchiveEntryKind::Directory
        } else if header.entry_type().is_symlink() {
            ArchiveEntryKind::Symlink
        } else if header.entry_type().is_file() {
            ArchiveEntryKind::File
        } else {
            ArchiveEntryKind::Other
        };

        entries_out.push(ArchiveEntry {
            path: path_str,
            kind,
            size: entry.size(),
        });
    }

    Ok(entries_out)
}

/// Read a single file entry from a tar archive into memory.
pub fn read_archive_entry(data: &[u8], entry_path: &str) -> Result<Option<Vec<u8>>, String> {
    let mut archive = get_tar_archive(data);
    let normalized_target = entry_path.trim_start_matches("./").trim_start_matches('/');

    let entries = archive.entries().map_err(|e| format!("Tar read error: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let path = entry.path().map_err(|e| format!("Path error: {e}"))?;
        let path_str = path.to_string_lossy().to_string();
        let normalized = path_str.trim_start_matches("./").trim_start_matches('/');

        if normalized == normalized_target {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| format!("Read payload error: {e}"))?;
            return Ok(Some(buf));
        }
    }

    Ok(None)
}

/// Extract an archive with bounds and security limits.
pub fn extract_archive(data: &[u8], options: &ExtractArchiveOptions) -> Result<usize, String> {
    let mut archive = get_tar_archive(data);
    let limits = options.limits.clone().unwrap_or_default();

    let mut total_files = 0;
    let mut total_bytes = 0u64;

    let entries = archive.entries().map_err(|e| format!("Tar read error: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let path = entry.path().map_err(|e| format!("Path error: {e}"))?;

        // Path stripping
        let components: Vec<_> = path.components().collect();
        if components.len() <= options.strip_components {
            continue;
        }

        let mut stripped_path = PathBuf::new();
        for comp in &components[options.strip_components..] {
            use std::path::Component;
            match comp {
                Component::Normal(p) => stripped_path.push(p),
                Component::ParentDir => {
                    return Err(format!("Directory traversal forbidden: {:?}", path));
                }
                _ => {}
            }
        }

        if stripped_path.as_os_str().is_empty() {
            continue;
        }

        let dest = options.dest_dir.join(&stripped_path);
        let header = entry.header();

        if header.entry_type().is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| format!("Failed to create dir: {e}"))?;
            continue;
        }

        total_files += 1;
        if total_files > limits.max_files {
            return Err(ArchiveLimitError {
                code: ARCHIVE_LIMIT_ERROR_CODE,
                message: format!("Exceeded max files limit {}", limits.max_files),
            }
            .to_string());
        }

        let size = entry.size();
        if size > limits.max_file_size {
            return Err(ArchiveLimitError {
                code: ARCHIVE_LIMIT_ERROR_CODE,
                message: format!("File size {} exceeds limit {}", size, limits.max_file_size),
            }
            .to_string());
        }

        total_bytes += size;
        if total_bytes > limits.max_total_bytes {
            return Err(ArchiveLimitError {
                code: ARCHIVE_LIMIT_ERROR_CODE,
                message: format!("Total bytes {} exceeds limit {}", total_bytes, limits.max_total_bytes),
            }
            .to_string());
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Parent dir error: {e}"))?;
        }

        entry.unpack(&dest).map_err(|e| format!("Unpack failed: {e}"))?;
    }

    Ok(total_files)
}

/// Canonicalize base64 string by trimming, removing whitespace/newlines, padding, and verifying validity.
pub fn canonicalize_base64(raw: &str) -> Result<String, String> {
    let mut cleaned: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(cleaned);
    }

    // Pad to multiple of 4
    let rem = cleaned.len() % 4;
    if rem != 0 {
        for _ in 0..(4 - rem) {
            cleaned.push('=');
        }
    }

    // Validate decode
    STANDARD
        .decode(&cleaned)
        .map_err(|e| format!("Invalid base64 payload: {e}"))?;

    Ok(cleaned)
}

/// Snapshot an exact byte range from a buffer (Buffer to BlobPart equivalence).
pub fn buffer_to_blob_part(buffer: &[u8], offset: usize, length: usize) -> Result<Vec<u8>, String> {
    if offset > buffer.len() {
        return Err(format!("Offset {} exceeds buffer length {}", offset, buffer.len()));
    }
    let end = offset.saturating_add(length).min(buffer.len());
    Ok(buffer[offset..end].to_vec())
}

/// Process-local async lock helper for small runtime state files.
#[derive(Clone, Default)]
pub struct AsyncLockRuntime {
    locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl AsyncLockRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_lock(&self, key: &str) -> Arc<Mutex<()>> {
        let mut map = self.locks.lock().await;
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
