use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use url::Url;
use uuid::Uuid;

use crate::sha256::sha256_hex;

pub const ATTACHMENTS_DIRNAME: &str = "attachments";
pub const ASSETS_DIRNAME: &str = "assets";
pub const OUTBOUND_ATTACHMENTS_DIRNAME: &str = "outbound";
pub const SANDBOX_SUBAGENT_ATTACHMENTS_MOUNT: &str = "/openclaw/attachments";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMediaKind {
    Image,
    Attachment,
}

/// Owns media materialized for one outgoing message until its transcript
/// entry has been accepted by the existing session owner.
#[derive(Debug, Default)]
pub struct OutgoingMediaCustody {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl OutgoingMediaCustody {
    pub fn track(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    pub fn absorb(&mut self, mut other: Self) {
        self.paths.append(&mut other.paths);
    }

    pub fn commit(&mut self) {
        self.committed = true;
        self.paths.clear();
    }

    pub fn rollback(&mut self) -> Vec<io::Error> {
        if self.committed {
            return Vec::new();
        }
        self.paths
            .drain(..)
            .filter_map(|path| match fs::remove_file(&path) {
                Ok(()) => None,
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => Some(error),
            })
            .collect()
    }
}

impl Drop for OutgoingMediaCustody {
    fn drop(&mut self) {
        // Explicit rollback reports errors. Drop is only a last-resort safety
        // net for an unwound tool path and therefore cannot surface I/O.
        let _ = self.rollback();
    }
}

pub fn get_agent_attachments_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(ATTACHMENTS_DIRNAME)
}

pub fn get_agent_assets_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(ASSETS_DIRNAME)
}

pub fn get_agent_outbound_attachments_dir(agent_dir: impl AsRef<Path>) -> PathBuf {
    agent_dir.as_ref().join(ATTACHMENTS_DIRNAME).join(OUTBOUND_ATTACHMENTS_DIRNAME)
}

pub fn get_agent_media_store_roots(agent_dir: impl AsRef<Path>) -> [PathBuf; 2] {
    [get_agent_attachments_dir(&agent_dir), get_agent_assets_dir(agent_dir)]
}

pub fn normalize_agent_id(agent_id: &str) -> String {
    let mut normalized = agent_id
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '_' })
        .collect::<String>();
    while normalized.starts_with('_') {
        normalized.remove(0);
    }
    if normalized.is_empty() {
        normalized.push_str("default");
    }
    normalized
}

pub fn resolve_subagent_session_attachment_root_dir(
    state_dir: &Path,
    agent_id: &str,
    child_session_key: &str,
) -> PathBuf {
    let session_hash = sha256_hex(child_session_key.as_bytes());
    let session_ref = if session_hash.len() > 32 {
        &session_hash[..32]
    } else {
        &session_hash
    };
    state_dir
        .join(ATTACHMENTS_DIRNAME)
        .join("subagents")
        .join(normalize_agent_id(agent_id))
        .join(session_ref)
}

pub fn subagent_attachment_root_for_run(
    state_dir: &Path,
    agent_id: Option<&str>,
    child_session_key: Option<&str>,
) -> Option<PathBuf> {
    match (agent_id, child_session_key) {
        (Some(agent_id), Some(child_session_key)) => Some(
            resolve_subagent_session_attachment_root_dir(state_dir, agent_id, child_session_key),
        ),
        _ => None,
    }
}

