use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::box_store_manifest::BoxStoreManifestEntry;

pub const BOX_STORE_BLOBS_PREFIX: &str = "blobs";

const SQLITE_AGENT_DB_BASENAMES: [&str; 2] = ["store.db", "conversation-blobs.db"];
const SQLITE_AGENT_DB_SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

fn is_sqlite_agent_db_bundle_path(rel_path: &str) -> bool {
    let Some(basename) = Path::new(rel_path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    SQLITE_AGENT_DB_BASENAMES.iter().any(|db| {
        basename == *db
            || SQLITE_AGENT_DB_SIDECAR_SUFFIXES
                .iter()
                .any(|suffix| basename == format!("{db}{suffix}"))
    })
}

fn ensure_plain_file_snapshot_path(rel_path: &str) -> Result<(), String> {
    if is_sqlite_agent_db_bundle_path(rel_path) {
        return Err(format!(
            "Agent Store plain-file sync refuses SQLite bundle path {rel_path};              a SQLite-aware capture/restore owner is required"
        ));
    }
    Ok(())
}

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
    if ensure_plain_file_snapshot_path(rel_path).is_err()
        || rel_path.is_empty()
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
        file.sync_all().map_err(|error| error.to_string())?;
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
        let parent_dir = OpenOptions::new()
            .read(true)
            .open(parent)
            .map_err(|error| error.to_string())?;
        parent_dir.sync_all().map_err(|error| error.to_string())?;
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


fn local_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0o600
    }
}

fn rel_path_string(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "snapshot path escaped Agent root".to_string())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| "snapshot path is not valid UTF-8".to_string())?;
                if value.is_empty() {
                    return Err("snapshot path contains an empty component".into());
                }
                parts.push(value);
            }
            _ => return Err("snapshot path contains an unsafe component".into()),
        }
    }
    if parts.is_empty() {
        return Err("snapshot path must not resolve to Agent root".into());
    }
    Ok(parts.join("/"))
}

fn walk_snapshot_root(
    root: &Path,
    current: &Path,
    out: &mut BTreeMap<String, BoxStoreManifestEntry>,
) -> Result<(), String> {
    let read_dir = match fs::read_dir(current) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && current == root => {
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut children = read_dir
        .map(|entry| entry.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let rel_path = rel_path_string(root, &path)?;
        ensure_plain_file_snapshot_path(&rel_path)?;
        if resolve_restore_destination(root, &rel_path).is_none() {
            return Err(format!("unsafe local snapshot path: {rel_path}"));
        }
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(&path).map_err(|error| error.to_string())?;
            let target = target
                .to_str()
                .ok_or_else(|| format!("symlink target is not UTF-8: {rel_path}"))?;
            if !symlink_target_stays_within_root(root, &path, target) {
                return Err(format!("unsafe local snapshot symlink target: {rel_path}"));
            }
            out.insert(
                rel_path,
                BoxStoreManifestEntry::Symlink {
                    target: target.to_string(),
                },
            );
        } else if file_type.is_dir() {
            walk_snapshot_root(root, &path, out)?;
        } else if file_type.is_file() {
            let bytes = fs::read(&path).map_err(|error| error.to_string())?;
            let sha = format!("{:x}", Sha256::digest(&bytes));
            out.insert(
                rel_path,
                BoxStoreManifestEntry::File {
                    sha,
                    size: bytes.len() as u64,
                    mode: local_mode(&metadata),
                },
            );
        } else {
            return Err(format!("unsupported local snapshot node: {rel_path}"));
        }
    }
    Ok(())
}

pub fn snapshot_local_agent_root(
    target_root: &Path,
) -> Result<BTreeMap<String, BoxStoreManifestEntry>, String> {
    let mut entries = BTreeMap::new();
    walk_snapshot_root(target_root, target_root, &mut entries)?;
    Ok(entries)
}

pub fn read_snapshot_file(
    target_root: &Path,
    rel_path: &str,
    entry: &BoxStoreManifestEntry,
) -> Result<Vec<u8>, String> {
    ensure_plain_file_snapshot_path(rel_path)?;
    let destination = resolve_restore_destination(target_root, rel_path)
        .ok_or_else(|| format!("unsafe local snapshot read path: {rel_path}"))?;
    let metadata = fs::symlink_metadata(&destination).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("local snapshot file changed type: {rel_path}"));
    }
    let bytes = fs::read(&destination).map_err(|error| error.to_string())?;
    let (sha, size) = match entry {
        BoxStoreManifestEntry::LegacyFile { sha, size }
        | BoxStoreManifestEntry::File { sha, size, .. } => (sha.as_str(), *size),
        BoxStoreManifestEntry::Symlink { .. } => {
            return Err("symlink entry cannot be read as snapshot blob".into());
        }
    };
    verify_blob(&bytes, sha, size)?;
    Ok(bytes)
}

pub fn blob_rel_path(sha: &str) -> Result<String, String> {
    let sha = sha.trim().to_ascii_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("box store blob sha must be 64 lowercase-compatible hex characters".into());
    }
    Ok(format!("{BOX_STORE_BLOBS_PREFIX}/{sha}"))
}

