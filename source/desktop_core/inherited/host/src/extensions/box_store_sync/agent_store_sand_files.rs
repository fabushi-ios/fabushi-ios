use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prost::Message;
use reqwest::blocking::{Body, Client, RequestBuilder, Response};
use reqwest::header::{CONTENT_LENGTH, ETAG, HeaderName, HeaderValue, RANGE};
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use url::Url;
use serde_json::{Map, Value};

use crate::cursor_backend::{
    CursorBackendError, resolve_sand_ghost_mode_header, send_cursor_unary_with_headers,
};
use super::box_store_diagnostics::report_box_store_diagnostic;

pub const PRESIGN_READ_BATCH_MAX: usize = 500;
pub const AGENT_STORE_RPC_TIMEOUT_MS: u64 = 60_000;
pub const AGENT_STORE_TOKEN_REFRESH_BUFFER_MS: u64 = 60_000;
const AGENT_STORE_TOKEN_HEADER: &str = "x-agent-store-token";
const LIST_PAGE_SIZE: i32 = 1_000;
const LIST_MAX_PAGES: usize = 10;

const MINT_AGENT_STORE_TOKEN_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/MintAgentStoreToken";
const LIST_AGENT_STORE_DIRECTORY_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/ListAgentStoreDirectory";
const PRESIGN_AGENT_STORE_READS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/PresignAgentStoreReads";
const PRESIGN_AGENT_STORE_WRITES_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/PresignAgentStoreWrites";

const BCS_AGENT_STORE_BUCKET_HOSTS: &[&str] = &[
    "agent-stores.s3.us-east-1.amazonaws.com",
    "agent-stores.s3.amazonaws.com",
];
const PLAYGROUND_AGENT_STORE_BUCKET_HOSTS: &[&str] = &[
    "agent-stores-928182716709-us-west-2-an.s3.us-west-2.amazonaws.com",
    "agent-stores-928182716709-us-west-2-an.s3.amazonaws.com",
];

#[derive(Debug, Default, Clone)]
pub struct AgentStoreMutableWriteEtags {
    etags: HashMap<(String, String), String>,
}

impl AgentStoreMutableWriteEtags {
    pub fn get(&self, source_id: &str, rel_path: &str) -> Option<&str> {
        self.etags
            .get(&(source_id.to_string(), rel_path.to_string()))
            .map(String::as_str)
    }

    pub fn set(&mut self, source_id: &str, rel_path: &str, etag: impl Into<String>) {
        self.etags.insert(
            (source_id.to_string(), rel_path.to_string()),
            etag.into(),
        );
    }

    pub fn delete(&mut self, source_id: &str, rel_path: &str) {
        self.etags
            .remove(&(source_id.to_string(), rel_path.to_string()));
    }
}

#[derive(Clone)]
pub struct AgentStoreClientDependencies {
    pub backend_url: String,
    pub get_access_token: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub get_machine_id: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
}

#[derive(Clone)]
pub struct AgentStoreClient {
    inner: Arc<AgentStoreClientInner>,
}

struct AgentStoreClientInner {
    deps: AgentStoreClientDependencies,
    tokens: Mutex<HashMap<String, CachedAgentStoreToken>>,
    http: Client,
}