pub fn compute_base64_decoded_size(base64: &str) -> usize {
    let trimmed = base64.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let padding = if trimmed.ends_with("==") {
        2
    } else if trimmed.ends_with('=') {
        1
    } else {
        0
    };
    (trimmed.len() * 3 / 4).saturating_sub(padding)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachmentProcessingError {
    #[error("attachment {label}: exceeds size limit ({size} > {max_bytes} bytes)")]
    ExceedsSizeLimit {
        label: String,
        size: usize,
        max_bytes: usize,
    },
}

pub fn validate_and_prepare_attachment_payload(
    label: &str,
    base64: &str,
    max_bytes: usize,
) -> Result<usize, AttachmentProcessingError> {
    let size = compute_base64_decoded_size(base64);
    if size > max_bytes {
        return Err(AttachmentProcessingError::ExceedsSizeLimit {
            label: label.to_string(),
            size,
            max_bytes,
        });
    }
    Ok(size)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundAttachmentResult {
    pub path: PathBuf,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundAttachmentError {
    #[error("outbound attachment exceeds max bytes limit ({size} > {max_bytes})")]
    ExceedsMaxBytes { size: usize, max_bytes: usize },
    #[error("failed to stage outbound attachment: {0}")]
    Io(String),
}

pub fn resolve_outbound_attachment_from_buffer(
    agent_dir: impl AsRef<Path>,
    filename: Option<&str>,
    buffer: &[u8],
    max_bytes: usize,
    content_type: Option<&str>,
) -> Result<OutboundAttachmentResult, OutboundAttachmentError> {
    if buffer.len() > max_bytes {
        return Err(OutboundAttachmentError::ExceedsMaxBytes {
            size: buffer.len(),
            max_bytes,
        });
    }
    let outbound_dir = get_agent_outbound_attachments_dir(agent_dir);
    fs::create_dir_all(&outbound_dir)
        .map_err(|err| OutboundAttachmentError::Io(err.to_string()))?;
    let safe_leaf = safe_media_leaf(filename.unwrap_or("outbound.bin"));
    let target = outbound_dir.join(format!("{}-{}", Uuid::new_v4().simple(), safe_leaf));
    fs::write(&target, buffer)
        .map_err(|err| OutboundAttachmentError::Io(err.to_string()))?;
    Ok(OutboundAttachmentResult {
        path: target,
        content_type: content_type.map(str::to_string),
    })
}

pub fn safe_media_leaf(source_name: &str) -> String {
    let normalized = source_name.replace('\\', "/");
    let leaf = Path::new(&normalized)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    let mut safe = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while safe.starts_with('.') {
        safe.remove(0);
    }
    if safe.is_empty() {
        safe.push_str("attachment");
    }
    safe.truncate(160);
    safe
}

pub fn persist_agent_media_bytes(
    agent_dir: impl AsRef<Path>,
    source_name: &str,
    bytes: &[u8],
    kind: AgentMediaKind,
) -> io::Result<PathBuf> {
    let root = match kind {
        AgentMediaKind::Image => get_agent_assets_dir(agent_dir),
        AgentMediaKind::Attachment => get_agent_attachments_dir(agent_dir),
    };
    fs::create_dir_all(&root)?;
    let target = root.join(format!("{}-{}", Uuid::new_v4().simple(), safe_media_leaf(source_name)));
    let temporary = root.join(format!(".{}.tmp", Uuid::new_v4().simple()));
    fs::write(&temporary, bytes)?;
    if let Err(error) = fs::rename(&temporary, &target) {
        crate::extensions::attachments::media_file_context::unlink_if_exists(Some(&temporary));
        return Err(error);
    }
    Ok(target)
}

pub fn file_url_for_path(path: impl AsRef<Path>) -> Option<String> {
    Url::from_file_path(path.as_ref()).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::OutgoingMediaCustody;

    fn temp_media_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-outgoing-media-custody-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn rollback_removes_every_uncommitted_path() {
        let first = temp_media_path("first");
        let second = temp_media_path("second");
        std::fs::write(&first, b"first").expect("write first media");
        std::fs::write(&second, b"second").expect("write second media");

        let mut custody = OutgoingMediaCustody::default();
        custody.track(first.clone());
        custody.track(second.clone());
        assert!(custody.rollback().is_empty());
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn commit_prevents_cleanup_of_persisted_media() {
        let path = temp_media_path("committed");
        std::fs::write(&path, b"committed").expect("write committed media");

        let mut custody = OutgoingMediaCustody::default();
        custody.track(path.clone());
        custody.commit();
        assert!(custody.rollback().is_empty());
        assert!(path.exists());
        std::fs::remove_file(path).expect("remove committed test media");
    }
}
