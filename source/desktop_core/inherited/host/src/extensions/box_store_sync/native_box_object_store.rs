use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::extensions::auth::auth_service::FabushiCoreCredentialProvider;
use super::box_object_store::{BoxObjectStore, BoxObjectStoreProvider};
use super::native_account_store::{NativeAccountStoreClient, NativeDeleteOutcome, NativeWriteOutcome, NativeWritePrecondition};
use super::native_object_transfer::{MAX_NATIVE_OBJECT_BYTES, NativeObjectHead, NativeObjectTransfer};
use super::object_store_port::{BoxStoreCanonicalWriteConflictError, BoxStoreConditionalWriteOutcome};

const MAX_MEMORY_GET: u64 = 64 * 1024 * 1024;
pub struct NativeBoxObjectStoreProvider { client: Arc<NativeAccountStoreClient> }
impl NativeBoxObjectStoreProvider {
    pub fn new(base: &str, credentials: Arc<dyn FabushiCoreCredentialProvider>) -> Result<Self, String> {
        Ok(Self { client: Arc::new(NativeAccountStoreClient::for_current_account(base, credentials).map_err(|e|e.to_string())?) })
    }
}
impl BoxObjectStoreProvider for NativeBoxObjectStoreProvider {
    fn for_store(&self, store_id: &str) -> Box<dyn BoxObjectStore> {
        Box::new(NativeBoxObjectStore { transfer: NativeObjectTransfer::new(self.client.clone(), store_id.into()), baselines: Mutex::new(HashMap::new()) })
    }
}
pub struct NativeBoxObjectStore {
    transfer: NativeObjectTransfer,
    baselines: Mutex<HashMap<String, Option<NativeObjectHead>>>,
}
fn precondition(head: Option<&NativeObjectHead>) -> NativeWritePrecondition {
    head.map(|h| NativeWritePrecondition::BaseEtag(h.etag.clone())).unwrap_or(NativeWritePrecondition::ExpectAbsent)
}
fn conflict(key: &str, path: Option<String>, etag: Option<String>) -> BoxStoreConditionalWriteOutcome {
    BoxStoreConditionalWriteOutcome::Conflict(BoxStoreCanonicalWriteConflictError::new(key,path,etag,Some("native-root-cas".into())))
}
impl NativeBoxObjectStore {
    fn remember(&self, key: &str, head: Option<NativeObjectHead>) -> Result<(), String> {
        self.baselines.lock().map_err(|_|"Native file baseline lock failed.")?.insert(key.into(),head); Ok(())
    }
    fn baseline(&self, key: &str) -> Result<Option<NativeObjectHead>, String> {
        self.transfer.ensure_current_account()?;
        if let Some(head) = self.baselines.lock().map_err(|_|"Native file baseline lock failed.")?.get(key).cloned() { return Ok(head); }
        let head = self.transfer.head(key)?; self.remember(key,head.clone())?; Ok(head)
    }
    fn finish_write(&self, key: &str, result: NativeWriteOutcome) -> Result<BoxStoreConditionalWriteOutcome, String> {
        match result {
            NativeWriteOutcome::Written { .. } => {
                // A subsequent operation must acquire a fresh native version.
                self.baselines.lock().map_err(|_|"Native file baseline lock failed.")?.remove(key);
                Ok(BoxStoreConditionalWriteOutcome::Written)
            }
            NativeWriteOutcome::Conflict { conflict_path, current_etag } => Ok(conflict(key,Some(conflict_path),current_etag)),
        }
    }
}
fn source_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK); }
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.custom_flags(0x00200000); }
    let file = options.open(path).map_err(|_|"Could not open native upload source.")?;
    let metadata = file.metadata().map_err(|_|"Could not inspect native upload source.")?;
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return Err("Native upload source reparse point refused.".into()); }
    }
    if !metadata.is_file() || metadata.len() > MAX_NATIVE_OBJECT_BYTES { return Err("Invalid native upload source type/size.".into()); }
    Ok(file)
}
impl BoxObjectStore for NativeBoxObjectStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let head = self.transfer.head(key)?; self.remember(key,head.clone())?;
        let Some(head) = head else { return Ok(None); };
        if head.size() > MAX_MEMORY_GET { return Err("Use streaming native download for objects over 64 MiB.".into()); }
        let mut bytes = Vec::with_capacity(head.size() as usize); self.transfer.download(&head,&mut bytes,MAX_MEMORY_GET)?; Ok(Some(bytes))
    }
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let baseline = self.baseline(key)?;
        let result = self.transfer.upload(key,&mut Cursor::new(bytes),precondition(baseline.as_ref()))?;
        match self.finish_write(key,result)? { BoxStoreConditionalWriteOutcome::Written=>Ok(()),BoxStoreConditionalWriteOutcome::Conflict(error)=>Err(error.to_string()) }
    }
    fn put_if_unchanged(&self, key: &str, expected: Option<&[u8]>, bytes: &[u8]) -> Result<bool, String> {
        Ok(matches!(self.put_if_unchanged_detailed(key,expected,bytes)?,BoxStoreConditionalWriteOutcome::Written))
    }
    fn put_if_unchanged_detailed(&self, key: &str, expected: Option<&[u8]>, bytes: &[u8]) -> Result<BoxStoreConditionalWriteOutcome, String> {
        let baseline = self.baseline(key)?;
        let matches = match (expected,baseline.as_ref()) {
            (None,None)=>true,
            (Some(bytes),Some(head))=>head.size()==bytes.len() as u64 && head.sha256()==format!("{:x}",Sha256::digest(bytes)),
            _=>false,
        };
        if !matches { return Ok(conflict(key,None,baseline.map(|h|h.etag))); }
        let result = self.transfer.upload(key,&mut Cursor::new(bytes),precondition(baseline.as_ref()))?;self.finish_write(key,result)
    }
    fn get_to_file(&self, key: &str, destination: &Path, max_bytes: Option<u64>) -> Result<Option<u64>, String> {
        let head=self.transfer.head(key)?;self.remember(key,head.clone())?;let Some(head)=head else{return Ok(None);};
        let limit=max_bytes.unwrap_or(MAX_NATIVE_OBJECT_BYTES);
        if head.size()>limit{return Err("Native object exceeds requested download limit.".into());}
        let mut options=OpenOptions::new();options.write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt;options.mode(0o600); }
        let mut staging=options.open(destination).map_err(|_|"Native download requires a new staging file; existing files/symlinks are refused.")?;
        let size=self.transfer.download(&head,&mut staging,limit)?;
        staging.sync_all().map_err(|_|"Native download staging sync failed.")?;Ok(Some(size))
    }
    fn put_from_file(&self, key: &str, source: &Path) -> Result<(), String> {
        let baseline=self.transfer.head(key)?;
        if let Some(head)=baseline {
            let mut file=source_file(source)?;let mut hash=Sha256::new();let mut buffer=[0;65536];let mut size=0u64;
            loop {let n=file.read(&mut buffer).map_err(|_|"Native source read failed.")?;if n==0{break;}size+=n as u64;if size>MAX_NATIVE_OBJECT_BYTES{return Err("Native source exceeds limit.".into());}hash.update(&buffer[..n]);}
            if size==head.size() && format!("{:x}",hash.finalize())==head.sha256(){self.transfer.ensure_current_account()?;self.remember(key,Some(head))?;return Ok(());}
            return Err("Content-addressed native file already has different content.".into());
        }
        let result=self.transfer.upload(key,&mut source_file(source)?,NativeWritePrecondition::ExpectAbsent)?;
        match self.finish_write(key,result)? {BoxStoreConditionalWriteOutcome::Written=>Ok(()),BoxStoreConditionalWriteOutcome::Conflict(error)=>Err(error.to_string())}
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>, String> { self.transfer.list(prefix) }
    fn delete(&self, key: &str) -> Result<(), String> {
        let Some(head)=self.baseline(key)? else {return Ok(());};
        match self.transfer.delete(key,&head.etag)? {
            NativeDeleteOutcome::Deleted|NativeDeleteOutcome::Absent=>{self.remember(key,None)?;Ok(())},
            NativeDeleteOutcome::Conflict{..}=>Err("Native object changed before deletion; root retained.".into()),
        }
    }
}
