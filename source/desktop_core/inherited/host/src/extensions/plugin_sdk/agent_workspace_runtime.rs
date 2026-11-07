use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentWorkspaceAccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWorkspaceAccess {
    pub workspace_dir: PathBuf,
    pub mode: AgentWorkspaceAccessMode,
    #[serde(default)]
    pub allow_subagents: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceBootstrapFilePolicy {
    pub allow_hidden_files: bool,
    pub max_file_size_bytes: u64,
}

impl WorkspaceBootstrapFilePolicy {
    pub fn new(allow_hidden: bool, max_size: u64) -> Self {
        Self {
            allow_hidden_files: allow_hidden,
            max_file_size_bytes: max_size,
        }
    }

    pub fn is_file_allowed(&self, relative_path: &Path, file_size: u64) -> bool {
        if !self.allow_hidden_files {
            for component in relative_path.components() {
                if let std::path::Component::Normal(os_str) = component {
                    if os_str.to_string_lossy().starts_with('.') {
                        return false;
                    }
                }
            }
        }
        file_size <= self.max_file_size_bytes
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceAttachmentPreparer {
    pub max_attachments: usize,
    pub max_total_size: u64,
}

impl WorkspaceAttachmentPreparer {
    pub fn new(max_attachments: usize, max_total_size: u64) -> Self {
        Self {
            max_attachments,
            max_total_size,
        }
    }

    pub fn can_admit_attachment(&self, current_count: usize, current_bytes: u64, next_bytes: u64) -> bool {
        current_count < self.max_attachments && (current_bytes + next_bytes) <= self.max_total_size
    }
}
