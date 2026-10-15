use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::box_store_manifest::BoxStoreManifestEntry;

pub const BOX_STORE_BLOBS_PREFIX: &str = "blobs";

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn resolve_restore_destination(target_root: &Path, rel_path: &str) -> Option<PathBuf> {
    if rel_path.is_empty()
        || rel_path.contains('\0')
        || Path::new(rel_path).is_absolute()
        || rel_path.split('/').any(|segment| {
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.as_bytes().iter().any(|byte| *byte < 32 || *byte == 127)
        })
    {
        return None;
    }
    let root = lexical_normalize(target_root);
    let destination = lexical_normalize(&root.join(rel_path));
    (destination != root && destination.starts_with(&root)).then_some(destination)
}

pub fn symlink_target_stays_within_root(
    target_root: &Path,
    symlink_path: &Path,
    target: &str,
) -> bool {
    if target.is_empty() || target.contains('\0') {
        return false;
    }
    let root = lexical_normalize(target_root);
    let target_path = Path::new(target);
    let candidate = if target_path.is_absolute() {
        lexical_normalize(target_path)
    } else {
        let Some(parent) = symlink_path.parent() else {
            return false;
        };
        lexical_normalize(&parent.join(target_path))
    };
    candidate == root || candidate.starts_with(&root)
}

fn unique_temp_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("restore");
    destination.with_file_name(format!(".{name}.box-store-part-{}", Uuid::new_v4()))
}

fn remove_existing_restore_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path).map_err(|error| error.to_string())
        }
        Ok(_) => fs::remove_file(path).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn verify_blob(bytes: &[u8], sha: &str, size: u64) -> Result<(), String> {
    if bytes.len() as u64 != size {
        return Err(format!(
            "box store blob size mismatch: expected {size}, got {}",
            bytes.len()
        ));
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != sha.to_ascii_lowercase() {
        return Err("box store blob sha mismatch".into());
    }
    Ok(())
}

pub fn restore_file_atomic(
    target_root: &Path,
    rel_path: &str,
    entry: &BoxStoreManifestEntry,
    bytes: &[u8],
) -> Result<u64, String> {
    let (sha, size, mode) = match entry {
        BoxStoreManifestEntry::LegacyFile { sha, size } => (sha.as_str(), *size, 0o600_u32),
        BoxStoreManifestEntry::File { sha, size, mode } => (sha.as_str(), *size, *mode),
        BoxStoreManifestEntry::Symlink { .. } => {
            return Err("symlink entry cannot be restored as a file".into());
        }
    };
    verify_blob(bytes, sha, size)?;
    let destination = resolve_restore_destination(target_root, rel_path)
        .ok_or_else(|| format!("unsafe box store restore path: {rel_path}"))?;
    let parent = destination
        .parent()
        .ok_or_else(|| "box store restore destination has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;

    let temp = unique_temp_path(&destination);
    let install = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.flush().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(mode & 0o777))
                .map_err(|error| error.to_string())?;
        }
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
            }
            Ok(_) | Err(_) => {}
        }
        fs::rename(&temp, &destination).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if install.is_err() {
        let _ = fs::remove_file(&temp);
    }
    install?;

    let restored = fs::read(&destination).map_err(|error| error.to_string())?;
    verify_blob(&restored, sha, size)?;
    Ok(size)
}

pub fn restore_symlink_atomic(
    target_root: &Path,
    rel_path: &str,
    target: &str,
) -> Result<(), String> {
    let destination = resolve_restore_destination(target_root, rel_path)
        .ok_or_else(|| format!("unsafe box store symlink path: {rel_path}"))?;
    if !symlink_target_stays_within_root(target_root, &destination, target) {
        return Err(format!("unsafe box store symlink target: {target}"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "box store symlink destination has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = unique_temp_path(&destination);
    let install = (|| -> Result<(), String> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, &temp).map_err(|error| error.to_string())?;
        }
        #[cfg(not(unix))]
        {
            let _ = target;
            return Err("box store symlink restore is unsupported on this platform".into());
        }
        remove_existing_restore_path(&destination)?;
        fs::rename(&temp, &destination).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if install.is_err() {
        let _ = fs::remove_file(&temp);
    }
    install?;
    let restored_target = fs::read_link(&destination).map_err(|error| error.to_string())?;
    if restored_target != Path::new(target) {
        return Err("restored symlink target does not match".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::box_store_manifest::BoxStoreManifestEntry;

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("fabushi-box-hydrate-{}", Uuid::new_v4()))
    }

    #[test]
    fn restore_destination_and_symlink_target_fail_closed() {
        let root = PathBuf::from("/sandbox/account/agent");
        assert_eq!(
            resolve_restore_destination(&root, "memory/state.json"),
            Some(root.join("memory/state.json"))
        );
        for invalid in ["", "../secret", "memory/../secret", "/absolute", "memory//state"] {
            assert!(resolve_restore_destination(&root, invalid).is_none());
        }
        let link = root.join("memory/latest");
        assert!(symlink_target_stays_within_root(&root, &link, "state.json"));
        assert!(!symlink_target_stays_within_root(&root, &link, "../../../secret"));
        assert!(!symlink_target_stays_within_root(&root, &link, "/outside/secret"));
    }

    #[test]
    fn file_restore_verifies_sha_size_and_commits_atomically() {
        let root = test_root();
        let bytes = b"durable-state";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let entry = BoxStoreManifestEntry::File {
            sha,
            size: bytes.len() as u64,
            mode: 0o600,
        };
        let restored = restore_file_atomic(&root, "memory/state.bin", &entry, bytes)
            .expect("restore");
        assert_eq!(restored, bytes.len() as u64);
        assert_eq!(fs::read(root.join("memory/state.bin")).unwrap(), bytes);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_restore_rejects_unverified_blob_without_touching_destination() {
        let root = test_root();
        fs::create_dir_all(root.join("memory")).unwrap();
        fs::write(root.join("memory/state.bin"), b"old").unwrap();
        let entry = BoxStoreManifestEntry::File {
            sha: "deadbeef".into(),
            size: 3,
            mode: 0o600,
        };
        assert!(restore_file_atomic(&root, "memory/state.bin", &entry, b"new").is_err());
        assert_eq!(fs::read(root.join("memory/state.bin")).unwrap(), b"old");
        let _ = fs::remove_dir_all(root);
    }
}
