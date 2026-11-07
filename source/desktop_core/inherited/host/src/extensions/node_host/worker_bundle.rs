//! Safe resolution of an installed node-worker bundle entry.
//!
//! Canonicalization is deliberately performed before containment checks so a
//! bundle symlink cannot escape the configured root. This helper resolves a
//! file only; worker process and container lifecycle remain separate owners.

use std::fs;
use std::path::{Path, PathBuf};

pub const WORKER_BUNDLE_ENTRY_PATH: &str = "worker.mjs";

pub fn resolve_worker_bundle_entry(
    bundle_root: &Path,
    gateway_namespace: &str,
    expected_bundle_hash: &str,
) -> Result<PathBuf, String> {
    validate_namespace(gateway_namespace)?;
    validate_bundle_hash(expected_bundle_hash)?;
    let root = fs::canonicalize(bundle_root)
        .map_err(|error| format!("node worker bundle root is unavailable: {error}"))?;
    let bundle = fs::canonicalize(
        root.join(gateway_namespace)
            .join("bundles")
            .join(expected_bundle_hash),
    )
    .map_err(|error| format!("node worker bundle is unavailable: {error}"))?;
    if !is_strictly_inside(&root, &bundle) {
        return Err("node worker bundle resolves outside its configured root".into());
    }
    let entry = fs::canonicalize(bundle.join(WORKER_BUNDLE_ENTRY_PATH))
        .map_err(|error| format!("node worker entry is unavailable: {error}"))?;
    if !is_strictly_inside(&bundle, &entry)
        || !fs::metadata(&entry)
            .map_err(|error| format!("node worker entry metadata is unavailable: {error}"))?
            .is_file()
    {
        return Err("node worker entry must be a regular file inside its bundle".into());
    }
    Ok(entry)
}

pub fn validate_namespace(namespace: &str) -> Result<(), String> {
    if namespace.is_empty()
        || namespace.len() > 128
        || !namespace
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        || !namespace
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
    {
        return Err("INVALID_REQUEST: invalid gatewayNamespace".into());
    }
    Ok(())
}

pub fn validate_bundle_hash(hash: &str) -> Result<(), String> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return Err("INVALID_REQUEST: expectedBundleHash must be 64 lowercase hexadecimal characters".into());
    }
    Ok(())
}

fn is_strictly_inside(root: &Path, target: &Path) -> bool {
    target.strip_prefix(root).is_ok_and(|relative| !relative.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{validate_bundle_hash, validate_namespace};

    #[test]
    fn bundle_components_are_bounded_and_lowercase() {
        assert!(validate_namespace("gw-main").is_ok());
        assert!(validate_namespace("../escape").is_err());
        assert!(validate_bundle_hash(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )
        .is_ok());
        assert!(validate_bundle_hash(&"A".repeat(64)).is_err());
    }
}