#[derive(Debug, Clone)]
struct CachedAgentStoreToken {
    token: String,
    expires_at_ms: u64,
    store_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStoreReadObject {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStoreObjectProbe {
    Absent,
    Present { etag: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStoreWritePrecondition {
    BaseEtag(String),
    ExpectAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStoreWriteOutcome {
    Written { etag: Option<String> },
    AlreadyPresent,
    Conflict {
        conflict_rel_path: Option<String>,
        base_etag: Option<String>,
    },
}

fn report_conflict_protection_downgraded(
    has_conflict: bool,
    headers: &HashMap<String, String>,
) {
    if has_conflict || presign_has_conditional_headers(headers) {
        return;
    }
    let diagnostic = Map::from_iter([
        ("extension".to_string(), Value::String("box_store".to_string())),
        (
            "kind".to_string(),
            Value::String("conflict_protection_downgraded".to_string()),
        ),
    ]);
    report_box_store_diagnostic(&diagnostic);
}

fn conflict_outcome(
    conflict_rel_path: Option<String>,
    base_etag: Option<String>,
) -> AgentStoreWriteOutcome {
    let diagnostic = Map::from_iter([
        ("extension".to_string(), Value::String("box_store".to_string())),
        (
            "kind".to_string(),
            Value::String("write_conflict_preserved".to_string()),
        ),
    ]);
    report_box_store_diagnostic(&diagnostic);
    AgentStoreWriteOutcome::Conflict {
        conflict_rel_path,
        base_etag,
    }
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreSourceRefProto {
    #[prost(int32, tag = "1")]
    kind: i32,
    #[prost(string, tag = "2")]
    source_id: String,
}

#[derive(Clone, PartialEq, Message)]
struct MintAgentStoreTokenRequestProto {
    #[prost(string, tag = "1")]
    agent_id: String,
    #[prost(string, optional, tag = "2")]
    share_id: Option<String>,
    #[prost(string, optional, tag = "3")]
    store_id: Option<String>,
    #[prost(message, optional, tag = "4")]
    source: Option<AgentStoreSourceRefProto>,
}

#[derive(Clone, PartialEq, Message)]
struct MintAgentStoreTokenResponseProto {
    #[prost(string, tag = "1")]
    token: String,
    #[prost(int64, tag = "2")]
    expires_at_ms: i64,
    #[prost(string, repeated, tag = "3")]
    agent_ids: Vec<String>,
    #[prost(string, repeated, tag = "4")]
    store_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreFileEntryProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    etag: String,
    #[prost(int64, tag = "3")]
    size_bytes: i64,
    #[prost(int64, tag = "4")]
    last_modified_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct ListAgentStoreDirectoryRequestProto {
    #[prost(string, optional, tag = "1")]
    store_id: Option<String>,
    #[prost(string, optional, tag = "2")]
    share_id: Option<String>,
    #[prost(string, tag = "3")]
    relative_path: String,
    #[prost(int32, tag = "4")]
    page_size: i32,
    #[prost(string, tag = "5")]
    page_token: String,
    #[prost(bool, tag = "6")]
    prefer_complete_flat_store: bool,
}

#[derive(Clone, PartialEq, Message)]
struct ListAgentStoreDirectoryResponseProto {
    #[prost(message, repeated, tag = "1")]
    files: Vec<AgentStoreFileEntryProto>,
    #[prost(string, repeated, tag = "2")]
    subdirs: Vec<String>,
    #[prost(string, tag = "3")]
    next_page_token: String,
    #[prost(int32, tag = "4")]
    mode: i32,
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreReadInstructionProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(int64, tag = "3")]
    expires_at_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct PresignAgentStoreReadsRequestProto {
    #[prost(string, tag = "1")]
    agent_id: String,
    #[prost(string, repeated, tag = "2")]
    rel_paths: Vec<String>,
    #[prost(string, optional, tag = "3")]
    share_id: Option<String>,
    #[prost(string, optional, tag = "4")]
    store_id: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct PresignAgentStoreReadsResponseProto {
    #[prost(message, repeated, tag = "1")]
    instructions: Vec<AgentStoreReadInstructionProto>,
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreWriteFileEntryProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(int64, tag = "2")]
    size_bytes: i64,
    #[prost(string, tag = "3")]
    sha: String,
    #[prost(string, optional, tag = "4")]
    base_etag: Option<String>,
    #[prost(bool, optional, tag = "5")]
    expect_absent: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreConflictWriteInstructionProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(map = "string, string", tag = "3")]
    headers: HashMap<String, String>,
    #[prost(int64, tag = "4")]
    expires_at_ms: i64,
}

#[derive(Clone, PartialEq, Message)]
struct AgentStoreWriteInstructionProto {
    #[prost(string, tag = "1")]
    rel_path: String,
    #[prost(string, tag = "2")]
    url: String,
    #[prost(map = "string, string", tag = "3")]
    headers: HashMap<String, String>,
    #[prost(int64, tag = "4")]
    expires_at_ms: i64,
    #[prost(message, optional, tag = "5")]
    conflict: Option<AgentStoreConflictWriteInstructionProto>,
    #[prost(bool, tag = "8")]
    primary_precondition_failed: bool,
}

#[derive(Clone, PartialEq, Message)]
struct PresignAgentStoreWritesRequestProto {
    #[prost(string, tag = "1")]
    agent_id: String,
    #[prost(message, repeated, tag = "2")]
    files: Vec<AgentStoreWriteFileEntryProto>,
    #[prost(string, optional, tag = "3")]
    store_id: Option<String>,
    #[prost(string, optional, tag = "4")]
    lock_token: Option<String>,
    #[prost(string, optional, tag = "5")]
    lock_client_uuid: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct PresignAgentStoreWritesResponseProto {
    #[prost(message, repeated, tag = "1")]
    instructions: Vec<AgentStoreWriteInstructionProto>,
}

impl AgentStoreClient {
    pub fn new(deps: AgentStoreClientDependencies) -> Result<Self, String> {
        let http = Client::builder()
            .timeout(Duration::from_millis(AGENT_STORE_RPC_TIMEOUT_MS))
            .redirect(Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            inner: Arc::new(AgentStoreClientInner {
                deps,
                tokens: Mutex::new(HashMap::new()),
                http,
            }),
        })
    }

    pub fn get_object(
        &self,
        source_id: &str,
        rel_path: &str,
    ) -> Result<Option<AgentStoreReadObject>, String> {
        let rel_path = normalize_rel_path(rel_path)?;
        let instruction = match self.presign_read(source_id, &rel_path)? {
            Some(value) => value,
            None => return Ok(None),
        };
        validate_presigned_url(&self.inner.deps.backend_url, &instruction.url, &rel_path)?;
        let response = self
            .inner
            .http
            .get(&instruction.url)
            .send()
            .map_err(|error| error.to_string())?;
        read_object_response(response, &rel_path)
    }

    pub fn probe_object(
        &self,
        source_id: &str,
        rel_path: &str,
    ) -> Result<AgentStoreObjectProbe, String> {
        let rel_path = normalize_rel_path(rel_path)?;
        let instruction = self
            .presign_read(source_id, &rel_path)?
            .ok_or_else(|| format!("agent-store baseline probe returned no read for {rel_path}"))?;
        validate_presigned_url(&self.inner.deps.backend_url, &instruction.url, &rel_path)?;

        let probe = |range: bool| {
            let request = self.inner.http.get(&instruction.url);
            let request = if range {
                request.header(RANGE, "bytes=0-0")
            } else {
                request
            };
            request.send().map_err(|error| error.to_string())
        };

        let mut response = probe(true)?;
        if response.status().as_u16() == 416 {
            drop(response);
            response = probe(false)?;
        }
        match response.status().as_u16() {
            404 => Ok(AgentStoreObjectProbe::Absent),
            200 | 206 => {
                let etag = response
                    .headers()
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(normalize_s3_etag)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "agent-store baseline probe returned no usable etag for {rel_path}"
                        )
                    })?;
                Ok(AgentStoreObjectProbe::Present { etag })
            }
            status => Err(format!(
                "agent-store baseline probe failed for {rel_path}: {status}"
            )),
        }
    }

    pub fn get_object_to_file(
        &self,
        source_id: &str,
        rel_path: &str,
        dest_path: &Path,
        max_bytes: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let rel_path = normalize_rel_path(rel_path)?;
        let instruction = match self.presign_read(source_id, &rel_path)? {
            Some(value) => value,
            None => return Ok(None),
        };
        validate_presigned_url(&self.inner.deps.backend_url, &instruction.url, &rel_path)?;
        let response = self
            .inner
            .http
            .get(&instruction.url)
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!(
                "agent-store read failed for {rel_path}: {}",
                response.status().as_u16()
            ));
        }
        stream_response_to_file(response, dest_path, max_bytes).map(Some)
    }

    pub fn put_bytes(
        &self,
        source_id: &str,
        rel_path: &str,
        bytes: &[u8],
        precondition: AgentStoreWritePrecondition,
    ) -> Result<AgentStoreWriteOutcome, String> {
        let rel_path = normalize_rel_path(rel_path)?;
        let sha = sha256_hex(bytes);
        let instruction = self.presign_write(
            source_id,
            AgentStoreWriteFileEntryProto {
                rel_path: rel_path.clone(),
                size_bytes: i64::try_from(bytes.len())
                    .map_err(|_| "agent-store object size overflow".to_string())?,
                sha: sha.clone(),
                base_etag: match &precondition {
                    AgentStoreWritePrecondition::BaseEtag(value) => Some(normalize_s3_etag(value)),
                    AgentStoreWritePrecondition::ExpectAbsent => None,
                },
                expect_absent: matches!(precondition, AgentStoreWritePrecondition::ExpectAbsent)
                    .then_some(true),
            },
        )?;
        validate_presigned_url(&self.inner.deps.backend_url, &instruction.url, &rel_path)?;
        if presign_has_conditional_headers(&instruction.headers) && instruction.conflict.is_none() {
            return Err(format!(
                "agent-store presign for {rel_path} carries a conditional header but no conflict instruction"
            ));
        }
        report_conflict_protection_downgraded(
            instruction.conflict.is_some(),
            &instruction.headers,
        );
        let mut response = self.put_bytes_once(&instruction.url, &instruction.headers, &sha, bytes)?;
        if response.status().as_u16() == 409 {
            response = self.put_bytes_once(&instruction.url, &instruction.headers, &sha, bytes)?;
        }
        let status = response.status().as_u16();
        if status == 412 || status == 409 || instruction.primary_precondition_failed {
            if let Some(conflict) = instruction.conflict {
                validate_presigned_url(
                    &self.inner.deps.backend_url,
                    &conflict.url,
                    &conflict.rel_path,
                )?;
                let mut conflict_response =
                    self.put_bytes_once(&conflict.url, &conflict.headers, &sha, bytes)?;
                if is_conditional_write_rejection(conflict_response.status().as_u16()) {
                    let refreshed = self.presign_write(
                        source_id,
                        AgentStoreWriteFileEntryProto {
                            rel_path: rel_path.clone(),
                            size_bytes: i64::try_from(bytes.len())
                                .map_err(|_| "agent-store object size overflow".to_string())?,
                            sha: sha.clone(),
                            base_etag: match &precondition {
                                AgentStoreWritePrecondition::BaseEtag(value) => {
                                    Some(normalize_s3_etag(value))
                                }
                                AgentStoreWritePrecondition::ExpectAbsent => None,
                            },
                            expect_absent: matches!(
                                precondition,
                                AgentStoreWritePrecondition::ExpectAbsent
                            )
                            .then_some(true),
                        },
                    )?;
                    let refreshed_conflict = refreshed.conflict.ok_or_else(|| {
                        format!(
                            "agent-store conflict write for {rel_path} collided and the fresh presign returned no conflict instruction"
                        )
                    })?;
                    validate_presigned_url(
                        &self.inner.deps.backend_url,
                        &refreshed_conflict.url,
                        &refreshed_conflict.rel_path,
                    )?;
                    conflict_response = self.put_bytes_once(
                        &refreshed_conflict.url,
                        &refreshed_conflict.headers,
                        &sha,
                        bytes,
                    )?;
                    if !conflict_response.status().is_success() {
                        return Err(format!(
                            "agent-store conflict write failed for {rel_path}: {}",
                            conflict_response.status().as_u16()
                        ));
                    }
                    return Ok(conflict_outcome(
                        Some(refreshed_conflict.rel_path),
                        match precondition {
                            AgentStoreWritePrecondition::BaseEtag(value) => Some(value),
                            AgentStoreWritePrecondition::ExpectAbsent => None,
                        },
                    ));
                }
                if !conflict_response.status().is_success() {
                    return Err(format!(
                        "agent-store conflict write failed for {rel_path}: {}",
                        conflict_response.status().as_u16()
                    ));
                }
                return Ok(conflict_outcome(
                    Some(conflict.rel_path),
                    match precondition {
                        AgentStoreWritePrecondition::BaseEtag(value) => Some(value),
                        AgentStoreWritePrecondition::ExpectAbsent => None,
                    },
                ));
            }
            return Ok(conflict_outcome(
                None,
                match precondition {
                    AgentStoreWritePrecondition::BaseEtag(value) => Some(value),
                    AgentStoreWritePrecondition::ExpectAbsent => None,
                },
            ));
        }
        if !response.status().is_success() {
            return Err(format!(
                "agent-store write failed for {rel_path}: {}",
                response.status().as_u16()
            ));
        }
        Ok(AgentStoreWriteOutcome::Written {
            etag: response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(normalize_s3_etag)
                .filter(|value| !value.is_empty()),
        })
    }

    pub fn put_file_content_addressed(
        &self,
        source_id: &str,
        rel_path: &str,
        src_path: &Path,
    ) -> Result<AgentStoreWriteOutcome, String> {
        let rel_path = normalize_rel_path(rel_path)?;
        let metadata = fs::metadata(src_path).map_err(|error| error.to_string())?;
        let size = metadata.len();
        let sha = sha256_file(src_path)?;
        let instruction = self.presign_write(
            source_id,
            AgentStoreWriteFileEntryProto {
                rel_path: rel_path.clone(),
                size_bytes: i64::try_from(size)
                    .map_err(|_| "agent-store object size overflow".to_string())?,
                sha: sha.clone(),
                base_etag: None,
                expect_absent: Some(true),
            },
        )?;
        validate_presigned_url(&self.inner.deps.backend_url, &instruction.url, &rel_path)?;
        report_conflict_protection_downgraded(
            instruction.conflict.is_some(),
            &instruction.headers,
        );
        let mut response =
            self.put_file_once(&instruction.url, &instruction.headers, &sha, src_path, size)?;
        if response.status().as_u16() == 409 {
            response =
                self.put_file_once(&instruction.url, &instruction.headers, &sha, src_path, size)?;
        }
        match response.status().as_u16() {
            412 => Ok(AgentStoreWriteOutcome::AlreadyPresent),
            status if (200..300).contains(&status) => Ok(AgentStoreWriteOutcome::Written {
                etag: response
                    .headers()
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(normalize_s3_etag)
                    .filter(|value| !value.is_empty()),
            }),
            status => Err(format!(
                "agent-store write failed for {rel_path}: {status}"
            )),
        }
    }

    pub fn presign_read_batch(
        &self,
        source_id: &str,
        rel_paths: &[String],
    ) -> Result<HashMap<String, (String, i64)>, String> {
        let mut by_rel_path = HashMap::new();
        for chunk in rel_paths.chunks(PRESIGN_READ_BATCH_MAX) {
            let canonical = chunk
                .iter()
                .map(|path| normalize_rel_path(path))
                .collect::<Result<Vec<_>, _>>()?;
            let token = self.token_for(source_id)?;
            let response: PresignAgentStoreReadsResponseProto = self.agent_rpc(
                source_id,
                PRESIGN_AGENT_STORE_READS_PATH,
                PresignAgentStoreReadsRequestProto {
                    agent_id: String::new(),
                    rel_paths: canonical,
                    share_id: None,
                    store_id: Some(token.store_id),
                },
            )?;
            for instruction in response.instructions {
                let rel_path = normalize_rel_path(&instruction.rel_path)?;
                validate_presigned_url(
                    &self.inner.deps.backend_url,
                    &instruction.url,
                    &rel_path,
                )?;
                by_rel_path.insert(rel_path, (instruction.url, instruction.expires_at_ms));
            }
        }
        Ok(by_rel_path)
    }

    pub fn list_objects(&self, source_id: &str, prefix: &str) -> Result<Vec<String>, String> {
        let normalized_prefix = if prefix.trim().is_empty() {
            String::new()
        } else {
            normalize_rel_path(prefix)?
        };
        let mut pending = vec![normalized_prefix.clone()];
        let mut found = Vec::new();
        let mut visited = std::collections::HashSet::new();

        while let Some(relative_path) = pending.pop() {
            if !visited.insert(relative_path.clone()) {
                continue;
            }
            let mut page_token = String::new();
            for page in 0..LIST_MAX_PAGES {
                let token = self.token_for(source_id)?;
                let response: ListAgentStoreDirectoryResponseProto = self.agent_rpc(
                    source_id,
                    LIST_AGENT_STORE_DIRECTORY_PATH,
                    ListAgentStoreDirectoryRequestProto {
                        store_id: Some(token.store_id.clone()),
                        share_id: None,
                        relative_path: relative_path.clone(),
                        page_size: LIST_PAGE_SIZE,
                        page_token: page_token.clone(),
                        prefer_complete_flat_store: relative_path.is_empty() && page == 0,
                    },
                )?;
                for file in response.files {
                    let rel_path = normalize_rel_path(&file.rel_path)?;
                    if normalized_prefix.is_empty()
                        || rel_path == normalized_prefix
                        || rel_path.starts_with(&format!("{normalized_prefix}/"))
                    {
                        found.push(rel_path);
                    }
                }
                for subdir in response.subdirs {
                    let candidate = if relative_path.is_empty() {
                        subdir
                    } else if subdir.starts_with(&relative_path) {
                        subdir
                    } else {
                        format!("{relative_path}/{subdir}")
                    };
                    if let Ok(candidate) = normalize_rel_path(&candidate) {
                        pending.push(candidate);
                    }
                }
                if response.next_page_token.is_empty() {
                    break;
                }
                page_token = response.next_page_token;
                if page + 1 == LIST_MAX_PAGES {
                    return Err(format!(
                        "agent-store directory listing exceeded {LIST_MAX_PAGES} pages for {relative_path}"
                    ));
                }
            }
        }

        found.sort();
        found.dedup();
        Ok(found)
    }

    fn presign_read(
        &self,
        source_id: &str,
        rel_path: &str,
    ) -> Result<Option<AgentStoreReadInstructionProto>, String> {
        let token = self.token_for(source_id)?;
        let response: PresignAgentStoreReadsResponseProto = self.agent_rpc(
            source_id,
            PRESIGN_AGENT_STORE_READS_PATH,
            PresignAgentStoreReadsRequestProto {
                agent_id: String::new(),
                rel_paths: vec![rel_path.to_string()],
                share_id: None,
                store_id: Some(token.store_id),
            },
        )?;
        let Some(instruction) = response.instructions.into_iter().next() else {
            return Ok(None);
        };
        if normalize_rel_path(&instruction.rel_path)? != rel_path {
            return Err(format!(
                "agent-store presign read relPath mismatch for {rel_path}: {}",
                instruction.rel_path
            ));
        }
        Ok(Some(instruction))
    }

    fn presign_write(
        &self,
        source_id: &str,
        file: AgentStoreWriteFileEntryProto,
    ) -> Result<AgentStoreWriteInstructionProto, String> {
        let expected_rel_path = normalize_rel_path(&file.rel_path)?;
        let token = self.token_for(source_id)?;
        let response: PresignAgentStoreWritesResponseProto = self.agent_rpc(
            source_id,
            PRESIGN_AGENT_STORE_WRITES_PATH,
            PresignAgentStoreWritesRequestProto {
                agent_id: String::new(),
                files: vec![file],
                store_id: Some(token.store_id),
                lock_token: None,
                lock_client_uuid: None,
            },
        )?;
        let instruction = response
            .instructions
            .into_iter()
            .next()
            .ok_or_else(|| format!("agent-store presign returned no write for {expected_rel_path}"))?;
        if normalize_rel_path(&instruction.rel_path)? != expected_rel_path {
            return Err(format!(
                "agent-store presign write relPath mismatch for {expected_rel_path}: {}",
                instruction.rel_path
            ));
        }
        Ok(instruction)
    }

    fn token_for(&self, source_id: &str) -> Result<CachedAgentStoreToken, String> {
        let now = now_ms();
        if let Some(token) = self
            .inner
            .tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(source_id)
            .cloned()
            .filter(|token| {
                token.expires_at_ms.saturating_sub(now) > AGENT_STORE_TOKEN_REFRESH_BUFFER_MS
            })
        {
            return Ok(token);
        }

        let request = mint_request_for_source_id(source_id)?;
        let response: MintAgentStoreTokenResponseProto = self
            .cursor_rpc(MINT_AGENT_STORE_TOKEN_PATH, request, &[])
            .map_err(|error| error.to_string())?;
        let store_id = response
            .store_ids
            .into_iter()
            .chain(response.agent_ids)
            .find(|value| !value.trim().is_empty())
            .ok_or_else(|| "agent-store token did not include a scoped store id".to_string())?;
        if response.token.trim().is_empty() {
            return Err("agent-store token mint returned an empty token".into());
        }
        let cached = CachedAgentStoreToken {
            token: response.token,
            expires_at_ms: u64::try_from(response.expires_at_ms).unwrap_or_default(),
            store_id,
        };
        self.inner
            .tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(source_id.to_string(), cached.clone());
        Ok(cached)
    }

    fn invalidate_token(&self, source_id: &str) {
        self.inner
            .tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(source_id);
    }

    fn agent_rpc<Req, Res>(
        &self,
        source_id: &str,
        path: &str,
        request: Req,
    ) -> Result<Res, String>
    where
        Req: Message + Clone,
        Res: Message + Default,
    {
        let token = self.token_for(source_id)?;
        let headers = vec![(AGENT_STORE_TOKEN_HEADER.to_string(), token.token)];
        match self.cursor_rpc(path, request.clone(), &headers) {
            Ok(response) => Ok(response),
            Err(error) if is_unauthorized_cursor_error(&error) => {
                self.invalidate_token(source_id);
                let fresh = self.token_for(source_id)?;
                let headers = vec![(AGENT_STORE_TOKEN_HEADER.to_string(), fresh.token)];
                self.cursor_rpc(path, request, &headers)
                    .map_err(|error| error.to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn cursor_rpc<Req, Res>(
        &self,
        path: &str,
        request: Req,
        extra_headers: &[(String, String)],
    ) -> Result<Res, CursorBackendError>
    where
        Req: Message,
        Res: Message + Default,
    {
        let access_token = (self.inner.deps.get_access_token)()
            .map_err(CursorBackendError::Transport)?;
        let machine_id = (self.inner.deps.get_machine_id)()
            .map_err(CursorBackendError::Transport)?;
        let ghost_mode = resolve_sand_ghost_mode_header(
            &self.inner.deps.backend_url,
            &access_token,
            &machine_id,
        );
        let mut body = Vec::new();
        request
            .encode(&mut body)
            .map_err(|error| CursorBackendError::InvalidProto(error.to_string()))?;
        let response = send_cursor_unary_with_headers(
            &self.inner.deps.backend_url,
            &access_token,
            &machine_id,
            path,
            &body,
            AGENT_STORE_RPC_TIMEOUT_MS,
            ghost_mode,
            extra_headers,
        )?;
        Res::decode(response.as_slice())
            .map_err(|error| CursorBackendError::InvalidProto(error.to_string()))
    }

    fn put_bytes_once(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        sha: &str,
        bytes: &[u8],
    ) -> Result<Response, String> {
        let request = apply_presigned_headers(
            self.inner.http.put(url),
            headers,
            Some(("x-amz-meta-content-sha256", sha)),
        )?;
        request
            .body(bytes.to_vec())
            .send()
            .map_err(|error| error.to_string())
    }

    fn put_file_once(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        sha: &str,
        src_path: &Path,
        size: u64,
    ) -> Result<Response, String> {
        let request = apply_presigned_headers(
            self.inner.http.put(url),
            headers,
            Some(("x-amz-meta-content-sha256", sha)),
        )?
        .header(CONTENT_LENGTH, size);
        let file = File::open(src_path).map_err(|error| error.to_string())?;
        request
            .body(Body::new(file))
            .send()
            .map_err(|error| error.to_string())
    }
}

fn read_object_response(response: Response, label: &str) -> Result<Option<AgentStoreReadObject>, String> {
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "agent-store read failed for {label}: {}",
            response.status().as_u16()
        ));
    }
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(normalize_s3_etag)
        .filter(|value| !value.is_empty());
    let bytes = response
        .bytes()
        .map_err(|error| error.to_string())?
        .to_vec();
    Ok(Some(AgentStoreReadObject { bytes, etag }))
}

fn stream_response_to_file(
    mut response: Response,
    dest_path: &Path,
    max_bytes: Option<u64>,
) -> Result<u64, String> {
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = File::create(dest_path).map_err(|error| error.to_string())?;
    let mut written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = response.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        written = written.saturating_add(count as u64);
        if max_bytes.is_some_and(|max| written > max) {
            drop(file);
            let _ = fs::remove_file(dest_path);
            return Err(format!(
                "agent-store object exceeded {}B",
                max_bytes.unwrap_or_default()
            ));
        }
        file.write_all(&buffer[..count])
            .map_err(|error| error.to_string())?;
    }
    file.flush().map_err(|error| error.to_string())?;
    Ok(written)
}

fn apply_presigned_headers(
    mut request: RequestBuilder,
    headers: &HashMap<String, String>,
    fallback: Option<(&str, &str)>,
) -> Result<RequestBuilder, String> {
    if headers.is_empty() {
        if let Some((name, value)) = fallback {
            request = request.header(name, value);
        }
        return Ok(request);
    }
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| error.to_string())?;
        let value = HeaderValue::from_str(value).map_err(|error| error.to_string())?;
        request = request.header(name, value);
    }
    Ok(request)
}