fn prune_snapshot_walk(
    root: &Path,
    current: &Path,
    desired: &BTreeMap<String, BoxStoreManifestEntry>,
) -> Result<u64, String> {
    let mut removed = 0_u64;
    let read_dir = match fs::read_dir(current) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    let mut children = read_dir
        .map(|entry| entry.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());

    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        let rel_path = rel_path_string(root, &path)?;
        if resolve_restore_destination(root, &rel_path).is_none() {
            return Err(format!("unsafe local prune path: {rel_path}"));
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            removed = removed.saturating_add(prune_snapshot_walk(root, &path, desired)?);
            let empty = fs::read_dir(&path)
                .map_err(|error| error.to_string())?
                .next()
                .is_none();
            if empty {
                fs::remove_dir(&path).map_err(|error| error.to_string())?;
                removed = removed.saturating_add(1);
            }
        } else if !desired.contains_key(&rel_path) {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

pub fn prune_local_agent_root(
    target_root: &Path,
    desired: &BTreeMap<String, BoxStoreManifestEntry>,
) -> Result<u64, String> {
    prune_snapshot_walk(target_root, target_root, desired)
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
        let leftovers = fs::read_dir(root.join("memory"))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.contains(".box-store-part-"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "atomic restore left temp files: {leftovers:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn local_snapshot_hashes_files_and_rejects_escaping_symlinks() {
        let root = test_root();
        fs::create_dir_all(root.join("memory")).unwrap();
        fs::write(root.join("memory/state.bin"), b"durable-state").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("state.bin", root.join("memory/latest")).unwrap();

        let entries = snapshot_local_agent_root(&root).expect("snapshot");
        let state = entries.get("memory/state.bin").expect("file entry");
        let bytes = read_snapshot_file(&root, "memory/state.bin", state).expect("snapshot read");
        assert_eq!(bytes, b"durable-state");
        #[cfg(unix)]
        assert!(matches!(
            entries.get("memory/latest"),
            Some(BoxStoreManifestEntry::Symlink { target }) if target == "state.bin"
        ));

        #[cfg(unix)]
        {
            fs::remove_file(root.join("memory/latest")).unwrap();
            std::os::unix::fs::symlink("../../../outside", root.join("memory/latest")).unwrap();
            assert!(snapshot_local_agent_root(&root).is_err());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn agent_root_preserves_workspace_cache_named_paths_as_durable_payload() {
        let root = test_root();
        for rel in [
            ".cache/session/state.bin",
            "target/runtime/model.bin",
            "node_modules/tool/state.json",
            "dist/export/result.txt",
        ] {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, rel.as_bytes()).unwrap();
        }

        let entries = snapshot_local_agent_root(&root)
            .expect("Agent root cache-like names are durable payload, not Desktop workspace caches");
        for rel in [
            ".cache/session/state.bin",
            "target/runtime/model.bin",
            "node_modules/tool/state.json",
            "dist/export/result.txt",
        ] {
            assert!(entries.contains_key(rel), "missing durable Agent payload {rel}");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plain_file_snapshot_rejects_sqlite_db_bundles_and_sidecars() {
        let root = test_root();
        fs::create_dir_all(root.join("nested")).unwrap();

        for basename in [
            "store.db",
            "store.db-wal",
            "store.db-shm",
            "conversation-blobs.db",
            "conversation-blobs.db-wal",
            "conversation-blobs.db-shm",
        ] {
            let path = root.join("nested").join(basename);
            fs::write(&path, b"sqlite-ish").unwrap();
            let error = snapshot_local_agent_root(&root)
                .expect_err("plain-file snapshot must fail closed on SQLite bundle members");
            assert!(
                error.contains("SQLite bundle path"),
                "unexpected rejection for {basename}: {error}"
            );
            fs::remove_file(path).unwrap();
        }

        fs::write(root.join("nested/store.db.backup"), b"ordinary-file").unwrap();
        let entries = snapshot_local_agent_root(&root)
            .expect("non-canonical sqlite-like filename remains ordinary data");
        assert!(entries.contains_key("nested/store.db.backup"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plain_file_restore_rejects_sqlite_db_bundle_without_touching_destination() {
        let root = test_root();
        fs::create_dir_all(root.join("nested")).unwrap();
        let destination = root.join("nested/store.db");
        fs::write(&destination, b"existing-db").unwrap();
        let bytes = b"replacement-db";
        let entry = BoxStoreManifestEntry::File {
            sha: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            mode: 0o600,
        };

        let error = restore_file_atomic(&root, "nested/store.db", &entry, bytes)
            .expect_err("plain-file restore must refuse SQLite store.db");
        assert!(error.contains("unsafe box store restore path"));
        assert_eq!(fs::read(&destination).unwrap(), b"existing-db");

        assert!(resolve_restore_destination(&root, "nested/store.db-wal").is_none());
        assert!(resolve_restore_destination(&root, "nested/conversation-blobs.db-shm").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blob_rel_path_requires_content_address_sha() {
        assert_eq!(
            blob_rel_path("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                .expect("blob path"),
            "blobs/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(blob_rel_path("deadbeef").is_err());
        assert!(blob_rel_path(&"z".repeat(64)).is_err());
    }

    #[test]
    fn prune_removes_nodes_missing_from_remote_manifest_after_verified_restore() {
        let root = test_root();
        fs::create_dir_all(root.join("memory/stale-dir")).unwrap();
        fs::write(root.join("memory/keep.bin"), b"keep").unwrap();
        fs::write(root.join("memory/stale.bin"), b"stale").unwrap();
        fs::write(root.join("memory/stale-dir/old.bin"), b"old").unwrap();

        let keep = b"keep";
        let mut desired = BTreeMap::new();
        desired.insert(
            "memory/keep.bin".into(),
            BoxStoreManifestEntry::File {
                sha: format!("{:x}", Sha256::digest(keep)),
                size: keep.len() as u64,
                mode: 0o600,
            },
        );

        let removed = prune_local_agent_root(&root, &desired).expect("prune");
        assert!(removed >= 3);
        assert_eq!(fs::read(root.join("memory/keep.bin")).unwrap(), keep);
        assert!(!root.join("memory/stale.bin").exists());
        assert!(!root.join("memory/stale-dir").exists());
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
