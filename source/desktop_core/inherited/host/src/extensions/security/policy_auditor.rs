use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct SecurityPolicyAuditor {
    safe_binaries: HashSet<String>,
}

impl Default for SecurityPolicyAuditor {
    fn default() -> Self {
        let mut set = HashSet::new();
        for bin in &[
            "git", "cargo", "rustc", "node", "pnpm", "npm", "bash", "sh", "python3", "python",
            "tar", "gzip", "cat", "ls", "echo", "pwd", "grep", "find",
        ] {
            set.insert(bin.to_string());
        }
        Self { safe_binaries: set }
    }
}

impl SecurityPolicyAuditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_binary_allowed(&self, bin_name: &str) -> bool {
        let clean = Path::new(bin_name)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(bin_name)
            .to_ascii_lowercase();

        self.safe_binaries.contains(&clean)
    }

    pub fn check_symlink_traversal(&self, root_dir: &Path, candidate: &Path) -> Result<(), String> {
        let canonical_root = match root_dir.canonicalize() {
            Ok(p) => p,
            Err(_) => root_dir.to_path_buf(),
        };

        let canonical_candidate = match candidate.canonicalize() {
            Ok(p) => p,
            Err(_) => candidate.to_path_buf(),
        };

        if canonical_candidate.starts_with(&canonical_root) {
            Ok(())
        } else {
            Err(format!(
                "Symlink traversal detected: '{}' resolves outside root '{}'",
                candidate.display(),
                root_dir.display()
            ))
        }
    }

    pub fn check_file_mode_security(&self, _path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = _path.metadata() {
                let mode = metadata.permissions().mode() & 0o777;
                // If group or other have write permission (0o022), flag warning
                if mode & 0o022 != 0 {
                    return Err(format!(
                        "Insecure file permissions: mode {:o} allows group or others to write",
                        mode
                    ));
                }
            }
        }
        Ok(())
    }
}