fn mint_request_for_source_id(source_id: &str) -> Result<MintAgentStoreTokenRequestProto, String> {
    let source_id = source_id.trim();
    if source_id.is_empty() {
        return Err("agent-store source id must not be empty".into());
    }
    let kind = if source_id
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bc-"))
    {
        1
    } else if is_uuid_like(source_id) {
        2
    } else {
        return Err(format!(
            "agent-store source id is not a recovered local/cloud source id: {source_id}"
        ));
    };
    Ok(MintAgentStoreTokenRequestProto {
        agent_id: String::new(),
        share_id: None,
        store_id: None,
        source: Some(AgentStoreSourceRefProto {
            kind,
            source_id: source_id.to_string(),
        }),
    })
}

fn is_uuid_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
}

fn is_unauthorized_cursor_error(error: &CursorBackendError) -> bool {
    matches!(
        error,
        CursorBackendError::HttpStatus {
            status: 401 | 403,
            ..
        }
    )
}

fn validate_presigned_url(backend_url: &str, raw_url: &str, rel_path: &str) -> Result<(), String> {
    let url = Url::parse(raw_url)
        .map_err(|_| format!("Refusing unparseable presigned URL for {rel_path}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "Refusing presigned URL for {rel_path}: embedded userinfo is forbidden"
        ));
    }

    let backend = Url::parse(backend_url).ok();
    let backend_host = backend
        .as_ref()
        .and_then(Url::host_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if (backend_host == "localhost" || backend_host.ends_with(".lclhst.build"))
        && url.scheme() == "http"
        && is_loopback_host_name(url.host_str().unwrap_or_default())
    {
        return Ok(());
    }

    if url.scheme() != "https" {
        return Err(format!(
            "Refusing presigned URL for {rel_path}: non-https scheme {}",
            url.scheme()
        ));
    }
    let playground =
        backend_host == "playground.cursor.sh" || backend_host.ends_with(".playground.cursor.sh");
    let allowed = if playground {
        PLAYGROUND_AGENT_STORE_BUCKET_HOSTS
    } else {
        BCS_AGENT_STORE_BUCKET_HOSTS
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !allowed.iter().any(|candidate| *candidate == host) {
        return Err(format!(
            "Refusing presigned URL for {rel_path}: host is not allowlisted: {host}"
        ));
    }
    Ok(())
}

fn is_loopback_host_name(hostname: &str) -> bool {
    matches!(
        hostname.to_ascii_lowercase().as_str(),
        "localhost" | "ip6-localhost" | "127.0.0.1" | "::1" | "0:0:0:0:0:0:0:1"
    )
}

fn presign_has_conditional_headers(headers: &HashMap<String, String>) -> bool {
    headers.keys().any(|name| {
        name.eq_ignore_ascii_case("if-match") || name.eq_ignore_ascii_case("if-none-match")
    })
}

pub fn is_conditional_write_rejection(status: u16) -> bool {
    matches!(status, 409 | 412)
}

pub fn normalize_rel_path(rel_path: &str) -> Result<String, String> {
    if rel_path.is_empty() {
        return Err("Agent store relative path must be a non-empty string".into());
    }
    if rel_path.contains('\0') {
        return Err("Agent store relative path must not contain NUL bytes".into());
    }
    let normalized = rel_path.replace('\\', "/");
    let bytes = normalized.as_bytes();
    let windows_drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':';
    if normalized.starts_with('/') || normalized.starts_with("//") || windows_drive {
        return Err(format!(
            "Agent store relative path must not be absolute: {rel_path}"
        ));
    }

    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(format!(
                "Agent store relative path must not contain '..': {rel_path}"
            ));
        }
        if segment.len() > 255 {
            return Err(format!(
                "Agent store path segment exceeds 255 bytes: {segment}"
            ));
        }
        if let Some(byte) = segment
            .as_bytes()
            .iter()
            .copied()
            .find(|byte| *byte < 32 || *byte == 127)
        {
            return Err(format!(
                "Agent store path segment contains control character (0x{byte:x}): {segment}"
            ));
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(format!(
                "Agent store path segment must not end with '.' or space (Windows trims these): {segment}"
            ));
        }
        let stem = segment
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if is_windows_reserved_name(&stem) {
            return Err(format!(
                "Agent store path segment uses a reserved device name: {segment}"
            ));
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Err(format!(
            "Agent store relative path resolved to empty: {rel_path}"
        ));
    }
    Ok(segments.join("/"))
}

