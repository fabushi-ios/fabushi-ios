//! Bounded remote workspace file transfer reader with chunking and traversal guards.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/remote-workspace-file.ts`
//! - `src/media/remote-workspace-file.test.ts`

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFileChunkPayload {
    #[serde(rename = "dataBase64")]
    pub data_base64: String,
    pub size: u64,
    pub revision: String,
}

#[derive(Debug, Clone)]
pub struct RemoteWorkspaceFileParams {
    pub path: PathBuf,
    pub max_bytes: u64,
    pub workspace_root: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteWorkspaceFileError {
    #[error("Remote workspace file transfer requires a valid media byte limit.")]
    InvalidLimit,
    #[error("Remote file command output cap must be at least 1024 bytes.")]
    CapTooSmall,
    #[error("file exceeds limit of {0} bytes")]
    FileTooLarge(u64),
    #[error("symbolic links are not allowed")]
    SymlinksNotAllowed,
    #[error("file escapes remote workspace")]
    EscapesWorkspace,
    #[error("not a regular file")]
    NotRegularFile,
    #[error("Remote workspace file transfer timed out.")]
    TimedOut,
    #[error("Remote owner closed")]
    OwnerClosed,
    #[error("Remote workspace artifact changed during chunked transfer.")]
    ArtifactChanged,
    #[error("Remote workspace artifact returned invalid or oversized chunk data.")]
    InvalidChunkData,
    #[error("Remote workspace artifact exceeded the command output cap.")]
    ExceededCommandCap,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Computes file revision tag: dev:ino:size:mtimeMs:ctimeMs
pub fn compute_file_revision(meta: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime(),
            meta.ctime()
        )
    }
    #[cfg(not(unix))]
    {
        let len = meta.len();
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        format!("win:0:{len}:{modified}:0")
    }
}

/// Reads a single chunk from a file while asserting workspace bounds and revision invariants.
pub fn read_remote_file_chunk(
    file_path: &Path,
    max_bytes: u64,
    offset: u64,
    chunk_bytes: usize,
    workspace_root: Option<&Path>,
) -> Result<RemoteFileChunkPayload, RemoteWorkspaceFileError> {
    let symlink_meta = fs::symlink_metadata(file_path)?;
    if symlink_meta.file_type().is_symlink() {
        return Err(RemoteWorkspaceFileError::SymlinksNotAllowed);
    }

    let mut file = File::open(file_path)?;
    let before_meta = file.metadata()?;
    if !before_meta.is_file() {
        return Err(RemoteWorkspaceFileError::NotRegularFile);
    }

    if let Some(workspace) = workspace_root {
        let canonical_ws = fs::canonicalize(workspace)?;
        let canonical_file = fs::canonicalize(file_path)?;
        if !canonical_file.starts_with(&canonical_ws) {
            return Err(RemoteWorkspaceFileError::EscapesWorkspace);
        }
    }

    let file_size = before_meta.len();
    if file_size > max_bytes {
        return Err(RemoteWorkspaceFileError::FileTooLarge(max_bytes));
    }
    if offset > file_size {
        return Err(RemoteWorkspaceFileError::ArtifactChanged);
    }

    let expected_to_read = std::cmp::min(chunk_bytes as u64, file_size - offset) as usize;
    let mut buffer = vec![0u8; expected_to_read];

    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buffer)?;

    let after_meta = file.metadata()?;
    let revision = compute_file_revision(&before_meta);
    if compute_file_revision(&after_meta) != revision {
        return Err(RemoteWorkspaceFileError::ArtifactChanged);
    }

    Ok(RemoteFileChunkPayload {
        data_base64: BASE64_STANDARD.encode(&buffer),
        size: file_size,
        revision,
    })
}

/// High-level bounded reader executing chunked transfer with monotonic deadline and cap enforcement.
pub fn read_bounded_remote_workspace_file<F>(
    params: &RemoteWorkspaceFileParams,
    output_bytes_cap: usize,
    assert_active: F,
) -> Result<Vec<u8>, RemoteWorkspaceFileError>
where
    F: Fn() -> Result<(), RemoteWorkspaceFileError>,
{
    if output_bytes_cap < 1024 {
        return Err(RemoteWorkspaceFileError::CapTooSmall);
    }
    let chunk_bytes = output_bytes_cap / 2;
    let start_time = Instant::now();

    assert_active()?;

    let mut chunks = Vec::new();
    let mut offset = 0u64;
    let mut expected_size: Option<u64> = None;
    let mut expected_revision: Option<String> = None;

    loop {
        assert_active()?;

        if let Some(timeout_ms) = params.timeout_ms {
            let elapsed = start_time.elapsed().as_millis() as u64;
            if elapsed >= timeout_ms {
                return Err(RemoteWorkspaceFileError::TimedOut);
            }
        }

        let chunk = read_remote_file_chunk(
            &params.path,
            params.max_bytes,
            offset,
            chunk_bytes,
            params.workspace_root.as_deref(),
        )?;

        let b64_bytes = chunk.data_base64.len();
        if b64_bytes > output_bytes_cap {
            return Err(RemoteWorkspaceFileError::ExceededCommandCap);
        }

        match expected_size {
            None => expected_size = Some(chunk.size),
            Some(sz) if sz != chunk.size => return Err(RemoteWorkspaceFileError::ArtifactChanged),
            _ => {}
        }

        match expected_revision.as_deref() {
            None => expected_revision = Some(chunk.revision.clone()),
            Some(rev) if rev != chunk.revision => return Err(RemoteWorkspaceFileError::ArtifactChanged),
            _ => {}
        }

        let decoded = BASE64_STANDARD
            .decode(&chunk.data_base64)
            .map_err(|_| RemoteWorkspaceFileError::InvalidChunkData)?;

        offset += decoded.len() as u64;
        chunks.extend_from_slice(&decoded);

        if offset >= chunk.size {
            break;
        }
    }

    Ok(chunks)
}
