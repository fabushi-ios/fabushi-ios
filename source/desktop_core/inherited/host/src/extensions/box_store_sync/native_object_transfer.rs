//! Streaming codec over the existing native account store, not a second store.
//! Downloads must target an unpublished caller-owned staging writer.
use std::io::{Read, Write};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::agent_store_sand_files::normalize_rel_path;
use super::native_account_store::{NativeAccountStoreClient, NativeDeleteOutcome, NativeWriteOutcome, NativeWritePrecondition};

pub const NATIVE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_NATIVE_OBJECT_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_CHUNKS: usize = 8192;
const PROTOCOL: &str = "fabushi.box.object.v1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Chunk { sha256: String, size: usize }
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest { protocol: String, key: String, size: u64, sha256: String, chunks: Vec<Chunk> }
#[derive(Clone)]
pub struct NativeObjectHead {
    pub etag: String,
    manifest: Manifest,
    // Prevent callers from passing a version acquired for a different owner.
    owner: Arc<NativeAccountStoreClient>,
    agent: String,
}
impl NativeObjectHead {
    pub fn size(&self) -> u64 { self.manifest.size }
    pub fn sha256(&self) -> &str { &self.manifest.sha256 }
}
pub struct NativeObjectTransfer {
    client: Arc<NativeAccountStoreClient>,
    agent: String,
}
fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn valid_sha(value: &str) -> bool { value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn root_key(key: &str) -> Result<String, String> {
    if key.len() > 4096 || normalize_rel_path(key).ok().as_deref() != Some(key) { return Err("Invalid native object key.".into()); }
    Ok(format!(".fabushi-box-v1/refs/{}.json", sha(key.as_bytes())))
}
fn chunk_key(digest: &str) -> String { format!(".fabushi-chunks-v1/{digest}") }
impl Manifest {
    fn validate(&self, key: &str) -> Result<(), String> {
        if self.protocol != PROTOCOL || self.key != key || self.size > MAX_NATIVE_OBJECT_BYTES
            || !valid_sha(&self.sha256) || self.chunks.len() > MAX_CHUNKS {
            return Err("Invalid native object manifest.".into());
        }
        let mut total = 0u64;
        for (i, chunk) in self.chunks.iter().enumerate() {
            if !valid_sha(&chunk.sha256) || chunk.size == 0 || chunk.size > NATIVE_CHUNK_BYTES
                || (i + 1 != self.chunks.len() && chunk.size != NATIVE_CHUNK_BYTES) {
                return Err("Invalid native object chunk description.".into());
            }
            total = total.checked_add(chunk.size as u64).ok_or("Native object size overflow.")?;
        }
        if total != self.size || (self.size == 0 && self.sha256 != sha(b"")) { return Err("Native object size/hash mismatch.".into()); }
        Ok(())
    }
}
impl NativeObjectTransfer {
    pub fn new(client: Arc<NativeAccountStoreClient>, agent: String) -> Self { Self { client, agent } }
    pub fn ensure_current_account(&self) -> Result<(), String> { self.client.ensure_current_account().map_err(|e|e.to_string()) }
    pub fn head(&self, key: &str) -> Result<Option<NativeObjectHead>, String> {
        let Some(root) = self.client.get(&self.agent, &root_key(key)?).map_err(|e|e.to_string())? else { return Ok(None); };
        if root.bytes.len() > 1024 * 1024 { return Err("Native object manifest exceeds limit.".into()); }
        let manifest: Manifest = serde_json::from_slice(&root.bytes).map_err(|_|"Invalid native object manifest JSON.")?;
        manifest.validate(key)?;
        Ok(Some(NativeObjectHead { etag: root.etag, manifest, owner: self.client.clone(), agent: self.agent.clone() }))
    }
    pub fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let mut keys = std::collections::BTreeSet::new();
        for path in self.client.list(&self.agent, ".fabushi-box-v1/refs").map_err(|e|e.to_string())? {
            let Some(root) = self.client.get(&self.agent, &path).map_err(|e|e.to_string())? else { continue; };
            if root.bytes.len() > 1024 * 1024 { return Err("Native object manifest exceeds limit.".into()); }
            let manifest: Manifest = serde_json::from_slice(&root.bytes).map_err(|_|"Invalid native object manifest JSON.")?;
            manifest.validate(&manifest.key)?;
            if root_key(&manifest.key)? != path { return Err("Native object key binding mismatch.".into()); }
            if manifest.key.starts_with(prefix) { keys.insert(manifest.key); }
        }
        Ok(keys.into_iter().collect())
    }
    pub fn delete(&self, key: &str, etag: &str) -> Result<NativeDeleteOutcome, String> {
        self.client.delete(&self.agent, &root_key(key)?, etag).map_err(|e|e.to_string())
    }
    pub fn download(&self, head: &NativeObjectHead, staging: &mut impl Write, max_bytes: u64) -> Result<u64, String> {
        if !Arc::ptr_eq(&head.owner, &self.client) || head.agent != self.agent { return Err("Native object owner mismatch.".into()); }
        self.client.ensure_current_account().map_err(|e|e.to_string())?;
        head.manifest.validate(&head.manifest.key)?;
        if head.manifest.size > max_bytes { return Err("Native object exceeds requested download limit.".into()); }
        let mut hash = Sha256::new();
        for chunk in &head.manifest.chunks {
            let object = self.client.get(&self.agent, &chunk_key(&chunk.sha256)).map_err(|e|e.to_string())?
                .ok_or("Native object chunk is missing; root was not materialized.")?;
            if object.bytes.len() != chunk.size || sha(&object.bytes) != chunk.sha256 { return Err("Native object chunk integrity mismatch.".into()); }
            hash.update(&object.bytes);
            staging.write_all(&object.bytes).map_err(|_|"Native object staging write failed.")?;
        }
        if format!("{:x}",hash.finalize()) != head.manifest.sha256 { return Err("Native object full hash mismatch; discard staging bytes.".into()); }
        staging.flush().map_err(|_|"Native object staging flush failed.")?;
        self.client.ensure_current_account().map_err(|e|e.to_string())?;
        Ok(head.manifest.size)
    }
    pub fn upload(&self, key: &str, source: &mut impl Read, precondition: NativeWritePrecondition) -> Result<NativeWriteOutcome, String> {
        let root = root_key(key)?;
        let mut manifest = Manifest { protocol: PROTOCOL.into(), key: key.into(), size: 0, sha256: String::new(), chunks: vec![] };
        let mut hash = Sha256::new();
        let mut buffer = vec![0; NATIVE_CHUNK_BYTES];
        loop {
            let mut length = 0;
            while length < buffer.len() {
                match source.read(&mut buffer[length..]) {
                    Ok(0) => break,
                    Ok(count) => length += count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err("Native object source read failed; no root published.".into()),
                }
            }
            if length == 0 { break; }
            manifest.size = manifest.size.checked_add(length as u64).ok_or("Native object size overflow.")?;
            if manifest.size > MAX_NATIVE_OBJECT_BYTES || manifest.chunks.len() >= MAX_CHUNKS { return Err("Native object exceeds 32 GiB limit; no root published.".into()); }
            let bytes = &buffer[..length]; let digest = sha(bytes); let path = chunk_key(&digest);
            hash.update(bytes);
            match self.client.get(&self.agent, &path).map_err(|e|e.to_string())? {
                Some(object) if object.bytes == bytes => {},
                Some(_) => return Err("Immutable native chunk content mismatch.".into()),
                None => match self.client.put(&self.agent, &path, bytes, NativeWritePrecondition::ExpectAbsent).map_err(|e|e.to_string())? {
                    NativeWriteOutcome::Written { .. } => {},
                    NativeWriteOutcome::Conflict { current_etag: Some(etag), .. } if etag == format!("sha256:{digest}") => {},
                    _ => return Err("Native chunk write conflicted; no root published.".into()),
                },
            }
            manifest.chunks.push(Chunk { sha256: digest, size: length });
        }
        manifest.sha256 = format!("{:x}",hash.finalize()); manifest.validate(key)?;
        let bytes = serde_json::to_vec(&manifest).map_err(|_|"Could not encode native object manifest.")?;
        if bytes.len() > 1024 * 1024 { return Err("Native object manifest exceeds limit.".into()); }
        self.client.put(&self.agent, &root, &bytes, precondition).map_err(|e|e.to_string())
    }
}