fn is_windows_reserved_name(stem: &str) -> bool {
    matches!(stem, "con" | "prn" | "aux" | "nul")
        || matches!(stem.strip_prefix("com"), Some(n) if matches!(n, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"))
        || matches!(stem.strip_prefix("lpt"), Some(n) if matches!(n, "1"|"2"|"3"|"4"|"5"|"6"|"7"|"8"|"9"))
}

pub fn batch_read_paths<I, S>(paths: I) -> Vec<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let paths = paths.into_iter().map(Into::into).collect::<Vec<_>>();
    paths
        .chunks(PRESIGN_READ_BATCH_MAX)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn normalize_s3_etag(value: &str) -> String {
    value.replace('"', "").trim().to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_outcome_reports_frozen_preservation_diagnostic() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::<Map<String, Value>>::new()));
        let captured = Arc::clone(&events);
        super::super::box_store_diagnostics::pin_box_store_diagnostics_reporter(Some(Arc::new(
            move |event| captured.lock().expect("events").push(event.clone()),
        )));
        let outcome = conflict_outcome(Some("conflicts/object".into()), Some("etag-a".into()));
        super::super::box_store_diagnostics::pin_box_store_diagnostics_reporter(None);

        assert!(matches!(
            outcome,
            AgentStoreWriteOutcome::Conflict {
                conflict_rel_path: Some(_),
                base_etag: Some(_)
            }
        ));
        let events = events.lock().expect("events");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].get("extension").and_then(Value::as_str),
            Some("box_store")
        );
        assert_eq!(
            events[0].get("kind").and_then(Value::as_str),
            Some("write_conflict_preserved")
        );
        drop(events);

        let downgrade_events = Arc::new(Mutex::new(Vec::<Map<String, Value>>::new()));
        let captured = Arc::clone(&downgrade_events);
        super::super::box_store_diagnostics::pin_box_store_diagnostics_reporter(Some(Arc::new(
            move |event| captured.lock().expect("downgrade events").push(event.clone()),
        )));
        report_conflict_protection_downgraded(false, &HashMap::new());
        report_conflict_protection_downgraded(true, &HashMap::new());
        super::super::box_store_diagnostics::pin_box_store_diagnostics_reporter(None);
        let downgrade_events = downgrade_events.lock().expect("downgrade events");
        assert_eq!(downgrade_events.len(), 1);
        assert_eq!(
            downgrade_events[0].get("kind").and_then(Value::as_str),
            Some("conflict_protection_downgraded")
        );
    }

    #[test]
    fn mint_target_matches_recovered_local_and_cloud_source_kinds() {
        let local = mint_request_for_source_id("123e4567-e89b-12d3-a456-426614174000")
            .expect("local mint target");
        let local = local.source.expect("local source");
        assert_eq!(local.kind, 2);
        assert_eq!(local.source_id, "123e4567-e89b-12d3-a456-426614174000");

        let cloud =
            mint_request_for_source_id("bc-prod-123e4567-e89b-12d3-a456-426614174000")
                .expect("cloud mint target");
        assert_eq!(cloud.source.expect("cloud source").kind, 1);
    }

    #[test]
    fn recovered_agent_store_proto_fields_round_trip() {
        let request = PresignAgentStoreWritesRequestProto {
            agent_id: String::new(),
            files: vec![AgentStoreWriteFileEntryProto {
                rel_path: "manifest.json".into(),
                size_bytes: 4,
                sha: "abcd".into(),
                base_etag: Some("etag-1".into()),
                expect_absent: None,
            }],
            store_id: Some("store-1".into()),
            lock_token: None,
            lock_client_uuid: None,
        };
        let encoded = request.encode_to_vec();
        let decoded = PresignAgentStoreWritesRequestProto::decode(encoded.as_slice())
            .expect("decode write request");
        assert_eq!(decoded.store_id.as_deref(), Some("store-1"));
        assert_eq!(decoded.files[0].base_etag.as_deref(), Some("etag-1"));
        assert_eq!(decoded.files[0].expect_absent, None);
    }

    #[test]
    fn conditional_presign_headers_require_conflict_protection() {
        let mut headers = HashMap::new();
        headers.insert("If-Match".to_string(), "etag-1".to_string());
        assert!(presign_has_conditional_headers(&headers));

        headers.clear();
        headers.insert("if-none-match".to_string(), "*".to_string());
        assert!(presign_has_conditional_headers(&headers));

        headers.clear();
        headers.insert("x-amz-meta-content-sha256".to_string(), "abc".to_string());
        assert!(!presign_has_conditional_headers(&headers));
    }

    #[test]
    fn localhost_presigns_are_allowed_only_for_local_backends() {
        assert!(validate_presigned_url(
            "http://localhost:3000",
            "http://127.0.0.1:4000/object",
            "manifest.json"
        )
        .is_ok());
        assert!(validate_presigned_url(
            "https://api2.cursor.sh",
            "http://127.0.0.1:4000/object",
            "manifest.json"
        )
        .is_err());
    }
}
