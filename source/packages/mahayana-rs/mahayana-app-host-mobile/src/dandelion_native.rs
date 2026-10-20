//! iOS-owned Dandelion native source owner.
//!
//! Source semantics are ported from bhrumom/global-dandelion-rs
//! ccbe267f5fedccac07494cd5cebf7e52d0fcbed9. This module intentionally owns
//! only the static native source/grant boundary. The signed Mini App
//! \`native.runtime\` capability remains fail-closed until the package execution
//! adapter is wired through this owner.
use crate::dandelion_static_runner::{
    CanonicalPackageSnapshot, RuntimeGrant, RunnerLease, StaticRunnerRegistry,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const MAX_GRANTS: usize = 16;
const MAX_COMPONENTS: usize = 64;
const MAX_PROBE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct AccountLease {
    scope: String,
    generation: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    dev: u64,
    ino: u64,
    mode: u32,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl FileStamp {
    fn from_file(file: &File) -> Result<Self, String> {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        Ok(Self {
            len: metadata.len(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            mode: metadata.mode(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        })
    }
}

struct SourceRoot {
    directory: File,
    device: u64,
}

impl SourceRoot {
    fn open(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("Dandelion source root must be an absolute picker URL path".into());
        }
        let encoded = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Dandelion source root contains a NUL byte".to_string())?;
        let descriptor = unsafe {
            libc::open(
                encoded.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let directory = unsafe { File::from_raw_fd(descriptor) };
        let metadata = directory.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Err("Dandelion source root is not a directory".into());
        }
        Ok(Self {
            directory,
            device: metadata.dev(),
        })
    }

    fn open_regular(&self, relative: &Path) -> Result<File, String> {
        if relative.is_absolute() || relative.as_os_str().as_bytes().len() > 4096 {
            return Err("Dandelion source path must be a bounded relative path".into());
        }
        let mut components = relative.components().peekable();
        let mut parent = self
            .directory
            .try_clone()
            .map_err(|error| error.to_string())?;
        let mut depth = 0usize;
        while let Some(component) = components.next() {
            depth += 1;
            if depth > MAX_COMPONENTS {
                return Err("Dandelion source path exceeds component limit".into());
            }
            let Component::Normal(name) = component else {
                return Err("Dandelion source path contains a non-normal component".into());
            };
            let encoded = CString::new(name.as_bytes())
                .map_err(|_| "Dandelion source component contains a NUL byte".to_string())?;
            let last = components.peek().is_none();
            let mut flags =
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
            if !last {
                flags |= libc::O_DIRECTORY;
            }
            let descriptor = unsafe { libc::openat(parent.as_raw_fd(), encoded.as_ptr(), flags) };
            if descriptor < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let file = unsafe { File::from_raw_fd(descriptor) };
            let metadata = file.metadata().map_err(|error| error.to_string())?;
            if metadata.dev() != self.device {
                return Err("Dandelion source crossed the selected filesystem device".into());
            }
            if last {
                if !metadata.is_file() {
                    return Err("Dandelion source is not a regular file".into());
                }
                return Ok(file);
            }
            if !metadata.is_dir() {
                return Err("Dandelion source intermediate component is not a directory".into());
            }
            parent = file;
        }
        Err("Dandelion source path is empty".into())
    }
}

struct SourceGrant {
    root: SourceRoot,
}

#[derive(Default)]
pub struct DandelionNativeOwner {
    account: Option<AccountLease>,
    grants: BTreeMap<String, SourceGrant>,
    runtime_grants: BTreeMap<String, RuntimeGrant>,
    runners: StaticRunnerRegistry,
    revision: u64,
}

impl DandelionNativeOwner {
    pub fn sync_account(&mut self, scope: &str, generation: &str) -> Result<(), String> {
        validate_identity(scope, "accountScope")?;
        validate_identity(generation, "accountGeneration")?;
        let next = AccountLease {
            scope: scope.to_owned(),
            generation: generation.to_owned(),
        };
        if self.account.as_ref() != Some(&next) {
            self.runners.clear("Dandelion account lease changed");
            self.runtime_grants.clear();
            self.grants.clear();
            self.account = Some(next);
            self.bump_revision();
        }
        Ok(())
    }

    pub fn clear_account(&mut self) {
        let changed = self.account.is_some()
            || !self.grants.is_empty()
            || !self.runtime_grants.is_empty()
            || self.runners.active_count() != 0;
        self.runners.clear("Dandelion account lease cleared");
        self.runtime_grants.clear();
        self.grants.clear();
        self.account = None;
        if changed {
            self.bump_revision();
        }
    }

    pub fn authorize_runtime(
        &mut self,
        grant_token: &str,
        app_id: &str,
        version: &str,
        package_digest: &str,
    ) -> Result<RuntimeGrant, String> {
        let account = self.require_account()?.clone();
        validate_runtime_token(grant_token, "grantToken")?;
        validate_runtime_token(app_id, "appId")?;
        validate_runtime_token(version, "version")?;
        validate_lowercase_digest(package_digest, "packageDigest")?;
        let grant = RuntimeGrant {
            token: grant_token.to_string(),
            account_scope: account.scope,
            account_generation: account.generation,
            app_id: app_id.to_string(),
            version: version.to_string(),
            package_digest: package_digest.to_string(),
        };
        if self.runtime_grants.len() >= 32 && !self.runtime_grants.contains_key(grant_token) {
            return Err("Dandelion runtime grant limit reached".into());
        }
        self.runtime_grants.insert(grant_token.to_string(), grant.clone());
        self.bump_revision();
        Ok(grant)
    }

    pub fn start_static_runner(
        &mut self,
        grant_token: &str,
        package: &CanonicalPackageSnapshot,
        plugin_root: &Path,
        private_base: &Path,
    ) -> Result<RunnerLease, String> {
        let account = self.require_account()?.clone();
        let grant = self
            .runtime_grants
            .get(grant_token)
            .cloned()
            .ok_or_else(|| "Dandelion runtime grant is unavailable".to_string())?;
        if grant.account_scope != account.scope || grant.account_generation != account.generation {
            return Err("Dandelion runtime grant belongs to a stale account lease".into());
        }
        let lease = self.runners.start(&grant, package, plugin_root, private_base)?;
        self.bump_revision();
        Ok(lease)
    }

    pub fn next_static_platform_request(
        &self,
        runner_id: &str,
    ) -> Result<Option<mahayana_native_engine::PlatformExecRequest>, String> {
        self.runners.next_platform_request(runner_id)
    }

    pub fn resolve_static_platform_request(
        &self,
        runner_id: &str,
        request_id: &str,
        result: Value,
    ) -> Result<(), String> {
        self.runners
            .resolve_platform_request(runner_id, request_id, result)
    }

    pub fn reject_static_platform_request(
        &self,
        runner_id: &str,
        request_id: &str,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        self.runners
            .reject_platform_request(runner_id, request_id, code, message)
    }

    pub fn static_runner_status(&self, runner_id: &str) -> Result<Value, String> {
        self.runners.status(runner_id)
    }

    pub fn cancel_static_runner(&mut self, runner_id: &str, reason: &str) -> Result<bool, String> {
        let cancelled = self.runners.cancel(runner_id, reason)?;
        if cancelled {
            self.bump_revision();
        }
        Ok(cancelled)
    }

    pub fn finish_static_runner(&mut self, runner_id: &str) -> Result<bool, String> {
        let finished = self.runners.finish(runner_id)?;
        if finished {
            self.bump_revision();
        }
        Ok(finished)
    }

    pub fn register_source(&mut self, grant_id: &str, path: &Path) -> Result<(), String> {
        self.require_account()?;
        validate_grant_id(grant_id)?;
        if !self.grants.contains_key(grant_id) && self.grants.len() >= MAX_GRANTS {
            return Err("Dandelion source grant limit reached".into());
        }
        let root = SourceRoot::open(path)?;
        self.grants.insert(grant_id.to_owned(), SourceGrant { root });
        self.bump_revision();
        Ok(())
    }

    pub fn revoke_source(&mut self, grant_id: &str) -> Result<bool, String> {
        validate_grant_id(grant_id)?;
        let removed = self.grants.remove(grant_id).is_some();
        if removed {
            self.bump_revision();
        }
        Ok(removed)
    }

    pub fn revoke_all(&mut self) {
        let changed = !self.grants.is_empty()
            || !self.runtime_grants.is_empty()
            || self.runners.active_count() != 0;
        self.runners.clear("Dandelion native grants revoked");
        self.runtime_grants.clear();
        self.grants.clear();
        if changed {
            self.bump_revision();
        }
    }

    pub fn probe_file(
        &self,
        grant_id: &str,
        relative_path: &str,
        max_bytes: u64,
    ) -> Result<serde_json::Value, String> {
        self.require_account()?;
        validate_grant_id(grant_id)?;
        if max_bytes == 0 || max_bytes > MAX_PROBE_BYTES {
            return Err("Dandelion probe byte limit is invalid".into());
        }
        let grant = self
            .grants
            .get(grant_id)
            .ok_or_else(|| "Dandelion source grant is unavailable".to_string())?;
        let relative = PathBuf::from(relative_path);
        let mut file = grant.root.open_regular(&relative)?;
        let before = FileStamp::from_file(&file)?;
        if before.len > max_bytes {
            return Err("Dandelion source exceeds the requested byte limit".into());
        }
        let capacity = usize::try_from(before.len)
            .map_err(|_| "Dandelion source length is not representable".to_string())?;
        let mut payload = Vec::with_capacity(capacity);
        file.read_to_end(&mut payload)
            .map_err(|error| error.to_string())?;
        if payload.len() as u64 != before.len || FileStamp::from_file(&file)? != before {
            return Err("Dandelion source changed while reading".into());
        }
        let digest = Sha256::digest(&payload)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(serde_json::json!({
            "length": before.len,
            "sha256": digest,
            "revision": self.revision,
        }))
    }

    pub fn status_value(&self) -> serde_json::Value {
        serde_json::json!({
            "protocol": "fabushi.ios.dandelion-native-owner.v1",
            "accountBound": self.account.is_some(),
            "grantCount": self.grants.len(),
            "runtimeGrantCount": self.runtime_grants.len(),
            "staticRunnerCount": self.runners.active_count(),
            "revision": self.revision,
            "sourceAuthority": "bhrumom/global-dandelion-rs@ccbe267f5fedccac07494cd5cebf7e52d0fcbed9",
            "portableFoundation": crate::dandelion_portable::SOURCE_AUTHORITY,
            "nativeRuntimeEnabled": false,
        })
    }

    fn require_account(&self) -> Result<&AccountLease, String> {
        self.account
            .as_ref()
            .ok_or_else(|| "Dandelion native owner has no account lease".to_string())
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if self.revision == 0 {
            self.revision = 1;
        }
    }
}

fn validate_identity(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{field} must be a 64-character hexadecimal identity"));
    }
    Ok(())
}

fn validate_runtime_token(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("{field} is invalid"));
    }
    Ok(())
}

fn validate_lowercase_digest(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("{field} must be lowercase 64-byte hexadecimal"));
    }
    Ok(())
}

