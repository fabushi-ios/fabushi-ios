use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};

pub const CANONICAL_ROOT_MEMORY_FILENAME: &str = "MEMORY.md";
pub const LEGACY_ROOT_MEMORY_FILENAME: &str = "memory.md";
pub const ROOT_MEMORY_REPAIR_RELATIVE_DIR: &str = ".openclaw-repair/root-memory";

pub fn resolve_canonical_root_memory_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(CANONICAL_ROOT_MEMORY_FILENAME)
}

pub fn resolve_legacy_root_memory_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(LEGACY_ROOT_MEMORY_FILENAME)
}

pub fn resolve_root_memory_repair_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(".openclaw-repair").join("root-memory")
}

pub fn should_skip_root_memory_auxiliary_path(workspace_dir: &Path, abs_path: &Path) -> bool {
    let relative = match abs_path.strip_prefix(workspace_dir) {
        Ok(rel) => rel,
        Err(_) => return false,
    };
    let rel_str = relative.to_string_lossy().replace('\\', "/");
    let normalized = rel_str.trim_start_matches("./");

    normalized == LEGACY_ROOT_MEMORY_FILENAME
        || normalized == ROOT_MEMORY_REPAIR_RELATIVE_DIR
        || normalized.starts_with(&format!("{}/", ROOT_MEMORY_REPAIR_RELATIVE_DIR))
}

pub fn normalize_memory_artifact_relative_path(relative_path: &str) -> Option<String> {
    let normalized = relative_path.replace('\\', "/");
    if normalized.is_empty() || normalized.starts_with('/') {
        return None;
    }
    if normalized.split('/').any(|seg| seg == "..") {
        return None;
    }
    if normalized == "MEMORY.md" || normalized == "memory.md" || normalized == "USER.md" {
        return Some(normalized);
    }
    if normalized.starts_with("users/") && normalized.ends_with("/USER.md") {
        let parts: Vec<&str> = normalized.split('/').collect();
        if parts.len() == 3 && parts[0] == "users" && parts[2] == "USER.md" {
            let username = parts[1];
            if !username.is_empty()
                && username.len() <= 128
                && username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Some(normalized);
            }
        }
    }
    if normalized.starts_with("memory/") && normalized.ends_with(".md") {
        if normalized.starts_with("memory/dreaming/") || normalized.starts_with("memory/.dreams/") {
            return None;
        }
        return Some(normalized);
    }
    None
}

pub fn compute_file_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryArtifactOriginClass {
    Agent,
    Untrusted,
}

impl MemoryArtifactOriginClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Untrusted => "untrusted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryArtifactProvenance {
    pub file_hash: String,
    pub origin_class: MemoryArtifactOriginClass,
    pub observed_at: u64,
    pub session_id: Option<String>,
    pub session_key: Option<String>,
}

pub fn resolve_memory_artifact_write_origin(
    previous_provenance: Option<&MemoryArtifactProvenance>,
    requested_origin: MemoryArtifactOriginClass,
    before_hash: &str,
) -> MemoryArtifactOriginClass {
    if requested_origin != MemoryArtifactOriginClass::Agent {
        return MemoryArtifactOriginClass::Untrusted;
    }
    match previous_provenance {
        None => MemoryArtifactOriginClass::Agent,
        Some(prev) => {
            if prev.origin_class == MemoryArtifactOriginClass::Agent && prev.file_hash == before_hash {
                MemoryArtifactOriginClass::Agent
            } else {
                MemoryArtifactOriginClass::Untrusted
            }
        }
    }
}
