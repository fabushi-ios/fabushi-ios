use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::{Body, Client, Response};
use reqwest::header::{CONTENT_LENGTH, ETAG, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use url::Url;

use super::agent_store_sand_files::normalize_rel_path;
use super::box_object_store::{BoxObjectStore, BoxObjectStoreProvider};
use super::box_store_manifest_format::BOX_STORE_BLOBS_PREFIX;
use super::request_coalescer::{RequestCoalescer, RequestCoalescerError};

pub const SAND_BOX_STORE_MULTIPART_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;
pub const SAND_BOX_STORE_MULTIPART_PART_SIZE_BYTES: u64 = 128 * 1024 * 1024;
pub const S3_MAX_SINGLE_PUT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
pub const MULTIPART_COMPLETE_MAX_ATTEMPTS: usize = 3;
pub const READ_PRESIGN_BATCH_MAX: usize = 500;
pub const WRITE_PRESIGN_BATCH_MAX: usize = 64;
pub const SAND_BOX_STORE_RPC_TIMEOUT_MS: u64 = 60_000;
const PREFETCHED_READ_FRESHNESS_MARGIN_MS: u64 = 30_000;
const MULTIPART_FAILURE_PRECONDITION_FAILED: i32 = 1;
const MULTIPART_FAILURE_TRANSIENT: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPart {
    pub part_number: usize,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPlan {
    pub parts: Vec<MultipartPart>,
    pub whole_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandBoxStoreClientErrorCode {
    InvalidArgument,
    Transport,
    Protocol,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreClientError {
    pub code: SandBoxStoreClientErrorCode,
    pub message: String,
}

impl SandBoxStoreClientError {
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self { code: SandBoxStoreClientErrorCode::InvalidArgument, message: message.into() }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self { code: SandBoxStoreClientErrorCode::Protocol, message: message.into() }
    }
}

impl std::fmt::Display for SandBoxStoreClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SandBoxStoreClientError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreReadInstruction {
    pub rel_path: String,
    pub url: String,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreObjectStat {
    pub exists: bool,
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreMultipartPartRequest {
    pub part_number: usize,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreWriteFile {
    pub rel_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub content_addressed: bool,
    pub if_match_etag: String,
    pub expect_absent: bool,
    pub multipart_parts: Vec<SandBoxStoreMultipartPartRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreMultipartPresignedPart {
    pub part_number: usize,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    pub url: String,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreMultipartInstruction {
    pub context: Option<Vec<u8>>,
    pub parts: Vec<SandBoxStoreMultipartPresignedPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreWriteInstruction {
    pub rel_path: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub multipart: Option<SandBoxStoreMultipartInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreMultipartCompletion {
    pub context: Vec<u8>,
    pub parts: Vec<SandBoxStoreCompletedPart>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreCompletedPart {
    pub part_number: usize,
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandBoxStoreMultipartCompletionOutcome {
    Success,
    Failure { code: i32 },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreListPage {
    pub entries: Vec<String>,
    pub truncated: bool,
    pub next_cursor: String,
}

pub trait SandBoxStoreServiceClient: Send + Sync {
    fn presign_reads(&self, rel_paths: &[String]) -> Result<Vec<SandBoxStoreReadInstruction>, SandBoxStoreClientError>;
    fn stat_object(&self, rel_path: &str) -> Result<SandBoxStoreObjectStat, SandBoxStoreClientError>;
    fn presign_writes(&self, files: &[SandBoxStoreWriteFile]) -> Result<Vec<SandBoxStoreWriteInstruction>, SandBoxStoreClientError>;
    fn complete_multipart_writes(&self, completions: &[SandBoxStoreMultipartCompletion]) -> Result<Vec<SandBoxStoreMultipartCompletionOutcome>, SandBoxStoreClientError>;
    fn abort_multipart_writes(&self, contexts: &[Vec<u8>]) -> Result<(), SandBoxStoreClientError>;
    fn list_objects(&self, prefix: &str, cursor: &str, max_entries: usize) -> Result<SandBoxStoreListPage, SandBoxStoreClientError>;
}

pub fn plan_sand_box_store_multipart_parts(src_path: &Path, part_size_bytes: u64) -> Result<MultipartPlan, String> {
    let size_bytes = fs::metadata(src_path).map_err(|error| error.to_string())?.len();
    plan_sand_box_store_multipart_parts_for_size(src_path, size_bytes, part_size_bytes)
}

pub fn plan_sand_box_store_multipart_parts_for_size(src_path: &Path, expected_size_bytes: u64, part_size_bytes: u64) -> Result<MultipartPlan, String> {
    if expected_size_bytes == 0 { return Err("multipart plan requires a positive size".into()); }
    if part_size_bytes == 0 { return Err("multipart plan requires a positive part size".into()); }
    let mut file = File::open(src_path).map_err(|error| error.to_string())?;
    let mut whole = Sha256::new();
    let mut parts = Vec::new();
    let mut position = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    while position < expected_size_bytes {
        let part_number = parts.len() + 1;
        let offset_bytes = position;
        let target = part_size_bytes.min(expected_size_bytes - position);
        let mut remaining = target;
        let mut part_hash = Sha256::new();
        while remaining > 0 {
            let take = usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| "multipart read size overflow".to_string())?;
            let read = file.read(&mut buffer[..take]).map_err(|error| error.to_string())?;
            if read == 0 {
                return Err(format!("multipart plan read {position}B but expected {expected_size_bytes}B: the file changed under the plan"));
            }
            part_hash.update(&buffer[..read]);
            whole.update(&buffer[..read]);
            position = position.saturating_add(read as u64);
            remaining = remaining.saturating_sub(read as u64);
        }
        parts.push(MultipartPart { part_number, offset_bytes, size_bytes: target, sha256: hex_digest(part_hash.finalize()) });
    }
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(|error| error.to_string())? != 0 {
        return Err(format!("multipart plan read more than expected {expected_size_bytes}B: the file changed under the plan"));
    }
    Ok(MultipartPlan { parts, whole_sha256: hex_digest(whole.finalize()) })
}

pub fn plan_sand_box_store_multipart_parts_from_bytes(bytes: &[u8], part_size_bytes: u64) -> Result<MultipartPlan, String> {
    if bytes.is_empty() { return Err("multipart plan requires a positive size".into()); }
    let part_size = usize::try_from(part_size_bytes).ok().filter(|value| *value > 0).ok_or_else(|| "multipart plan requires a positive part size".to_string())?;
    let parts = bytes.chunks(part_size).enumerate().map(|(index, chunk)| MultipartPart {
        part_number: index + 1,
        offset_bytes: (index * part_size) as u64,
        size_bytes: chunk.len() as u64,
        sha256: sha256_hex(chunk),
    }).collect();
    Ok(MultipartPlan { parts, whole_sha256: sha256_hex(bytes) })
}

pub fn should_use_multipart(size_bytes: u64, threshold_bytes: u64) -> bool {
    size_bytes >= threshold_bytes || size_bytes > S3_MAX_SINGLE_PUT_BYTES
}

pub fn sha256_hex(bytes: &[u8]) -> String { hex_digest(Sha256::digest(bytes)) }

fn sha256_file(src_path: &Path) -> Result<String, String> {
    let mut file = File::open(src_path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 { break; }
        hash.update(&buffer[..read]);
    }
    Ok(hex_digest(hash.finalize()))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[derive(Clone)]
pub struct SandBoxStoreServiceProvider {
    store: SandBoxStoreServiceObjectStore,
}

impl SandBoxStoreServiceProvider {
    pub fn new(client: Arc<dyn SandBoxStoreServiceClient>) -> Result<Self, String> {
        Self::with_limits(client, SAND_BOX_STORE_MULTIPART_THRESHOLD_BYTES, SAND_BOX_STORE_MULTIPART_PART_SIZE_BYTES)
    }

    pub fn with_limits(client: Arc<dyn SandBoxStoreServiceClient>, multipart_threshold_bytes: u64, multipart_part_size_bytes: u64) -> Result<Self, String> {
        Ok(Self { store: SandBoxStoreServiceObjectStore::new(client, multipart_threshold_bytes, multipart_part_size_bytes)? })
    }
}

impl BoxObjectStoreProvider for SandBoxStoreServiceProvider {
    fn for_store(&self, _store_id: &str) -> Box<dyn BoxObjectStore> { Box::new(self.store.clone()) }
}

#[derive(Debug, Clone)]
enum SandBoxStoreBaseline {
    Absent,
    Present { etag: String, digest: [u8; 32] },
}

#[derive(Debug, Clone)]
enum MutableWritePrecondition {
    Absent,
    Etag(String),
}

#[derive(Clone)]
pub struct SandBoxStoreServiceObjectStore {
    client: Arc<dyn SandBoxStoreServiceClient>,
    http: Client,
    multipart_threshold_bytes: u64,
    multipart_part_size_bytes: u64,
    session_baselines: Arc<Mutex<HashMap<String, SandBoxStoreBaseline>>>,
    prefetched_reads: Arc<Mutex<HashMap<String, SandBoxStoreReadInstruction>>>,
    write_presign_coalescer: Arc<RequestCoalescer<SandBoxStoreWriteFile, SandBoxStoreWriteInstruction, SandBoxStoreClientError>>,
}

impl SandBoxStoreServiceObjectStore {
    pub fn new(client: Arc<dyn SandBoxStoreServiceClient>, multipart_threshold_bytes: u64, multipart_part_size_bytes: u64) -> Result<Self, String> {
        let http = Client::builder().timeout(Duration::from_millis(SAND_BOX_STORE_RPC_TIMEOUT_MS)).redirect(Policy::none()).build().map_err(|error| error.to_string())?;
        let coalescer_client = Arc::clone(&client);
        let run = Arc::new(move |files: Vec<SandBoxStoreWriteFile>| {
            let instructions = coalescer_client.presign_writes(&files)?;
            if instructions.len() != files.len() {
                return Err(SandBoxStoreClientError::protocol(format!("sand-box-store presign returned {} instructions for {} files", instructions.len(), files.len())));
            }
            for (file, instruction) in files.iter().zip(&instructions) {
                if instruction.rel_path != file.rel_path {
                    return Err(SandBoxStoreClientError::protocol(format!("sand-box-store presign returned {} for {}", instruction.rel_path, file.rel_path)));
                }
            }
            Ok(instructions)
        });
        let write_presign_coalescer = RequestCoalescer::new(
            WRITE_PRESIGN_BATCH_MAX,
            run,
            Some(Arc::new(|file: &SandBoxStoreWriteFile| Some(file.rel_path.clone()))),
            Some(Arc::new(|error| matches!(error, RequestCoalescerError::Run(value) if value.code == SandBoxStoreClientErrorCode::InvalidArgument))),
        );
        Ok(Self {
            client,
            http,
            multipart_threshold_bytes: multipart_threshold_bytes.max(1),
            multipart_part_size_bytes: multipart_part_size_bytes.max(1),
            session_baselines: Arc::new(Mutex::new(HashMap::new())),
            prefetched_reads: Arc::new(Mutex::new(HashMap::new())),
            write_presign_coalescer,
        })
    }

    pub fn prefetch_reads(&self, keys: &[String]) -> Result<(), String> {
        for chunk in keys.chunks(READ_PRESIGN_BATCH_MAX) {
            let canonical = chunk.iter().map(|key| normalize_rel_path(key)).collect::<Result<Vec<_>, _>>()?;
            let instructions = self.client.presign_reads(&canonical).map_err(|error| error.to_string())?;
            let mut cache = self.prefetched_reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for instruction in instructions {
                let rel_path = normalize_rel_path(&instruction.rel_path)?;
                validate_presigned_url(&instruction.url)?;
                cache.insert(rel_path, instruction);
            }
        }
        Ok(())
    }

    fn take_prefetched_read(&self, key: &str) -> Option<SandBoxStoreReadInstruction> {
        let mut cache = self.prefetched_reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cached = cache.remove(key)?;
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().try_into().unwrap_or(u64::MAX);
        (cached.expires_at_ms.saturating_sub(now_ms) > PREFETCHED_READ_FRESHNESS_MARGIN_MS).then_some(cached)
    }

    fn presign_read(&self, key: &str) -> Result<Option<SandBoxStoreReadInstruction>, String> {
        let instructions = self.client.presign_reads(&[key.to_string()]).map_err(|error| error.to_string())?;
        let Some(instruction) = instructions.into_iter().next() else { return Ok(None); };
        if normalize_rel_path(&instruction.rel_path)? != key {
            return Err(format!("sand-box-store read presign returned {} for {key}", instruction.rel_path));
        }
        validate_presigned_url(&instruction.url)?;
        Ok(Some(instruction))
    }

    fn read_object(&self, key: &str, instruction: &SandBoxStoreReadInstruction) -> Result<Option<(Vec<u8>, Option<String>)>, String> {
        let response = self.http.get(&instruction.url).send().map_err(|error| error.to_string())?;
        match response.status().as_u16() {
            404 => Ok(None),
            status if (200..300).contains(&status) => {
                let etag = response.headers().get(ETAG).and_then(|value| value.to_str().ok()).map(normalize_etag).filter(|value| !value.is_empty());
                let bytes = response.bytes().map_err(|error| error.to_string())?.to_vec();
                Ok(Some((bytes, etag)))
            }
            status => Err(format!("sand-box-store read failed for {key}: {status}")),
        }
    }

    fn remember_read(&self, key: &str, object: Option<(&[u8], Option<&str>)>) {
        let mut baselines = self.session_baselines.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match object {
            None => { baselines.insert(key.to_string(), SandBoxStoreBaseline::Absent); }
            Some((bytes, Some(etag))) if !etag.is_empty() => {
                baselines.insert(key.to_string(), SandBoxStoreBaseline::Present { etag: etag.to_string(), digest: Sha256::digest(bytes).into() });
            }
            Some(_) => { baselines.remove(key); }
        }
    }

    fn baseline_for_expected(&self, key: &str, expected: Option<&[u8]>) -> Option<MutableWritePrecondition> {
        let baselines = self.session_baselines.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match (baselines.get(key), expected) {
            (Some(SandBoxStoreBaseline::Absent), None) => Some(MutableWritePrecondition::Absent),
            (Some(SandBoxStoreBaseline::Present { etag, digest }), Some(expected)) if *digest == <[u8; 32]>::from(Sha256::digest(expected)) => Some(MutableWritePrecondition::Etag(etag.clone())),
            _ => None,
        }
    }

    fn known_or_probed_precondition(&self, key: &str) -> Result<MutableWritePrecondition, String> {
        if let Some(value) = {
            let baselines = self.session_baselines.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match baselines.get(key) {
                Some(SandBoxStoreBaseline::Absent) => Some(MutableWritePrecondition::Absent),
                Some(SandBoxStoreBaseline::Present { etag, .. }) => Some(MutableWritePrecondition::Etag(etag.clone())),
                None => None,
            }
        } { return Ok(value); }
        let stat = self.client.stat_object(key).map_err(|error| error.to_string())?;
        if !stat.exists { return Ok(MutableWritePrecondition::Absent); }
        let etag = normalize_etag(&stat.etag);
        if etag.is_empty() { return Err(format!("sand-box-store write for {key} has no usable baseline: the object exists but stat carried no etag")); }
        Ok(MutableWritePrecondition::Etag(etag))
    }

    fn presign_write(&self, file: SandBoxStoreWriteFile) -> Result<SandBoxStoreWriteInstruction, String> {
        self.write_presign_coalescer.request(file).map_err(|error| error.to_string())
    }

    fn put_content_addressed_bytes(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let instruction = self.presign_write(SandBoxStoreWriteFile {
            rel_path: key.to_string(),
            sha256: sha256_hex(bytes),
            size_bytes: bytes.len() as u64,
            content_addressed: true,
            if_match_etag: String::new(),
            expect_absent: false,
            multipart_parts: Vec::new(),
        })?;
        let mut response = self.put_bytes_once(&instruction, bytes)?;
        if response.status().as_u16() == 409 { response = self.put_bytes_once(&instruction, bytes)?; }
        match response.status().as_u16() {
            412 => Ok(()),
            status if (200..300).contains(&status) => Ok(()),
            status => Err(format!("sand-box-store write failed for {key}: {status}")),
        }
    }

    fn put_mutable_bytes(&self, key: &str, bytes: &[u8], precondition: MutableWritePrecondition) -> Result<bool, String> {
        let (if_match_etag, expect_absent) = match &precondition {
            MutableWritePrecondition::Absent => (String::new(), true),
            MutableWritePrecondition::Etag(value) => (value.clone(), false),
        };
        let instruction = self.presign_write(SandBoxStoreWriteFile {
            rel_path: key.to_string(),
            sha256: sha256_hex(bytes),
            size_bytes: bytes.len() as u64,
            content_addressed: false,
            if_match_etag,
            expect_absent,
            multipart_parts: Vec::new(),
        })?;
        let mut response = self.put_bytes_once(&instruction, bytes)?;
        if response.status().as_u16() == 409 { response = self.put_bytes_once(&instruction, bytes)?; }
        match response.status().as_u16() {
            409 | 412 => {
                self.session_baselines.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(key);
                Ok(false)
            }
            status if (200..300).contains(&status) => {
                let etag = response.headers().get(ETAG).and_then(|value| value.to_str().ok()).map(normalize_etag).filter(|value| !value.is_empty());
                let mut baselines = self.session_baselines.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(etag) = etag {
                    baselines.insert(key.to_string(), SandBoxStoreBaseline::Present { etag, digest: Sha256::digest(bytes).into() });
                } else {
                    baselines.remove(key);
                }
                Ok(true)
            }
            status => Err(format!("sand-box-store write failed for {key}: {status}")),
        }
    }

    fn put_bytes_once(&self, instruction: &SandBoxStoreWriteInstruction, bytes: &[u8]) -> Result<Response, String> {
        validate_presigned_url(&instruction.url)?;
        apply_headers(self.http.put(&instruction.url), &instruction.headers)?.body(bytes.to_vec()).send().map_err(|error| error.to_string())
    }

    fn put_file_once(&self, url: &str, headers: &HashMap<String, String>, src_path: &Path, size: u64) -> Result<Response, String> {
        validate_presigned_url(url)?;
        let file = File::open(src_path).map_err(|error| error.to_string())?;
        apply_headers(self.http.put(url), headers)?.header(CONTENT_LENGTH, size).body(Body::sized(file, size)).send().map_err(|error| error.to_string())
    }

    fn put_file_range_once(&self, part: &SandBoxStoreMultipartPresignedPart, src_path: &Path) -> Result<Response, String> {
        validate_presigned_url(&part.url)?;
        let mut file = File::open(src_path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(part.offset_bytes)).map_err(|error| error.to_string())?;
        let body = file.take(part.size_bytes);
        apply_headers(self.http.put(&part.url), &part.headers)?.header(CONTENT_LENGTH, part.size_bytes).body(Body::sized(body, part.size_bytes)).send().map_err(|error| error.to_string())
    }

    fn upload_multipart(&self, key: &str, src_path: &Path, multipart: &SandBoxStoreMultipartInstruction) -> Result<(), String> {
        let context = multipart.context.clone().ok_or_else(|| format!("sand-box-store multipart instruction for {key} carries no upload context"))?;
        let result = (|| {
            let mut parts = multipart.parts.clone();
            parts.sort_by_key(|part| part.part_number);
            let mut completed = Vec::with_capacity(parts.len());
            for part in &parts {
                let mut response = self.put_file_range_once(part, src_path)?;
                if !response.status().is_success() { response = self.put_file_range_once(part, src_path)?; }
                if !response.status().is_success() {
                    return Err(format!("sand-box-store multipart part {} failed for {key}: {}", part.part_number, response.status().as_u16()));
                }
                let etag = response.headers().get(ETAG).and_then(|value| value.to_str().ok()).map(normalize_etag).filter(|value| !value.is_empty()).ok_or_else(|| format!("sand-box-store multipart part {} for {key} returned no etag", part.part_number))?;
                completed.push(SandBoxStoreCompletedPart { part_number: part.part_number, etag });
            }
            let completion = SandBoxStoreMultipartCompletion { context: context.clone(), parts: completed };
            let mut last_failure = "no result".to_string();
            for _ in 0..MULTIPART_COMPLETE_MAX_ATTEMPTS {
                match self.client.complete_multipart_writes(&[completion.clone()]) {
                    Ok(outcomes) => match outcomes.first() {
                        Some(SandBoxStoreMultipartCompletionOutcome::Success) => return Ok(()),
                        Some(SandBoxStoreMultipartCompletionOutcome::Failure { code: MULTIPART_FAILURE_PRECONDITION_FAILED }) => return Ok(()),
                        Some(SandBoxStoreMultipartCompletionOutcome::Failure { code }) => {
                            last_failure = format!("failure code {code}");
                            if *code != MULTIPART_FAILURE_TRANSIENT { break; }
                        }
                        Some(SandBoxStoreMultipartCompletionOutcome::Missing) | None => { last_failure = "no result".into(); break; }
                    },
                    Err(error) => { last_failure = error.to_string(); }
                }
            }
            Err(format!("sand-box-store multipart complete failed for {key}: {last_failure}"))
        })();
        if result.is_err() { let _ = self.client.abort_multipart_writes(&[context]); }
        result
    }
}

impl BoxObjectStore for SandBoxStoreServiceObjectStore {
    fn prefetch_reads(&self, keys: &[String]) -> Result<(), String> {
        SandBoxStoreServiceObjectStore::prefetch_reads(self, keys)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let key = normalize_rel_path(key)?;
        let instruction = match self.take_prefetched_read(&key) { Some(value) => Some(value), None => self.presign_read(&key)? };
        let Some(instruction) = instruction else { self.remember_read(&key, None); return Ok(None); };
        let object = self.read_object(&key, &instruction)?;
        match object {
            None => { self.remember_read(&key, None); Ok(None) }
            Some((bytes, etag)) => { self.remember_read(&key, Some((&bytes, etag.as_deref()))); Ok(Some(bytes)) }
        }
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        let key = normalize_rel_path(key)?;
        if key == BOX_STORE_BLOBS_PREFIX || key.starts_with(&format!("{BOX_STORE_BLOBS_PREFIX}/")) {
            return self.put_content_addressed_bytes(&key, bytes);
        }
        let precondition = self.known_or_probed_precondition(&key)?;
        if self.put_mutable_bytes(&key, bytes, precondition)? { Ok(()) } else { Err(format!("sand-box-store write for {key} lost a concurrent-write race")) }
    }

    fn put_if_unchanged(&self, key: &str, expected: Option<&[u8]>, bytes: &[u8]) -> Result<bool, String> {
        let key = normalize_rel_path(key)?;
        let Some(precondition) = self.baseline_for_expected(&key, expected) else { return Ok(false); };
        self.put_mutable_bytes(&key, bytes, precondition)
    }

    fn get_to_file(&self, key: &str, dest_path: &Path, max_bytes: Option<u64>) -> Result<Option<u64>, String> {
        let key = normalize_rel_path(key)?;
        let instruction = match self.take_prefetched_read(&key) { Some(value) => Some(value), None => self.presign_read(&key)? };
        let Some(instruction) = instruction else { return Ok(None); };
        let mut response = self.http.get(&instruction.url).send().map_err(|error| error.to_string())?;
        if response.status().as_u16() == 404 { return Ok(None); }
        if !response.status().is_success() { return Err(format!("sand-box-store read failed for {key}: {}", response.status().as_u16())); }
        if let (Some(max), Some(length)) = (max_bytes, response.content_length()) {
            if length > max { return Err(format!("object {length} exceeds maximum restore bytes")); }
        }
        if let Some(parent) = dest_path.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
        let mut file = File::create(dest_path).map_err(|error| error.to_string())?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let read = response.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 { break; }
            copied = copied.saturating_add(read as u64);
            if max_bytes.is_some_and(|max| copied > max) {
                let _ = fs::remove_file(dest_path);
                return Err(format!("object {copied} exceeds maximum restore bytes"));
            }
            file.write_all(&buffer[..read]).map_err(|error| error.to_string())?;
        }
        file.flush().map_err(|error| error.to_string())?;
        Ok(Some(copied))
    }

    fn put_from_file(&self, key: &str, src_path: &Path) -> Result<(), String> {
        let key = normalize_rel_path(key)?;
        let size = fs::metadata(src_path).map_err(|error| error.to_string())?.len();
        let sha = sha256_file(src_path)?;
        let multipart_parts = if should_use_multipart(size, self.multipart_threshold_bytes) {
            let plan = plan_sand_box_store_multipart_parts_for_size(src_path, size, self.multipart_part_size_bytes)?;
            if plan.whole_sha256 != sha { return Err(format!("sand-box-store multipart plan for {key} hashed {} but the caller expected {sha}", plan.whole_sha256)); }
            plan.parts.into_iter().map(|part| SandBoxStoreMultipartPartRequest { part_number: part.part_number, size_bytes: part.size_bytes, sha256: part.sha256 }).collect()
        } else { Vec::new() };
        let instruction = self.presign_write(SandBoxStoreWriteFile {
            rel_path: key.clone(),
            sha256: sha,
            size_bytes: size,
            content_addressed: true,
            if_match_etag: String::new(),
            expect_absent: false,
            multipart_parts,
        })?;
        if let Some(multipart) = &instruction.multipart { return self.upload_multipart(&key, src_path, multipart); }
        if size > S3_MAX_SINGLE_PUT_BYTES { return Err(format!("sand-box-store write for {key} is {size}B, over the S3 single-PUT maximum, and the backend returned no multipart instruction")); }
        let mut response = self.put_file_once(&instruction.url, &instruction.headers, src_path, size)?;
        if response.status().as_u16() == 409 { response = self.put_file_once(&instruction.url, &instruction.headers, src_path, size)?; }
        match response.status().as_u16() {
            412 => Ok(()),
            status if (200..300).contains(&status) => Ok(()),
            status => Err(format!("sand-box-store write failed for {key}: {status}")),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, String> {
        let prefix = if prefix.trim().is_empty() { String::new() } else { normalize_rel_path(prefix)? };
        let mut keys = Vec::new();
        let mut cursor = String::new();
        loop {
            let page = self.client.list_objects(&prefix, &cursor, 0).map_err(|error| error.to_string())?;
            for entry in page.entries { keys.push(normalize_rel_path(&entry)?); }
            if !page.truncated || page.next_cursor.is_empty() { keys.sort(); keys.dedup(); return Ok(keys); }
            if page.next_cursor == cursor { return Err("sand-box-store list cursor did not advance".into()); }
            cursor = page.next_cursor;
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        Err(format!("SandBoxStoreServiceObjectStore is append-only; cannot delete {key}"))
    }
}

fn apply_headers(mut request: reqwest::blocking::RequestBuilder, headers: &HashMap<String, String>) -> Result<reqwest::blocking::RequestBuilder, String> {
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| error.to_string())?;
        let value = HeaderValue::from_str(value).map_err(|error| error.to_string())?;
        request = request.header(name, value);
    }
    Ok(request)
}

fn normalize_etag(value: &str) -> String { value.trim().trim_matches('"').trim().to_string() }

fn validate_presigned_url(value: &str) -> Result<(), String> {
    let url = Url::parse(value).map_err(|error| error.to_string())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("sand-box-store presigned URL contains forbidden authority data".into());
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if url.host_str().is_some_and(is_loopback_host) => Ok(()),
        scheme => Err(format!("sand-box-store presigned URL uses unsupported scheme {scheme}")),
    }
}

fn is_loopback_host(host: &str) -> bool { matches!(host, "127.0.0.1" | "::1" | "localhost") }

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, atomic::{AtomicUsize, Ordering}};
    use std::thread;
    use std::time::Duration;
    use super::*;

    #[derive(Default)]
    struct FakeClient {
        calls: Mutex<Vec<Vec<String>>>,
        first_entered: (Mutex<bool>, Condvar),
        release_first: (Mutex<bool>, Condvar),
        call_count: AtomicUsize,
    }

    impl FakeClient {
        fn wait_first_entered(&self) {
            let (lock, wake) = &self.first_entered;
            let mut entered = lock.lock().expect("first-entered lock");
            while !*entered { entered = wake.wait(entered).expect("first-entered wait"); }
        }
        fn release_first(&self) {
            let (lock, wake) = &self.release_first;
            *lock.lock().expect("release lock") = true;
            wake.notify_all();
        }
    }

    impl SandBoxStoreServiceClient for FakeClient {
        fn presign_reads(&self, _rel_paths: &[String]) -> Result<Vec<SandBoxStoreReadInstruction>, SandBoxStoreClientError> { Ok(Vec::new()) }
        fn stat_object(&self, _rel_path: &str) -> Result<SandBoxStoreObjectStat, SandBoxStoreClientError> { Ok(SandBoxStoreObjectStat { exists: false, etag: String::new() }) }
        fn presign_writes(&self, files: &[SandBoxStoreWriteFile]) -> Result<Vec<SandBoxStoreWriteInstruction>, SandBoxStoreClientError> {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.calls.lock().expect("calls lock").push(files.iter().map(|file| file.rel_path.clone()).collect());
            if call == 0 {
                let (entered_lock, entered_wake) = &self.first_entered;
                *entered_lock.lock().expect("entered lock") = true;
                entered_wake.notify_all();
                let (release_lock, release_wake) = &self.release_first;
                let mut released = release_lock.lock().expect("release lock");
                while !*released { released = release_wake.wait(released).expect("release wait"); }
            }
            if files.len() > 1 { return Err(SandBoxStoreClientError::invalid_argument("split batched presign")); }
            Ok(files.iter().map(|file| SandBoxStoreWriteInstruction {
                rel_path: file.rel_path.clone(),
                url: "http://127.0.0.1/unused".into(),
                headers: HashMap::new(),
                multipart: None,
            }).collect())
        }
        fn complete_multipart_writes(&self, _completions: &[SandBoxStoreMultipartCompletion]) -> Result<Vec<SandBoxStoreMultipartCompletionOutcome>, SandBoxStoreClientError> { Ok(vec![SandBoxStoreMultipartCompletionOutcome::Success]) }
        fn abort_multipart_writes(&self, _contexts: &[Vec<u8>]) -> Result<(), SandBoxStoreClientError> { Ok(()) }
        fn list_objects(&self, _prefix: &str, _cursor: &str, _max_entries: usize) -> Result<SandBoxStoreListPage, SandBoxStoreClientError> { Ok(SandBoxStoreListPage { entries: Vec::new(), truncated: false, next_cursor: String::new() }) }
    }

    fn write_request(rel_path: &str) -> SandBoxStoreWriteFile {
        SandBoxStoreWriteFile {
            rel_path: rel_path.into(),
            sha256: "00".repeat(32),
            size_bytes: 1,
            content_addressed: true,
            if_match_etag: String::new(),
            expect_absent: false,
            multipart_parts: Vec::new(),
        }
    }

    #[test]
    fn multipart_plan_rejects_a_file_that_changes_under_the_expected_size() {
        let path = std::env::temp_dir().join(format!("sand-box-store-plan-{}", uuid::Uuid::new_v4().simple()));
        fs::write(&path, b"abc").expect("seed file");
        let error = plan_sand_box_store_multipart_parts_for_size(&path, 4, 2).expect_err("short file must fail closed");
        assert!(error.contains("file changed under the plan"));
        fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn provider_presign_path_uses_coalescer_and_splits_invalid_argument_batches() {
        let fake = Arc::new(FakeClient::default());
        let store = Arc::new(SandBoxStoreServiceObjectStore::new(fake.clone(), 64, 128).expect("construct store"));
        let seed = { let store = Arc::clone(&store); thread::spawn(move || store.presign_write(write_request("seed"))) };
        fake.wait_first_entered();
        let left = { let store = Arc::clone(&store); thread::spawn(move || store.presign_write(write_request("left"))) };
        let right = { let store = Arc::clone(&store); thread::spawn(move || store.presign_write(write_request("right"))) };
        thread::sleep(Duration::from_millis(20));
        fake.release_first();
        assert_eq!(seed.join().expect("seed thread").expect("seed").rel_path, "seed");
        assert_eq!(left.join().expect("left thread").expect("left").rel_path, "left");
        assert_eq!(right.join().expect("right thread").expect("right").rel_path, "right");
        let calls = fake.calls.lock().expect("calls lock").clone();
        assert_eq!(calls.first().cloned(), Some(vec!["seed".into()]));
        assert!(calls.iter().any(|batch| batch.len() == 2), "queued writes must enter one coalesced batch before invalid-argument split: {calls:?}");
        assert!(calls.iter().filter(|batch| batch.len() == 1).count() >= 3, "invalid batch must be retried individually: {calls:?}");
    }
}
