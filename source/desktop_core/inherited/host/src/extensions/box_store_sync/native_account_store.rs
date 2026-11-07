//! Native account file transport owned by BoxStoreSync. Production cutover also
//! requires large-file/pack support; this client does not emulate vendor RPCs.
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use reqwest::{Method, blocking::Client};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

use crate::extensions::auth::auth_service::{FabushiCoreCredential, FabushiCoreCredentialProvider};
use crate::extensions::auth::credential_renewer::configured_product_backend_url;

pub const MAX_NATIVE_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 12 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum NativeFileError {
    #[error("Native file account/session changed or is unavailable.")]
    Account,
    #[error("Invalid native file request.")]
    InvalidInput,
    #[error("Invalid native file service response.")]
    Protocol,
    #[error("Native file service returned HTTP {0}.")]
    Status(u16),
    #[error("Native file request failed; a write outcome may be unknown. Reconcile before retry.")]
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFile {
    pub bytes: Vec<u8>,
    pub etag: String,
}
#[derive(Debug, Clone)]
pub enum NativeWritePrecondition { ExpectAbsent, BaseEtag(String) }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeWriteOutcome {
    Written { etag: String },
    Conflict { conflict_path: String, current_etag: Option<String> },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDeleteOutcome { Deleted, Absent, Conflict { current_etag: String } }

pub struct NativeAccountStoreClient {
    base: Url,
    credentials: Arc<dyn FabushiCoreCredentialProvider>,
    identity: (String, String, String, String),
    http: Client,
}
fn identity(c: &FabushiCoreCredential) -> (String, String, String, String) {
    (c.issuer().into(), c.user_id.clone(), c.session_id.clone(), c.device_id.clone())
}
fn valid_path(path: &str) -> bool {
    path.len() <= 4096 && path.trim() == path
        && super::agent_store_sand_files::normalize_rel_path(path).ok().as_deref() == Some(path)
}
fn valid_etag(etag: &str) -> bool {
    etag.strip_prefix("sha256:").is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
fn digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, NativeFileError> {
    v.get(key).and_then(Value::as_str).ok_or(NativeFileError::Protocol)
}

impl NativeAccountStoreClient {
    pub fn for_current_account(base: &str, credentials: Arc<dyn FabushiCoreCredentialProvider>) -> Result<Self, NativeFileError> {
        let approved = configured_product_backend_url(Some(base), None).map_err(|_| NativeFileError::InvalidInput)?;
        let base = Url::parse(&approved).map_err(|_| NativeFileError::InvalidInput)?;
        let credential = credentials.get_fabushi_core_credential().map_err(|_| NativeFileError::Account)?;
        credential.authorize_origin(base.as_str()).map_err(|_| NativeFileError::Account)?;
        let http = Client::builder().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(60))
            .build().map_err(|_| NativeFileError::OutcomeUnknown)?;
        Ok(Self { base, identity: identity(&credential), credentials, http })
    }
    fn credential(&self) -> Result<FabushiCoreCredential, NativeFileError> {
        let c = self.credentials.get_fabushi_core_credential().map_err(|_| NativeFileError::Account)?;
        c.authorize_origin(self.base.as_str()).map_err(|_| NativeFileError::Account)?;
        if identity(&c) != self.identity { return Err(NativeFileError::Account); }
        Ok(c)
    }
    pub(super) fn ensure_current_account(&self) -> Result<(), NativeFileError> {
        self.credential().map(|_| ())
    }
    fn url(&self, agent: &str) -> Result<Url, NativeFileError> {
        if agent.is_empty() || agent.len() > 160 || !agent.as_bytes()[0].is_ascii_alphanumeric()
            || !agent.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)) {
            return Err(NativeFileError::InvalidInput);
        }
        self.base.join(&format!("/v1/account/agents/{agent}/store")).map_err(|_| NativeFileError::InvalidInput)
    }
    fn request(&self, method: Method, url: Url, body: Option<Value>) -> Result<(u16, Value), NativeFileError> {
        let credential = self.credential()?;
        let mut request = self.http.request(method, url).bearer_auth(credential.access_token())
            .header("x-fabushi-device-id", &credential.device_id).header("accept", "application/json");
        if let Some(body) = body { request = request.json(&body); }
        let response = request.send().map_err(|_| NativeFileError::OutcomeUnknown)?;
        let status = response.status().as_u16();
        if response.content_length().is_some_and(|v| v > MAX_JSON_BYTES) { return Err(NativeFileError::Protocol); }
        let mut bytes = Vec::new();
        response.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes).map_err(|_| NativeFileError::OutcomeUnknown)?;
        self.credential()?;
        if ![200, 404, 409].contains(&status) { return Err(NativeFileError::Status(status)); }
        if bytes.len() as u64 > MAX_JSON_BYTES { return Err(NativeFileError::Protocol); }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| NativeFileError::Protocol)?;
        if !value.is_object() { return Err(NativeFileError::Protocol); }
        Ok((status, value))
    }
    pub fn get(&self, agent: &str, path: &str) -> Result<Option<NativeFile>, NativeFileError> {
        if !valid_path(path) { return Err(NativeFileError::InvalidInput); }
        let mut url = self.url(agent)?; url.query_pairs_mut().append_pair("path", path);
        let (status, value) = self.request(Method::GET, url, None)?;
        if status == 404 && value.get("error").and_then(Value::as_str) == Some("agent_store_object_not_found")
            && value.get("agentId").and_then(Value::as_str) == Some(agent) && value.get("path").and_then(Value::as_str) == Some(path) {
            return Ok(None);
        }
        if status != 200 { return Err(NativeFileError::Status(status)); }
        if string(&value, "agentId")? != agent || string(&value, "path")? != path { return Err(NativeFileError::Protocol); }
        let encoded = string(&value, "dataBase64")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| NativeFileError::Protocol)?;
        let sha = digest(&bytes); let etag = format!("sha256:{sha}");
        if bytes.len() > MAX_NATIVE_FILE_BYTES || string(&value, "sha256")? != sha || string(&value, "blobId")? != sha
            || string(&value, "etag")? != etag || value.get("sizeBytes").and_then(Value::as_u64) != Some(bytes.len() as u64) {
            return Err(NativeFileError::Protocol);
        }
        Ok(Some(NativeFile { bytes, etag }))
    }
    pub fn put(&self, agent: &str, path: &str, bytes: &[u8], precondition: NativeWritePrecondition) -> Result<NativeWriteOutcome, NativeFileError> {
        if !valid_path(path) || bytes.len() > MAX_NATIVE_FILE_BYTES { return Err(NativeFileError::InvalidInput); }
        let mut body = json!({"path":path,"dataBase64":base64::engine::general_purpose::STANDARD.encode(bytes)});
        match precondition {
            NativeWritePrecondition::ExpectAbsent => body["expectAbsent"] = json!(true),
            NativeWritePrecondition::BaseEtag(etag) => {
                if !valid_etag(&etag) { return Err(NativeFileError::InvalidInput); }
                body["baseEtag"] = json!(etag);
            }
        }
        let (status, value) = self.request(Method::PUT, self.url(agent)?, Some(body))?;
        if string(&value, "path")? != path { return Err(NativeFileError::Protocol); }
        if status == 409 && string(&value, "outcome")? == "conflict" {
            let conflict_path = string(&value, "conflictPath")?;
            let current = match value.get("baseEtag") { Some(Value::Null) => None, Some(Value::String(v)) if valid_etag(v) => Some(v.clone()), _ => return Err(NativeFileError::Protocol) };
            if !valid_path(conflict_path) || !conflict_path.starts_with(".conflicts/") || !conflict_path.ends_with(&format!("/{path}")) { return Err(NativeFileError::Protocol); }
            return Ok(NativeWriteOutcome::Conflict { conflict_path: conflict_path.into(), current_etag: current });
        }
        if status != 200 { return Err(NativeFileError::Status(status)); }
        let sha = digest(bytes); let etag = format!("sha256:{sha}");
        if string(&value, "outcome")? != "written" || string(&value, "agentId")? != agent
            || string(&value, "sha256")? != sha || string(&value, "blobId")? != sha || string(&value, "etag")? != etag {
            return Err(NativeFileError::Protocol);
        }
        Ok(NativeWriteOutcome::Written { etag })
    }
    pub fn list(&self, agent: &str, prefix: &str) -> Result<Vec<String>, NativeFileError> {
        if !prefix.is_empty() && !valid_path(prefix) { return Err(NativeFileError::InvalidInput); }
        let mut url = self.url(agent)?; url.query_pairs_mut().append_pair("prefix", prefix);
        let (status, value) = self.request(Method::GET, url, None)?;
        if status != 200 { return Err(NativeFileError::Status(status)); }
        if string(&value, "agentId")? != agent { return Err(NativeFileError::Protocol); }
        let files = value.get("files").and_then(Value::as_array).ok_or(NativeFileError::Protocol)?;
        if files.len() > 20_000 { return Err(NativeFileError::Protocol); }
        let mut paths = std::collections::BTreeSet::new();
        for file in files {
            let path = string(file, "path")?;
            if !valid_path(path) || !path.starts_with(prefix) || !valid_etag(string(file, "etag")?) || !paths.insert(path.to_string()) { return Err(NativeFileError::Protocol); }
        }
        Ok(paths.into_iter().collect())
    }
    pub fn delete(&self, agent: &str, path: &str, base_etag: &str) -> Result<NativeDeleteOutcome, NativeFileError> {
        if !valid_path(path) || !valid_etag(base_etag) { return Err(NativeFileError::InvalidInput); }
        let mut url = self.url(agent)?; url.query_pairs_mut().append_pair("path", path).append_pair("baseEtag", base_etag);
        let (status, value) = self.request(Method::DELETE, url, None)?;
        if status != 200 { return Err(NativeFileError::Status(status)); }
        if string(&value, "agentId")? != agent || string(&value, "path")? != path { return Err(NativeFileError::Protocol); }
        match (value.get("deleted").and_then(Value::as_bool), value.get("conflict")) {
            (Some(true), None) => Ok(NativeDeleteOutcome::Deleted),
            (Some(false), None) => Ok(NativeDeleteOutcome::Absent),
            (Some(false), Some(Value::Bool(true))) => {
                let etag = string(&value, "etag")?;
                if !valid_etag(etag) { return Err(NativeFileError::Protocol); }
                Ok(NativeDeleteOutcome::Conflict { current_etag: etag.into() })
            }
            _ => Err(NativeFileError::Protocol),
        }
    }
}