fn validate_grant_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Dandelion source grant id is invalid".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn temp_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-dandelion-owner-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn account_switch_revokes_sources_and_probe_is_bounded() {
        let root = temp_root();
        fs::write(root.join("data.txt"), b"bodhi").unwrap();
        let mut owner = DandelionNativeOwner::default();
        owner.sync_account(&"a".repeat(64), &"b".repeat(64)).unwrap();
        owner.register_source("grant-1", &root).unwrap();
        let probe = owner.probe_file("grant-1", "data.txt", 64).unwrap();
        assert_eq!(probe["length"], 5);
        assert_eq!(
            probe["sha256"],
            "20c065202e0ea69cb73337e90f5b95ab6bec3d61d09fa025c8f23bc33eb312cf"
        );
        owner.sync_account(&"c".repeat(64), &"d".repeat(64)).unwrap();
        assert_eq!(owner.status_value()["grantCount"], 0);
        assert!(owner.probe_file("grant-1", "data.txt", 64).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_grants_are_account_version_digest_and_token_fenced() {
        let mut owner = DandelionNativeOwner::default();
        owner.sync_account(&"a".repeat(64), &"b".repeat(64)).unwrap();
        let grant = owner
            .authorize_runtime("grant-1", "global-dharma", "1.2.3", &"c".repeat(64))
            .unwrap();
        assert_eq!(grant.account_scope, "a".repeat(64));
        assert!(owner
            .authorize_runtime("../escape", "global-dharma", "1.2.3", &"c".repeat(64))
            .is_err());
        assert!(owner
            .authorize_runtime("grant-2", "global-dharma", "1.2.3", &"C".repeat(64))
            .is_err());
        owner.sync_account(&"d".repeat(64), &"e".repeat(64)).unwrap();
        assert_eq!(owner.status_value()["runtimeGrantCount"], 0);
    }

    #[test]
    fn held_root_resolution_rejects_parent_and_symlink_escape() {
        let root = temp_root();
        let outside = temp_root();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        symlink(outside.join("secret.txt"), root.join("escape")).unwrap();
        let mut owner = DandelionNativeOwner::default();
        owner.sync_account(&"a".repeat(64), &"b".repeat(64)).unwrap();
        owner.register_source("grant-2", &root).unwrap();
        assert!(owner.probe_file("grant-2", "../secret.txt", 64).is_err());
        assert!(owner.probe_file("grant-2", "escape", 64).is_err());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }
}
