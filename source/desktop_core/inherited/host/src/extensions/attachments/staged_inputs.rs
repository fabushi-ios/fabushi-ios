//! Staged input handling, ownership validation, directory sandboxing, and media path resolution.
//!
//! Ported with 100% behavioral equivalence from OpenClaw:
//! - `src/media/staged-inputs.ts`
//! - `src/media/staged-inputs.test-support.ts`

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::extensions::attachments::media_facts::MediaFact;
use crate::extensions::attachments::media_file_context::sanitize_untrusted_file_name;

/// Existing per-file allowance for staging task inputs (50MB).
pub const STAGED_INPUT_MAX_BYTES: usize = 50 * 1024 * 1024;

pub const STAGED_INPUT_DIRECTORY_PREFIX: &str = "media/inbound/openclaw-staged-";
pub const STAGED_INPUT_GIT_PATHSPEC: &str = ":(glob)media/inbound/openclaw-staged-*/**";
pub const STAGED_INPUT_GITIGNORE: &str =
    "# Raw task inputs remain private; copy outputs into the project to publish.\n*\n";

pub const STAGED_INPUT_GITIGNORE_SHA256: &str =
    "c2ae48852cf5745c7dcbcdfa46b0141497cb37fda5b011164f75dd3c08358e6f";

/// Returns true if identity is 64 hex characters or a lowercase UUID v4 string.
fn is_valid_staged_input_identity(identity: &str) -> bool {
    let hex_64 = identity.len() == 64 && identity.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
    if hex_64 {
        return true;
    }
    // UUID v4 format: 8-4-4-4-12 lowercase hex, with 4 at version and 8, 9, a, or b at variant
    if identity.len() == 36 {
        let parts: Vec<&str> = identity.split('-').collect();
        if parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
        {
            let is_hex = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
            if is_hex(parts[0])
                && is_hex(parts[1])
                && is_hex(parts[2])
                && is_hex(parts[3])
                && is_hex(parts[4])
                && parts[2].starts_with('4')
                && (parts[3].starts_with('8')
                    || parts[3].starts_with('9')
                    || parts[3].starts_with('a')
                    || parts[3].starts_with('b'))
            {
                return true;
            }
        }
    }
    false
}

/// A producer-shaped name is only a candidate; the marker establishes ownership.
pub fn staged_input_path_directory(relative_path: &str) -> Option<String> {
    if !relative_path.starts_with(STAGED_INPUT_DIRECTORY_PREFIX) {
        return None;
    }
    let rest = &relative_path[STAGED_INPUT_DIRECTORY_PREFIX.len()..];
    let identity = rest.split('/').next()?;
    if is_valid_staged_input_identity(identity) {
        Some(format!("{STAGED_INPUT_DIRECTORY_PREFIX}{identity}"))
    } else {
        None
    }
}

