//! iOS-safe directory ownership checks. No Linux inventory or process topology.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn ensure_private_directory(path: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Dandelion private path is not an owned directory".into());
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).map_err(|error| error.to_string())?;
    let verified = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if verified.file_type().is_symlink()
        || !verified.is_dir()
        || verified.permissions().mode() & 0o077 != 0
    {
        return Err("Dandelion private directory permissions are not owner-only".into());
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

pub fn validate_installed_directory(
    installed_path: &Path,
    expected_plugin_root: &Path,
) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(installed_path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Dandelion active package path is not a regular directory".into());
    }
    let installed = fs::canonicalize(installed_path).map_err(|error| error.to_string())?;
    let plugin_root = fs::canonicalize(expected_plugin_root).map_err(|error| error.to_string())?;
    if installed == plugin_root || !installed.starts_with(&plugin_root) {
        return Err("Dandelion active package escaped its canonical PluginInstaller root".into());
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-dandelion-scanner-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn private_and_installed_roots_are_separate_and_symlinks_fail_closed() {
        let root = root();
        let plugin = root.join("plugins").join("app");
        let version = plugin.join("versions").join("1").join("artifact");
        fs::create_dir_all(&version).unwrap();
        assert_eq!(validate_installed_directory(&version, &plugin).unwrap(), fs::canonicalize(&version).unwrap());

        let private = root.join("private").join("identity");
        ensure_private_directory(&private).unwrap();
        let mode = fs::metadata(&private).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);

        let outside = root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let escape = plugin.join("versions").join("escape");
        symlink(&outside, &escape).unwrap();
        assert!(validate_installed_directory(&escape, &plugin).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
