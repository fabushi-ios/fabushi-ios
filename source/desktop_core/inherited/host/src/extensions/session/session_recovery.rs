use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::{Map, Value};

use crate::agents::agent_profile::{
    SandAgentProfile, get_sand_profile_path, write_sand_profile_file,
};
use crate::agents::settings_file::{get_sand_settings_path, write_sand_settings_file};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("conversation recovery scan failed: {detail}")]
pub struct ConversationRecoveryScanError {
    pub detail: String,
}

pub fn transcript_entry_matches_recovered(a: &Value, b: &Value) -> bool {
    if a.get("kind") != b.get("kind") {
        return false;
    }
    match a.get("kind").and_then(Value::as_str) {
        Some("message") => a.get("role") == b.get("role") && a.get("content") == b.get("content"),
        Some("send-message") => a.get("message") == b.get("message"),
        Some("tool-call") => {
            a.get("name") == b.get("name")
                && a.get("status") == b.get("status")
                && a.get("summary") == b.get("summary")
        }
        _ => false,
    }
}

pub type BlobReadResult = Result<Option<Vec<u8>>, String>;
pub type CachedBlobRead = Shared<BoxFuture<'static, BlobReadResult>>;
pub type BlobWriteFuture = BoxFuture<'static, Result<(), String>>;

type GetBlobSource =
    Arc<dyn Fn(Vec<u8>) -> BoxFuture<'static, BlobReadResult> + Send + Sync + 'static>;
type SetBlobSource =
    Arc<dyn Fn(Vec<u8>, Vec<u8>) -> BlobWriteFuture + Send + Sync + 'static>;
type FlushBlobSource =
    Arc<dyn Fn() -> BlobWriteFuture + Send + Sync + 'static>;

pub struct CachedBlobReadStore {
    reads: Mutex<HashMap<Vec<u8>, CachedBlobRead>>,
    get_source: GetBlobSource,
    set_source: SetBlobSource,
    flush_source: FlushBlobSource,
}

impl CachedBlobReadStore {
    pub fn get_blob(&self, blob_id: &[u8]) -> CachedBlobRead {
        let key = blob_id.to_vec();
        let mut reads = self
            .reads
            .lock()
            .expect("session recovery blob cache poisoned");
        if let Some(cached) = reads.get(&key) {
            return cached.clone();
        }
        let read = (self.get_source)(key.clone()).shared();
        reads.insert(key, read.clone());
        read
    }

    pub fn set_blob(&self, blob_id: &[u8], data: Vec<u8>) -> BlobWriteFuture {
        let key = blob_id.to_vec();
        let cached: CachedBlobRead =
            futures::future::ready::<BlobReadResult>(Ok(Some(data.clone())))
                .boxed()
                .shared();
        self.reads
            .lock()
            .expect("session recovery blob cache poisoned")
            .insert(key.clone(), cached);
        (self.set_source)(key, data)
    }

    pub fn flush(&self) -> BlobWriteFuture {
        (self.flush_source)()
    }
}

pub fn cache_blob_reads<Get, Set, Flush>(
    get_blob: Get,
    set_blob: Set,
    flush: Flush,
) -> CachedBlobReadStore
where
    Get: Fn(Vec<u8>) -> BoxFuture<'static, BlobReadResult> + Send + Sync + 'static,
    Set: Fn(Vec<u8>, Vec<u8>) -> BlobWriteFuture + Send + Sync + 'static,
    Flush: Fn() -> BlobWriteFuture + Send + Sync + 'static,
{
    CachedBlobReadStore {
        reads: Mutex::new(HashMap::new()),
        get_source: Arc::new(get_blob),
        set_source: Arc::new(set_blob),
        flush_source: Arc::new(flush),
    }
}

pub fn ensure_profile_file(
    db_path: impl AsRef<Path>,
    agent_name: Option<&str>,
    description: &str,
) -> io::Result<PathBuf> {
    let db_path = db_path.as_ref();
    let agent_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let path = get_sand_profile_path(agent_dir);
    if path.exists() {
        return Ok(path);
    }
    let name = agent_name.unwrap_or_default().trim();
    write_sand_profile_file(
        &path,
        &SandAgentProfile {
            name: if name.is_empty() {
                "Grok".to_string()
            } else {
                name.to_string()
            },
            description: description.trim().to_string(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )?;
    Ok(path)
}

pub fn ensure_settings_file(db_path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let db_path = db_path.as_ref();
    let agent_dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let path = get_sand_settings_path(agent_dir);
    if !path.exists() {
        write_sand_settings_file(&path, &Map::new())?;
    }
    Ok(path)
}

pub fn boxed_blob_future<F>(future: F) -> BlobWriteFuture
where
    F: Future<Output = Result<(), String>> + Send + 'static,
{
    future.boxed()
}
