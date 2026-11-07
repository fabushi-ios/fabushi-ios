use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use fs2::FileExt;

use super::agent_store_sand_files::{
    AgentStoreClient, AgentStoreClientDependencies, AgentStoreObjectProbe, AgentStoreReadObject,
    AgentStoreWriteOutcome, AgentStoreWritePrecondition, normalize_rel_path,
};

use super::object_store_port::{
    BoxStoreCanonicalWriteConflictError, BoxStoreConditionalWriteOutcome,
};
use super::sand_box_store_files::SandBoxStoreServiceProvider;
use super::sand_box_store_v2_client::{
    SandBoxStoreV2Client, SandBoxStoreV2ClientDependencies,
};
use crate::r#box::box_store_backend_policy::{
    BoxStoreBackendKind, resolve_box_store_backend_policy,
};

pub trait BoxObjectStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String>;

    fn put_if_unchanged(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<bool, String> {
        if self.get(key)?.as_deref() != expected {
            return Ok(false);
        }
        self.put(key, bytes)?;
        Ok(true)
    }

    fn put_if_unchanged_detailed(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<BoxStoreConditionalWriteOutcome, String> {
        if self.put_if_unchanged(key, expected, bytes)? {
            Ok(BoxStoreConditionalWriteOutcome::Written)
        } else {
            Ok(BoxStoreConditionalWriteOutcome::Conflict(
                BoxStoreCanonicalWriteConflictError::new(
                    key,
                    None,
                    None,
                    Some("compare-and-swap".into()),
                ),
            ))
        }
    }

    fn get_to_file(
        &self,
        key: &str,
        dest_path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<Option<u64>, String>;
    fn prefetch_reads(&self, _keys: &[String]) -> Result<(), String> {
        Ok(())
    }
    fn put_from_file(&self, key: &str, src_path: &Path) -> Result<(), String>;
    fn list(&self, prefix: &str) -> Result<Vec<String>, String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

pub trait BoxObjectStoreProvider: Send + Sync {
    fn for_store(&self, store_id: &str) -> Box<dyn BoxObjectStore>;
}

#[derive(Clone, Default)]
pub struct BoxObjectStoreProviderDependencies {
    pub native_credentials: Option<Arc<dyn crate::extensions::auth::auth_service::FabushiCoreCredentialProvider>>,
    pub backend_url: Option<String>,
    pub get_access_token: Option<Arc<dyn Fn() -> Result<String, String> + Send + Sync>>,
    pub get_machine_id: Option<Arc<dyn Fn() -> Result<String, String> + Send + Sync>>,
}

pub fn resolve_box_object_store_provider(
    environment: &std::collections::BTreeMap<String, String>,
    deps: BoxObjectStoreProviderDependencies,
) -> Result<Arc<dyn BoxObjectStoreProvider>, String> {
    let policy = resolve_box_store_backend_policy(environment);
    match policy.kind {
        BoxStoreBackendKind::Native => {
            let base = deps.backend_url.ok_or("Native BoxStore backend URL is not configured")?;
            let credentials = deps.native_credentials.ok_or("Native BoxStore credential provider is not configured")?;
            Ok(Arc::new(super::native_box_object_store::NativeBoxObjectStoreProvider::new(&base, credentials)?))
        }
        BoxStoreBackendKind::LocalFs => {
            let base_dir = policy
                .local_dir
                .ok_or_else(|| "local BoxStore backend selected without an absolute local directory".to_string())?;
            Ok(Arc::new(LocalFsObjectStoreProvider::new(base_dir)))
        }
        BoxStoreBackendKind::AgentStore => {
            let remote = remote_provider_dependencies("AgentStore", deps)?;
            Ok(Arc::new(AgentStoreObjectStoreProvider::new(remote)?))
        }
        BoxStoreBackendKind::SandBoxStoreV2 => {
            let remote = remote_provider_dependencies("SandBoxStoreV2", deps)?;
            let client = Arc::new(SandBoxStoreV2Client::new(SandBoxStoreV2ClientDependencies {
                backend_url: remote.backend_url,
                get_access_token: remote.get_access_token,
                get_machine_id: remote.get_machine_id,
            }));
            Ok(Arc::new(SandBoxStoreServiceProvider::new(client)?))
        }
    }
}

fn remote_provider_dependencies(
    backend_name: &str,
    deps: BoxObjectStoreProviderDependencies,
) -> Result<AgentStoreClientDependencies, String> {
    Ok(AgentStoreClientDependencies {
        backend_url: deps
            .backend_url
            .ok_or_else(|| format!("{backend_name} backend URL is not configured"))?,
        get_access_token: deps
            .get_access_token
            .ok_or_else(|| format!("{backend_name} auth token resolver is not configured"))?,
        get_machine_id: deps
            .get_machine_id
            .ok_or_else(|| format!("{backend_name} machine id resolver is not configured"))?,
    })
}

#[derive(Clone)]
pub struct AgentStoreObjectStoreProvider {
    client: AgentStoreClient,
}

impl AgentStoreObjectStoreProvider {
    pub fn new(deps: AgentStoreClientDependencies) -> Result<Self, String> {
        Ok(Self {
            client: AgentStoreClient::new(deps)?,
        })
    }
}

impl BoxObjectStoreProvider for AgentStoreObjectStoreProvider {
    fn for_store(&self, store_id: &str) -> Box<dyn BoxObjectStore> {
        Box::new(AgentStoreObjectStore::new(
            self.client.clone(),
            store_id.to_string(),
        ))
    }
}

#[derive(Debug, Clone)]
enum AgentStoreBaseline {
    Absent,
    Present { etag: String, digest: [u8; 32] },
}

pub struct AgentStoreObjectStore {
    client: AgentStoreClient,
    source_id: String,
    baselines: Mutex<HashMap<String, AgentStoreBaseline>>,
}

impl AgentStoreObjectStore {
    pub fn new(client: AgentStoreClient, source_id: String) -> Self {
        Self {
            client,
            source_id,
            baselines: Mutex::new(HashMap::new()),
        }
    }

    fn remember_read(&self, key: &str, object: Option<&AgentStoreReadObject>) {
        let baseline = match object {
            None => Some(AgentStoreBaseline::Absent),
            Some(object) => object.etag.as_ref().map(|etag| AgentStoreBaseline::Present {
                etag: etag.clone(),
                digest: Sha256::digest(&object.bytes).into(),
            }),
        };
        let mut baselines = self
            .baselines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match baseline {
            Some(baseline) => {
                baselines.insert(key.to_string(), baseline);
            }
            None => {
                baselines.remove(key);
            }
        }
    }

    fn baseline_for_expected(
        &self,
        key: &str,
        expected: Option<&[u8]>,
    ) -> Option<AgentStoreWritePrecondition> {
        let baselines = self
            .baselines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match (baselines.get(key), expected) {
            (Some(AgentStoreBaseline::Absent), None) => {
                Some(AgentStoreWritePrecondition::ExpectAbsent)
            }
            (Some(AgentStoreBaseline::Present { etag, digest }), Some(expected))
                if *digest == <[u8; 32]>::from(Sha256::digest(expected)) =>
            {
                Some(AgentStoreWritePrecondition::BaseEtag(etag.clone()))
            }
            _ => None,
        }
    }

    fn known_write_precondition(&self, key: &str) -> Option<AgentStoreWritePrecondition> {
        let baselines = self
            .baselines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match baselines.get(key) {
            Some(AgentStoreBaseline::Absent) => Some(AgentStoreWritePrecondition::ExpectAbsent),
            Some(AgentStoreBaseline::Present { etag, .. }) => {
                Some(AgentStoreWritePrecondition::BaseEtag(etag.clone()))
            }
            None => None,
        }
    }

    fn probe_precondition(&self, key: &str) -> Result<AgentStoreWritePrecondition, String> {
        match self.client.probe_object(&self.source_id, key)? {
            AgentStoreObjectProbe::Absent => Ok(AgentStoreWritePrecondition::ExpectAbsent),
            AgentStoreObjectProbe::Present { etag } => {
                Ok(AgentStoreWritePrecondition::BaseEtag(etag))
            }
        }
    }

    fn remember_write(&self, key: &str, bytes: &[u8], outcome: &AgentStoreWriteOutcome) {
        let mut baselines = self
            .baselines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match outcome {
            AgentStoreWriteOutcome::Written { etag: Some(etag) } => {
                baselines.insert(
                    key.to_string(),
                    AgentStoreBaseline::Present {
                        etag: etag.clone(),
                        digest: Sha256::digest(bytes).into(),
                    },
                );
            }
            _ => {
                baselines.remove(key);
            }
        }
    }
}

impl BoxObjectStore for AgentStoreObjectStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let object = self.client.get_object(&self.source_id, key)?;
        self.remember_read(key, object.as_ref());
        Ok(object.map(|object| object.bytes))
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let precondition = match self.known_write_precondition(key) {
            Some(precondition) => precondition,
            None => self.probe_precondition(key)?,
        };
        let outcome = self
            .client
            .put_bytes(&self.source_id, key, bytes, precondition)?;
        self.remember_write(key, bytes, &outcome);
        match outcome {
            AgentStoreWriteOutcome::Written { .. } => Ok(()),
            AgentStoreWriteOutcome::AlreadyPresent => Ok(()),
            AgentStoreWriteOutcome::Conflict {
                conflict_rel_path, ..
            } => Err(match conflict_rel_path {
                Some(path) => format!(
                    "agent-store write for {key} lost a concurrent-write race; content preserved at {path}"
                ),
                None => format!(
                    "agent-store write for {key} lost a concurrent-write race"
                ),
            }),
        }
    }

    fn put_if_unchanged(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let Some(precondition) = self.baseline_for_expected(key, expected) else {
            return Ok(false);
        };
        let outcome = self
            .client
            .put_bytes(&self.source_id, key, bytes, precondition)?;
        self.remember_write(key, bytes, &outcome);
        Ok(matches!(
            outcome,
            AgentStoreWriteOutcome::Written { .. } | AgentStoreWriteOutcome::AlreadyPresent
        ))
    }

    fn put_if_unchanged_detailed(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<BoxStoreConditionalWriteOutcome, String> {
        let Some(precondition) = self.baseline_for_expected(key, expected) else {
            return Ok(BoxStoreConditionalWriteOutcome::Conflict(
                BoxStoreCanonicalWriteConflictError::new(
                    key,
                    None,
                    None,
                    Some("cached-baseline-mismatch".into()),
                ),
            ));
        };
        let baseline_source = match &precondition {
            AgentStoreWritePrecondition::BaseEtag(_) => "agent-store-etag",
            AgentStoreWritePrecondition::ExpectAbsent => "agent-store-expect-absent",
        };
        let fallback_base_etag = match &precondition {
            AgentStoreWritePrecondition::BaseEtag(etag) => Some(etag.clone()),
            AgentStoreWritePrecondition::ExpectAbsent => None,
        };
        let outcome = self
            .client
            .put_bytes(&self.source_id, key, bytes, precondition)?;
        self.remember_write(key, bytes, &outcome);
        match outcome {
            AgentStoreWriteOutcome::Written { .. } | AgentStoreWriteOutcome::AlreadyPresent => {
                Ok(BoxStoreConditionalWriteOutcome::Written)
            }
            AgentStoreWriteOutcome::Conflict {
                conflict_rel_path,
                base_etag,
            } => Ok(BoxStoreConditionalWriteOutcome::Conflict(
                BoxStoreCanonicalWriteConflictError::new(
                    key,
                    conflict_rel_path,
                    base_etag.or(fallback_base_etag),
                    Some(baseline_source.into()),
                ),
            )),
        }
    }

    fn get_to_file(
        &self,
        key: &str,
        dest_path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<Option<u64>, String> {
        self.client
            .get_object_to_file(&self.source_id, key, dest_path, max_bytes)
    }

    fn put_from_file(&self, key: &str, src_path: &Path) -> Result<(), String> {
        match self
            .client
            .put_file_content_addressed(&self.source_id, key, src_path)?
        {
            AgentStoreWriteOutcome::Written { .. }
            | AgentStoreWriteOutcome::AlreadyPresent => Ok(()),
            AgentStoreWriteOutcome::Conflict { .. } => Err(format!(
                "content-addressed agent-store write unexpectedly conflicted for {key}"
            )),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        self.client.list_objects(&self.source_id, prefix)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        Err(format!(
            "AgentStoreObjectStore is append-only; cannot delete {key}"
        ))
    }
}

#[derive(Debug, Clone)]
pub struct LocalFsObjectStoreProvider {
    base_dir: PathBuf,
}

impl LocalFsObjectStoreProvider {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }
}

impl BoxObjectStoreProvider for LocalFsObjectStoreProvider {
    fn for_store(&self, store_id: &str) -> Box<dyn BoxObjectStore> {
        let normalized = normalize_rel_path(store_id)
            .unwrap_or_else(|_| "invalid-store-id".to_string());
        Box::new(LocalFsObjectStore::new(self.base_dir.join(normalized)))
    }
}

#[derive(Debug, Clone)]
pub struct LocalFsObjectStore {
    root: PathBuf,
}

impl LocalFsObjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, String> {
        let normalized = normalize_rel_path(key)?;
        Ok(self.root.join(normalized))
    }
}

impl BoxObjectStore for LocalFsObjectStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match fs::read(self.path_for(key)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, bytes).map_err(|error| error.to_string())
    }

    fn put_if_unchanged(
        &self,
        key: &str,
        expected: Option<&[u8]>,
        bytes: &[u8],
    ) -> Result<bool, String> {
        let path = self.path_for(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        let mut lock_os = path.as_os_str().to_os_string();
        lock_os.push(".lock");
        let lock_path = PathBuf::from(lock_os);
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| error.to_string())?;
        lock.lock_exclusive().map_err(|error| error.to_string())?;

        let current = match fs::read(&path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                let _ = FileExt::unlock(&lock);
                return Err(error.to_string());
            }
        };
        if current.as_deref() != expected {
            let _ = FileExt::unlock(&lock);
            return Ok(false);
        }

        let mut temp_os = path.as_os_str().to_os_string();
        temp_os.push(".conditional-write.tmp");
        let temp_path = PathBuf::from(temp_os);
        let result = (|| -> Result<(), String> {
            fs::write(&temp_path, bytes).map_err(|error| error.to_string())?;
            fs::rename(&temp_path, &path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        let unlock = FileExt::unlock(&lock).map_err(|error| error.to_string());
        result?;
        unlock?;
        Ok(true)
    }

    fn get_to_file(
        &self,
        key: &str,
        dest_path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let src = self.path_for(key)?;
        let metadata = match fs::metadata(&src) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if max_bytes.is_some_and(|max| metadata.len() > max) {
            return Err(format!(
                "object {} exceeds maximum restore bytes",
                metadata.len()
            ));
        }
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(src, dest_path)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn put_from_file(&self, key: &str, src_path: &Path) -> Result<(), String> {
        let dest = self.path_for(key)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(src_path, dest)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let normalized_prefix = if prefix.trim().is_empty() {
            String::new()
        } else {
            normalize_rel_path(prefix)?
        };
        let mut files = Vec::new();
        if !self.root.exists() {
            return Ok(files);
        }
        walk_files(&self.root, &self.root, &mut files).map_err(|error| error.to_string())?;
        files.retain(|key| is_under_prefix(key, &normalized_prefix));
        files.sort();
        Ok(files)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match fs::remove_file(self.path_for(key)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

pub fn is_under_prefix(key: &str, prefix: &str) -> bool {
    prefix.is_empty() || key == prefix || key.starts_with(&format!("{prefix}/"))
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_files(root, &path, out)?;
        } else if file_type.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_resolver_preserves_frozen_backend_selection_and_fails_closed() {
        let local_root = std::env::temp_dir().join(format!(
            "fabushi-box-object-store-provider-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let local_env = std::collections::BTreeMap::from([(
            crate::r#box::box_store_backend_policy::SAND_BOX_STORE_LOCAL_DIR_ENV.to_string(),
            local_root.to_string_lossy().into_owned(),
        )]);
        let local = resolve_box_object_store_provider(
            &local_env,
            BoxObjectStoreProviderDependencies::default(),
        )
        .expect("local provider requires no remote auth");
        local
            .for_store("agentStore:test")
            .put("probe.txt", b"ok")
            .expect("local provider write");
        assert_eq!(
            fs::read(local_root.join("agentStore:test/probe.txt")).expect("local provider bytes"),
            b"ok"
        );
        fs::remove_dir_all(&local_root).expect("cleanup local provider");

        let v2_env = std::collections::BTreeMap::from([(
            crate::r#box::box_store_backend_policy::SAND_BOX_STORE_BACKEND_ENV.to_string(),
            "v2".to_string(),
        )]);
        let error = match resolve_box_object_store_provider(
            &v2_env,
            BoxObjectStoreProviderDependencies::default(),
        ) {
            Ok(_) => panic!("v2 without auth must fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("SandBoxStoreV2 backend URL"));

        let agent_env = std::collections::BTreeMap::new();
        let error = match resolve_box_object_store_provider(
            &agent_env,
            BoxObjectStoreProviderDependencies::default(),
        ) {
            Ok(_) => panic!("agent-store without auth must fail closed"),
            Err(error) => error,
        };
        assert!(error.contains("AgentStore backend URL"));
    }

    #[test]
    fn local_fs_conditional_put_rejects_stale_manifest_baseline() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-box-object-store-cas-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = LocalFsObjectStore::new(&root);
        let key = "manifest.json";

        assert!(store.put_if_unchanged(key, None, b"v1").expect("create v1"));
        let baseline = store.get(key).expect("read v1").expect("v1 exists");

        store.put(key, b"concurrent-v2").expect("concurrent writer");
        assert!(
            !store
                .put_if_unchanged(key, Some(&baseline), b"stale-v3")
                .expect("conditional stale write")
        );
        assert_eq!(
            store.get(key).expect("read winner").as_deref(),
            Some(b"concurrent-v2".as_slice())
        );

        fs::remove_dir_all(root).expect("cleanup");
    }
}