pub fn is_staged_input_path(relative_path: &str, directories: &HashSet<String>) -> bool {
    staged_input_path_directory(relative_path)
        .map(|dir| directories.contains(&dir))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedManifestEntry {
    pub path: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Complete manifests bind the regular marker's exact bytes through their file digest.
pub fn staged_input_directories_from_entries(entries: &[StagedManifestEntry]) -> HashSet<String> {
    let mut directories = HashSet::new();
    for entry in entries {
        if let Some(directory) = staged_input_path_directory(&entry.path) {
            let expected_ignore = format!("{directory}/.gitignore");
            if entry.path == expected_ignore
                && entry.r#type == "file"
                && entry.size == Some(STAGED_INPUT_GITIGNORE.len())
                && entry.sha256.as_deref() == Some(STAGED_INPUT_GITIGNORE_SHA256)
            {
                directories.insert(directory);
            }
        }
    }
    directories
}

pub fn staged_input_directory(identity: &str) -> String {
    format!("{STAGED_INPUT_DIRECTORY_PREFIX}{identity}")
}

pub fn staged_input_file_name(name: &str) -> String {
    sanitize_untrusted_file_name(&format!("input-{name}"), "input-attachment")
}

/// Maps producer-stamped upload handles to exact private paths for the current turn.
pub fn resolve_staged_input_media_paths(media: Option<&[MediaFact]>) -> HashMap<String, String> {
    let mut paths = HashMap::new();
    let mut ambiguous = HashSet::new();

    let file_pattern = Regex::new(r"^file_[^/\\]+$").unwrap();

    for fact in media.unwrap_or_default() {
        if fact.staged != Some(true) {
            continue;
        }
        let Some(ref fact_path) = fact.path else {
            continue;
        };
        let Some(directory) = staged_input_path_directory(fact_path) else {
            continue;
        };
        let prefix = format!("{directory}/input-");
        if !fact_path.starts_with(&prefix) {
            continue;
        }
        let file_name = &fact_path[prefix.len()..];
        if !file_pattern.is_match(file_name) {
            continue;
        }

        let extension = Path::new(file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"));

        let aliases = if let Some(ref ext) = extension {
            let stem = &file_name[..file_name.len() - ext.len()];
            vec![file_name.to_string(), stem.to_string()]
        } else {
            vec![file_name.to_string()]
        };

        for alias in aliases {
            if ambiguous.contains(&alias) {
                continue;
            }
            if let Some(existing) = paths.get(&alias) {
                if existing != fact_path {
                    paths.remove(&alias);
                    ambiguous.insert(alias);
                }
            } else {
                paths.insert(alias, fact_path.clone());
            }
        }
    }

    paths
}

/// Ensures the staged input directory exists and is owned by OpenClaw with its `.gitignore` marker.
pub fn ensure_staged_input_directory(root_dir: &Path, directory: &str) -> Result<(), String> {
    let dir_path = root_dir.join(directory);
    let ignore_path = dir_path.join(".gitignore");

    if dir_path.exists() {
        if !ignore_path.exists() {
            return Err("Input staging directory is not owned by OpenClaw".to_string());
        }
        let content = fs::read_to_string(&ignore_path)
            .map_err(|e| format!("Failed to read .gitignore: {e}"))?;
        if content != STAGED_INPUT_GITIGNORE {
            return Err("Input staging directory is not owned by OpenClaw".to_string());
        }
        return Ok(());
    }

    fs::create_dir_all(&dir_path)
        .map_err(|e| format!("Failed to create directory {directory}: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        use std::io::Write;
        let mut file = opts
            .open(&ignore_path)
            .map_err(|e| format!("Failed to create {}: {e}", ignore_path.display()))?;
        file.write_all(STAGED_INPUT_GITIGNORE.as_bytes())
            .map_err(|e| format!("Failed to write .gitignore: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&ignore_path, STAGED_INPUT_GITIGNORE)
            .map_err(|e| format!("Failed to write .gitignore: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_constant() {
        let mut hasher = Sha256::new();
        hasher.update(STAGED_INPUT_GITIGNORE.as_bytes());
        let computed = format!("{:x}", hasher.finalize());
        assert_eq!(computed, STAGED_INPUT_GITIGNORE_SHA256);
    }

    #[test]
    fn test_staged_input_path_directory() {
        let valid_hex = "a".repeat(64);
        let path = format!("media/inbound/openclaw-staged-{valid_hex}/file.txt");
        assert_eq!(
            staged_input_path_directory(&path),
            Some(format!("media/inbound/openclaw-staged-{valid_hex}"))
        );

        let valid_uuid = "11111111-1111-4111-8111-111111111111";
        let path_uuid = format!("media/inbound/openclaw-staged-{valid_uuid}/input-secret.txt");
        assert_eq!(
            staged_input_path_directory(&path_uuid),
            Some(format!("media/inbound/openclaw-staged-{valid_uuid}"))
        );

        // Invalid cases
        assert_eq!(staged_input_path_directory("media/inbound/other/file.txt"), None);
        assert_eq!(staged_input_path_directory("media/inbound/openclaw-staged-cafe/file.txt"), None);
        // Uppercase hex rejected
        assert_eq!(
            staged_input_path_directory(&format!("media/inbound/openclaw-staged-{}/file.txt", "A".repeat(64))),
            None
        );
    }

    #[test]
    fn test_resolve_staged_input_media_paths() {
        let uuid = "11111111-1111-4111-8111-111111111111";
        let dir = staged_input_directory(uuid);
        let p1 = format!("{dir}/input-file_report.pdf");
        let p2 = format!("{dir}/input-file_other.txt");

        let facts = vec![
            MediaFact {
                path: Some(p1.clone()),
                staged: Some(true),
                ..Default::default()
            },
            MediaFact {
                path: Some(p2.clone()),
                staged: Some(true),
                ..Default::default()
            },
            MediaFact {
                path: Some("media/inbound/unrelated.jpg".to_string()),
                staged: Some(true),
                ..Default::default()
            },
        ];

        let resolved = resolve_staged_input_media_paths(Some(&facts));
        assert_eq!(resolved.get("file_report.pdf"), Some(&p1));
        assert_eq!(resolved.get("file_report"), Some(&p1));
        assert_eq!(resolved.get("file_other.txt"), Some(&p2));
        assert_eq!(resolved.get("file_other"), Some(&p2));
    }
}
