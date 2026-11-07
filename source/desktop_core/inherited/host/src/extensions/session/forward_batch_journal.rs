use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::agent_db::SandAgentDb;

const FORWARD_BATCHES_KEY: &str = "humanForwardBatches";
const FORWARD_BATCH_VERSION: u32 = 1;
const MAX_FORWARD_ATTACHMENTS: usize = 6;
const MAX_FORWARD_ATTACHMENT_BYTES: u64 = super::native_messaging::MAX_HUMAN_RESOURCE_UPLOAD_BYTES;
const MAX_FORWARD_BATCH_ATTACHMENT_BYTES: u64 =
    MAX_FORWARD_ATTACHMENT_BYTES * MAX_FORWARD_ATTACHMENTS as u64;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForwardAttachmentSnapshot {
    version: u32,
    sha256: String,
    size: u64,
}

fn forward_attachment_scope(account: &str, issuer: &str, request: &str) -> Result<String, String> {
    if account.is_empty() || issuer.is_empty() || request.is_empty() || request.len() > 200 {
        return Err("Human forward attachment snapshot has an invalid account/request scope".into());
    }
    let mut digest = Sha256::new();
    digest.update(b"human-forward-attachments-v1\0");
    for part in [account, issuer, request] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn sync_forward_snapshot_directory(path: &Path) -> Result<(), String> {
    // File data is synchronized on every platform. Unix also permits syncing
    // the directory entry; full power-loss/filesystem qualification is separate.
    #[cfg(unix)]
    File::open(path).and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Human forward snapshot directory could not be synchronized: {error}"))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn require_forward_snapshot_directory(path: &Path, create: bool) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {
                    if let Some(parent) = path.parent() { sync_forward_snapshot_directory(parent)?; }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("Human forward snapshot directory could not be created: {error}")),
            }
        }
        Err(error) => return Err(format!("Human forward snapshot directory is unavailable: {error}")),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Human forward snapshot directory could not be inspected: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir()
        || fs::canonicalize(path).map_err(|error| error.to_string())? != path
    {
        return Err("Human forward snapshot directory must be a confined non-symlink directory".into());
    }
    Ok(())
}

fn forward_snapshot_directory(
    owner: &SandAgentDb, account: &str, issuer: &str, request: &str, create: bool,
) -> Result<PathBuf, String> {
    let scope = forward_attachment_scope(account, issuer, request)?;
    let parent = owner.db_path().parent()
        .ok_or_else(|| "Human forward source database has no conversation directory".to_string())?;
    // The existing database owner establishes the trusted base. Descendant
    // components are derived here, never taken from attachment metadata.
    let base = fs::canonicalize(if parent.as_os_str().is_empty() { Path::new(".") } else { parent })
        .map_err(|error| format!("Human forward source directory is unavailable: {error}"))?;
    let root = base.join("forward-attachments");
    require_forward_snapshot_directory(&root, create)?;
    let directory = root.join(scope);
    require_forward_snapshot_directory(&directory, create)?;
    Ok(directory)
}

fn read_forward_regular_file(path: &Path, expected_size: Option<u64>) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Human forward attachment snapshot/source is unavailable: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Human forward attachment must be a regular non-symlink file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Do not follow a replacement symlink or block on a replacement FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)
        .map_err(|error| format!("Human forward attachment could not be opened safely: {error}"))?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if opened.file_type().is_symlink() || !opened.is_file() {
        return Err("Human forward attachment handle is not a regular file".into());
    }
    let size = opened.len();
    if size == 0 || size > MAX_FORWARD_ATTACHMENT_BYTES
        || expected_size.is_some_and(|expected| expected != size)
    {
        return Err("Human forward attachment size is invalid, changed, or exceeds 25 MiB".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file).take(MAX_FORWARD_ATTACHMENT_BYTES + 1).read_to_end(&mut bytes)
        .map_err(|error| format!("Human forward attachment bounded read failed: {error}"))?;
    if bytes.len() as u64 != size || bytes.len() as u64 > MAX_FORWARD_ATTACHMENT_BYTES {
        return Err("Human forward attachment size changed during its bounded read".into());
    }
    Ok(bytes)
}

struct ForwardSnapshotTemporary {
    path: PathBuf,
    file: Option<File>,
}

impl Drop for ForwardSnapshotTemporary {
    fn drop(&mut self) {
        // Close first so cleanup also works on Windows. This guard is created
        // only after create_new succeeds and never owns a pre-existing path.
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn publish_forward_snapshot(directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    require_forward_snapshot_directory(directory, false)?;
    let temporary_path = directory.join(format!(".capture-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&temporary_path)
        .map_err(|error| format!("Human forward snapshot staging could not be created: {error}"))?;
    let mut temporary = ForwardSnapshotTemporary { path: temporary_path, file: Some(file) };
    let file = temporary.file.as_mut().expect("new snapshot temporary file");
    file.write_all(bytes).and_then(|_| file.flush()).and_then(|_| file.sync_all())
        .map_err(|error| format!("Human forward snapshot staging could not be persisted: {error}"))?;
    drop(temporary.file.take());
    require_forward_snapshot_directory(directory, false)?;
    // A same-directory hard link publishes the complete synchronized file
    // atomically without replacing an existing destination (unlike rename).
    match fs::hard_link(&temporary.path, path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_forward_regular_file(path, Some(bytes.len() as u64))?;
            if existing != bytes {
                return Err("Human forward snapshot already exists with different bytes; refusing overwrite".into());
            }
        }
        Err(error) => return Err(format!("Human forward snapshot could not be published without overwrite: {error}")),
    }
    // Also synchronize a matching orphan reused after interruption between
    // publication and the prior attempt's directory synchronization.
    sync_forward_snapshot_directory(directory)?;
    Ok(())
}

/// Capture a new batch's attachment bytes before the source journal commits.
/// A failed call may leave unreferenced complete snapshots, never a sent batch.
pub(super) fn snapshot_forward_attachments(
    owner: &SandAgentDb, account: &str, issuer: &str, request: &str, attachments: &[Value],
) -> Result<Vec<Value>, String> {
    if attachments.len() > MAX_FORWARD_ATTACHMENTS {
        return Err("Human forward supports at most six attachments".into());
    }
    forward_attachment_scope(account, issuer, request)?;
    let mut snapshots = Vec::with_capacity(attachments.len());
    let mut total = 0_u64;
    for (index, attachment) in attachments.iter().enumerate() {
        let object = attachment.as_object()
            .ok_or_else(|| "Human forward attachment must be an object".to_string())?;
        let source = object.get("path").and_then(Value::as_str).filter(|path| !path.trim().is_empty())
            .ok_or_else(|| "Human forward attachment requires a source path".to_string())?;
        if !object.get("name").and_then(Value::as_str).is_some_and(|name| !name.trim().is_empty()) {
            return Err("Human forward attachment requires a name".into());
        }
        let declared_size = object.get("size").map(|size| size.as_u64()
            .ok_or_else(|| "Human forward source attachment size is invalid".to_string())).transpose()?;
        let bytes = read_forward_regular_file(Path::new(source), declared_size)?;
        total = total.checked_add(bytes.len() as u64)
            .filter(|total| *total <= MAX_FORWARD_BATCH_ATTACHMENT_BYTES)
            .ok_or_else(|| "Human forward attachment batch size exceeds its supported limit".to_string())?;
        let snapshot = ForwardAttachmentSnapshot {
            version: 1, sha256: format!("{:x}", Sha256::digest(&bytes)), size: bytes.len() as u64,
        };
        let directory = forward_snapshot_directory(owner, account, issuer, request, true)?;
        let path = directory.join(format!("{index}-{}.bin", snapshot.sha256));
        publish_forward_snapshot(&directory, &path, &bytes)?;
        let mut captured = object.clone();
        captured.insert("path".into(), Value::String(path.to_str()
            .ok_or_else(|| "Human forward snapshot path is not valid UTF-8".to_string())?.to_string()));
        captured.insert("forwardSnapshot".into(), serde_json::to_value(snapshot).map_err(|error| error.to_string())?);
        snapshots.push(Value::Object(captured));
    }
    Ok(snapshots)
}

/// Return the same verified byte buffer that the existing Native uploader must
/// submit. Never fall back to a mutable original path or synthesize a legacy pin.
pub(super) fn read_forward_attachment_bytes(
    owner: &SandAgentDb, account: &str, issuer: &str, request: &str, index: usize, attachment: &Value,
) -> Result<Vec<u8>, String> {
    if index >= MAX_FORWARD_ATTACHMENTS {
        return Err("Human forward attachment index exceeds the six-attachment limit".into());
    }
    let pin = attachment.get("forwardSnapshot")
        .ok_or_else(|| "Human forward attachment has no pinned byte snapshot; legacy content cannot be safely recovered".to_string())?;
    let snapshot: ForwardAttachmentSnapshot = serde_json::from_value(pin.clone())
        .map_err(|error| format!("Human forward attachment snapshot metadata is invalid: {error}"))?;
    if snapshot.version != 1 || snapshot.size == 0 || snapshot.size > MAX_FORWARD_ATTACHMENT_BYTES
        || snapshot.sha256.len() != 64
        || !snapshot.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("Human forward attachment snapshot version, digest or size is invalid".into());
    }
    if attachment.get("size").is_some_and(|size| size.as_u64() != Some(snapshot.size)) {
        return Err("Human forward attachment snapshot disagrees with its source size".into());
    }
    let directory = forward_snapshot_directory(owner, account, issuer, request, false)?;
    let path = directory.join(format!("{index}-{}.bin", snapshot.sha256));
    let expected_path = path.to_str()
        .ok_or_else(|| "Human forward snapshot path is not valid UTF-8".to_string())?;
    if attachment.get("path").and_then(Value::as_str) != Some(expected_path) {
        return Err("Human forward attachment snapshot path is outside its pinned account/request/index scope".into());
    }
    let bytes = read_forward_regular_file(&path, Some(snapshot.size))?;
    if format!("{:x}", Sha256::digest(&bytes)) != snapshot.sha256 {
        return Err("Human forward attachment snapshot digest does not match its pinned bytes".into());
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HumanForwardTarget {
    pub conversation_id: String,
    pub client_nonce: String,
    pub completed: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub comment_nonce: Option<String>,
    #[serde(default)]
    pub comment_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HumanForwardBatch {
    pub version: u32,
    pub account_id: String,
    pub issuer: String,
    pub request_id: String,
    pub source_conversation_id: String,
    pub source_entry_id: String,
    pub text: String,
    pub attachments: Vec<Value>,
    pub context: Value,
    pub destinations: Vec<HumanForwardTarget>,
}

pub(super) fn forward_destination_nonce(
    account: &str,
    issuer: &str,
    request: &str,
    destination: &str,
) -> String {
    let mut digest = Sha256::new();
    for part in [account, issuer, request, destination] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("human-forward-{:x}", digest.finalize())
}

impl HumanForwardBatch {
    pub(super) fn new(
        account_id: String,
        issuer: String,
        request_id: String,
        source_conversation_id: String,
        source_entry_id: String,
        text: String,
        attachments: Vec<Value>,
        context: Value,
        destination_ids: &[String],
    ) -> Self {
        let destinations = destination_ids.iter().map(|id| HumanForwardTarget {
            conversation_id: id.clone(),
            client_nonce: forward_destination_nonce(&account_id, &issuer, &request_id, id),
            completed: false,
            error: None,
            comment_nonce: None,
            comment_completed: false,
        }).collect();
        Self {
            version: FORWARD_BATCH_VERSION,
            account_id,
            issuer,
            request_id,
            source_conversation_id,
            source_entry_id,
            text,
            attachments,
            context,
            destinations,
        }
    }

    pub(super) fn validate(&self, expected_source: &str) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        if self.version != FORWARD_BATCH_VERSION
            || self.account_id.is_empty()
            || self.issuer.is_empty()
            || self.request_id.is_empty()
            || self.request_id.len() > 200
            || self.source_conversation_id != expected_source
            || self.source_entry_id.is_empty()
            || self.destinations.is_empty()
            || self.destinations.len() > 32
            || (self.text.trim().is_empty() && self.attachments.is_empty())
            || self.context.get("batchVersion").and_then(Value::as_u64) != Some(1)
            || self.context.get("requestId").and_then(Value::as_str) != Some(self.request_id.as_str())
            || self.context.get("sourceConversationId").and_then(Value::as_str) != Some(self.source_conversation_id.as_str())
            || self.context.get("sourceEntryId").and_then(Value::as_str) != Some(self.source_entry_id.as_str())
            || self.context.pointer("/privacy/dropSenderNames").and_then(Value::as_bool).is_none()
            || self.context.pointer("/privacy/dropCaptions").and_then(Value::as_bool).is_none()
        {
            return Err("persisted Human forward batch is invalid or has an unsupported version".into());
        }
        if let Some(comment) = self.context.get("comment") {
            if !comment.is_object()
                || comment.get("text").and_then(Value::as_str).is_none()
                || !comment.get("entities").map(Value::is_array).unwrap_or(true)
            {
                return Err("persisted Human forward batch comment is invalid".into());
            }
        }
        for target in &self.destinations {
            if target.conversation_id.is_empty()
                || !seen.insert(target.conversation_id.as_str())
                || target.client_nonce != forward_destination_nonce(
                    &self.account_id, &self.issuer, &self.request_id, &target.conversation_id,
                )
                || (target.completed && target.error.is_some())
            {
                return Err("persisted Human forward destination is invalid".into());
            }
            if let Some(ref c_nonce) = target.comment_nonce {
                let expected = format!("{}:comment", target.client_nonce);
                if c_nonce != &expected {
                    return Err("persisted Human forward destination comment nonce is invalid".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn contains_requested(&self, requested: &[String]) -> bool {
        requested.iter().all(|id| self.destinations.iter().any(|dest| &dest.conversation_id == id))
    }

    pub(super) fn response(&self, requested: &[String]) -> Value {
        let mut results = Vec::new();
        for id in requested {
            if let Some(dest) = self.destinations.iter().find(|dest| &dest.conversation_id == id) {
                results.push(serde_json::json!({
                    "conversationId": id,
                    "clientNonce": dest.client_nonce,
                    "status": if dest.completed { "sent" } else { "failed" },
                    "error": if dest.completed { None } else { dest.error.as_deref().or(Some("forward delivery remains pending")) },
                }));
            }
        }
        serde_json::json!({
            "requestId": self.request_id,
            "privacy": self.context.get("privacy"),
            "destinations": results,
        })
    }
}

pub(super) fn load_forward_batches(owner: &SandAgentDb, source: &str) -> Result<Vec<HumanForwardBatch>, String> {
    let observed = read_forward_journal_value(owner)?;
    decode_forward_batches(observed, source)
}

fn read_forward_journal_value(owner: &SandAgentDb) -> Result<Option<Value>, String> {
    let metadata = owner.read_metadata().map_err(|error| error.to_string())?;
    let object = metadata.as_object()
        .ok_or_else(|| "persisted Human forward metadata must be an object".to_string())?;
    Ok(object.get(FORWARD_BATCHES_KEY).cloned())
}

fn decode_forward_batches(
    observed: Option<Value>,
    source: &str,
) -> Result<Vec<HumanForwardBatch>, String> {
    let Some(value) = observed else { return Ok(Vec::new()); };
    let batches: Vec<HumanForwardBatch> = serde_json::from_value(value)
        .map_err(|error| format!("persisted Human forward batches are invalid: {error}"))?;
    let mut seen = std::collections::BTreeSet::new();
    for batch in &batches {
        batch.validate(source)?;
        if !seen.insert((batch.account_id.as_str(), batch.issuer.as_str(), batch.request_id.as_str())) {
            return Err("persisted Human forward batches contain duplicate request identities".into());
        }
    }
    Ok(batches)
}

pub(super) fn store_forward_batch(owner: &SandAgentDb, batch: &HumanForwardBatch) -> Result<(), String> {
    batch.validate(&batch.source_conversation_id)?;
    // Read the exact key value once. A second writer may commit between
    // observation and this merge; the SandAgentDb conditional CAS detects
    // that interleaving instead of erasing another batch's journal.
    let observed = read_forward_journal_value(owner)?;
    store_forward_batch_observed(owner, batch, observed)
}

fn store_forward_batch_observed(
    owner: &SandAgentDb,
    batch: &HumanForwardBatch,
    observed: Option<Value>,
) -> Result<(), String> {
    batch.validate(&batch.source_conversation_id)?;
    let mut batches = decode_forward_batches(observed.clone(), &batch.source_conversation_id)?;
    if let Some(existing) = batches.iter_mut().find(|prior|
        prior.account_id == batch.account_id && prior.issuer == batch.issuer && prior.request_id == batch.request_id
    ) {
        if existing.source_entry_id != batch.source_entry_id
            || existing.text != batch.text
            || existing.attachments != batch.attachments
            || existing.context != batch.context
            || existing.destinations.iter().map(|item| (&item.conversation_id, &item.client_nonce))
                .ne(batch.destinations.iter().map(|item| (&item.conversation_id, &item.client_nonce)))
        {
            return Err("forwardHumanMessage clientNonce already identifies a different immutable batch".into());
        }
        if existing.destinations.iter().zip(&batch.destinations)
            .any(|(prior, requested)| prior.completed && !requested.completed)
        {
            return Err("Human forward batch progress is stale and cannot regress a completed destination".into());
        }
        *existing = batch.clone();
    } else {
        batches.push(batch.clone());
    }
    let value = serde_json::to_value(batches).map_err(|error| error.to_string())?;
    if !owner.set_metadata_if_unchanged(FORWARD_BATCHES_KEY, observed.as_ref(), value)
        .map_err(|error| error.to_string())?
    {
        return Err("Human forward batch progress could not be durably stored".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SnapshotFixture {
        root: PathBuf,
        owner: SandAgentDb,
    }

    impl SnapshotFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("forward-snapshot-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("snapshot fixture directory");
            let owner = SandAgentDb::open(root.join("store.db"), 500).expect("snapshot source owner");
            Self { root, owner }
        }

        fn source(&self, filename: &str, bytes: &[u8]) -> Value {
            let path = self.root.join(filename);
            fs::write(&path, bytes).expect("source attachment bytes");
            serde_json::json!({
                "path": path, "name": filename, "size": bytes.len(),
                "resourceId": format!("source-{filename}"), "mimeType": "application/octet-stream",
                "retainedMetadata": {"origin": "existing Human resource owner"},
            })
        }
    }

    impl Drop for SnapshotFixture {
        fn drop(&mut self) {
            self.owner.close(false);
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn snapshot_context(request: &str) -> Value {
        serde_json::json!({
            "batchVersion": 1, "requestId": request, "sourceConversationId": "source",
            "sourceEntryId": "entry", "privacy": {"dropSenderNames": false, "dropCaptions": false},
        })
    }

    #[test]
    fn forward_attachment_capture_keeps_original_bytes_and_metadata_after_source_loss_and_reopen() {
        let fixture = SnapshotFixture::new();
        let inputs = [fixture.source("first.bin", b"first bytes"), fixture.source("second.bin", b"second bytes")];
        let snapshots = snapshot_forward_attachments(&fixture.owner, "account", "issuer", "request", &inputs)
            .expect("capture complete ordered attachment set");
        assert_eq!(snapshots.len(), 2);
        for index in 0..2 {
            for key in ["name", "size", "resourceId", "mimeType", "retainedMetadata"] {
                assert_eq!(snapshots[index][key], inputs[index][key], "retain original {key}");
            }
            assert_ne!(snapshots[index]["path"], inputs[index]["path"]);
            assert_eq!(snapshots[index]["forwardSnapshot"]["version"], 1);
        }
        let batch = HumanForwardBatch::new(
            "account".into(), "issuer".into(), "request".into(), "source".into(), "entry".into(),
            String::new(), snapshots.clone(), snapshot_context("request"), &["target".into()],
        );
        store_forward_batch(&fixture.owner, &batch).expect("journal references captured bytes");
        fs::write(fixture.root.join("first.bin"), b"newer bytes").expect("same-size source replacement");
        fs::remove_file(fixture.root.join("second.bin")).expect("original cache removed");
        fixture.owner.close(false);
        let reopened = SandAgentDb::open(fixture.owner.db_path(), 500).expect("reopen durable journal");
        let loaded = load_forward_batches(&reopened, "source").expect("load persisted attachment identity");
        assert_eq!(loaded, vec![batch]);
        assert_eq!(read_forward_attachment_bytes(&reopened, "account", "issuer", "request", 0, &loaded[0].attachments[0])
            .expect("first immutable bytes"), b"first bytes");
        assert_eq!(read_forward_attachment_bytes(&reopened, "account", "issuer", "request", 1, &loaded[0].attachments[1])
            .expect("second immutable bytes"), b"second bytes");
        assert_eq!(fs::read(fixture.root.join("first.bin")).unwrap(), b"newer bytes");
        assert!(!fixture.root.join("second.bin").exists(), "recovery must not recreate the original cache");
        reopened.close(false);
    }

    #[test]
    fn forward_attachment_reader_rejects_missing_or_corrupt_snapshots_without_source_fallback() {
        let fixture = SnapshotFixture::new();
        let input = fixture.source("source.bin", b"pinned");
        let snapshots = snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[input]).unwrap();
        let attachment = &snapshots[0];
        let path = PathBuf::from(attachment["path"].as_str().unwrap());
        fs::write(fixture.root.join("source.bin"), b"source").unwrap();
        fs::remove_file(&path).unwrap();
        assert!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, attachment)
            .unwrap_err().contains("unavailable"));
        assert!(!path.exists(), "missing pin cannot be regenerated from mutable source bytes");
        fs::write(&path, b"broken").unwrap();
        assert!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, attachment)
            .unwrap_err().contains("digest"));
        for bytes in [b"x".as_slice(), b"pinned++".as_slice()] {
            fs::write(&path, bytes).unwrap();
            assert!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, attachment)
                .unwrap_err().contains("size"));
        }
        fs::write(&path, b"pinned").expect("restore only exact pinned bytes");
        assert_eq!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, attachment).unwrap(), b"pinned");
        for (key, value) in [("version", serde_json::json!(2)), ("size", serde_json::json!(0)),
            ("size", serde_json::json!(MAX_FORWARD_ATTACHMENT_BYTES + 1)),
            ("sha256", serde_json::json!("../not-a-digest"))]
        {
            let mut invalid = attachment.clone();
            invalid["forwardSnapshot"][key] = value;
            assert!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, &invalid).is_err());
        }
        assert_eq!(fs::read(fixture.root.join("source.bin")).unwrap(), b"source");
    }

    #[test]
    fn forward_attachment_publication_never_overwrites_and_cleans_only_its_owned_temporary() {
        let fixture = SnapshotFixture::new();
        let source = fixture.source("source.bin", b"original");
        let first = snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[source.clone()]).unwrap();
        let second = snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[source.clone()])
            .expect("identical complete snapshot can be reused");
        assert_eq!(first, second);
        let path = PathBuf::from(first[0]["path"].as_str().unwrap());
        let directory = path.parent().unwrap();
        let unrelated = directory.join("keep-existing-record");
        fs::write(&unrelated, b"do not delete").unwrap();
        let foreign_temporary = directory.join(".capture-foreign.tmp");
        fs::write(&foreign_temporary, b"another capture owns this").unwrap();
        fs::write(&path, b"tampered").unwrap();
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[source.clone()])
            .unwrap_err().contains("refusing overwrite"));
        assert_eq!(fs::read(&path).unwrap(), b"tampered");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[source]).is_err());
        assert!(path.is_dir(), "an existing nonregular target also cannot be replaced");
        assert_eq!(fs::read(unrelated).unwrap(), b"do not delete");
        assert_eq!(fs::read(&foreign_temporary).unwrap(), b"another capture owns this");
        assert!(fs::read_dir(directory).unwrap().all(|entry|
            {
                let entry = entry.unwrap();
                !entry.file_name().to_string_lossy().starts_with(".capture-")
                    || entry.path() == foreign_temporary
            }),
            "both successful and failing publication remove only owned temporary files");
    }

    #[test]
    fn forward_attachment_capture_bounds_count_size_and_regular_file_inputs_before_acceptance() {
        let fixture = SnapshotFixture::new();
        let missing = serde_json::json!({"path": fixture.root.join("missing"), "name": "missing"});
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &vec![missing; 7])
            .unwrap_err().contains("at most six"));
        let empty = fixture.source("empty", b"");
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[empty])
            .unwrap_err().contains("size"));
        let directory = fixture.root.join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[
            serde_json::json!({"path":directory,"name":"directory"})
        ]).unwrap_err().contains("regular"));
        let oversized = fixture.root.join("oversized");
        File::create(&oversized).unwrap().set_len(25 * 1024 * 1024 + 1).unwrap();
        assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[
            serde_json::json!({"path":oversized,"name":"oversized"})
        ]).unwrap_err().contains("size"));
        let source = fixture.source("normal", b"abc");
        for size in [serde_json::json!(4), serde_json::json!(-1), serde_json::json!("3")] {
            let mut wrong_size = source.clone();
            wrong_size["size"] = size;
            assert!(snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[wrong_size])
                .unwrap_err().contains("size"));
        }
        assert!(!fixture.root.join("forward-attachments").exists(), "invalid sources do not create accepted snapshots");
        let six = snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &vec![source; 6])
            .expect("existing six-attachment boundary remains supported");
        assert_eq!(six.len(), 6);
        for (index, attachment) in six.iter().enumerate() {
            assert_eq!(read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", index, attachment).unwrap(), b"abc");
        }
    }

    #[test]
    fn forward_attachment_exact_resource_byte_limit_remains_supported() {
        let fixture = SnapshotFixture::new();
        let path = fixture.root.join("largest-source");
        // Oracle comes from the service contract, independently of the reader constant.
        const SERVICE_UPLOAD_LIMIT: u64 = 25 * 1024 * 1024;
        File::create(&path).unwrap().set_len(SERVICE_UPLOAD_LIMIT).unwrap();
        let snapshots = snapshot_forward_attachments(&fixture.owner, "a", "i", "r", &[
            serde_json::json!({"path":path,"name":"largest","size":SERVICE_UPLOAD_LIMIT})
        ]).expect("largest supported regular file");
        let bytes = read_forward_attachment_bytes(&fixture.owner, "a", "i", "r", 0, &snapshots[0])
            .expect("largest pinned byte buffer");
        assert_eq!(bytes.len() as u64, SERVICE_UPLOAD_LIMIT);
        assert!(bytes.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn forward_attachment_paths_are_account_scoped_and_reject_aliases_and_symlinks() {
        let fixture = SnapshotFixture::new();
        let source = fixture.source("source.bin", b"source bytes");
        let captured = snapshot_forward_attachments(&fixture.owner, "a", "bc", "r", &[source.clone()]).unwrap();
        assert_ne!(forward_attachment_scope("a", "bc", "r").unwrap(), forward_attachment_scope("ab", "c", "r").unwrap());
        for (account, issuer, request, index) in [("b", "bc", "r", 0), ("a", "isolated", "r", 0),
            ("a", "bc", "other", 0), ("a", "bc", "r", 1), ("a", "bc", "r", 6)]
        {
            assert!(read_forward_attachment_bytes(&fixture.owner, account, issuer, request, index, &captured[0]).is_err());
        }
        let path = PathBuf::from(captured[0]["path"].as_str().unwrap());
        let mut outside = captured[0].clone();
        outside["path"] = source["path"].clone();
        assert!(read_forward_attachment_bytes(&fixture.owner, "a", "bc", "r", 0, &outside)
            .unwrap_err().contains("outside"));
        let mut alias = captured[0].clone();
        alias["path"] = Value::String(format!("{}/./{}", path.parent().unwrap().display(), path.file_name().unwrap().to_string_lossy()));
        assert!(read_forward_attachment_bytes(&fixture.owner, "a", "bc", "r", 0, &alias)
            .unwrap_err().contains("outside"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let original = fixture.root.join("source.bin");
            fs::remove_file(&path).unwrap();
            symlink(&original, &path).unwrap();
            assert!(read_forward_attachment_bytes(&fixture.owner, "a", "bc", "r", 0, &captured[0])
                .unwrap_err().contains("non-symlink"));
            let source_link = fixture.root.join("source-link");
            symlink(&original, &source_link).unwrap();
            assert!(snapshot_forward_attachments(&fixture.owner, "a", "bc", "r2", &[
                serde_json::json!({"path":source_link,"name":"link"})
            ]).unwrap_err().contains("non-symlink"));
            let other = SnapshotFixture::new();
            let other_source = other.source("source.bin", b"other bytes");
            let outside_directory = fixture.root.join("outside-directory");
            fs::create_dir(&outside_directory).unwrap();
            symlink(&outside_directory, other.root.join("forward-attachments")).unwrap();
            assert!(snapshot_forward_attachments(&other.owner, "a", "bc", "r", &[other_source])
                .unwrap_err().contains("non-symlink"));
            assert_eq!(fs::read_dir(&outside_directory).unwrap().count(), 0, "no writes through redirected snapshot directory");
            assert_eq!(fs::read(&original).unwrap(), b"source bytes");
        }
    }

    #[test]
    fn forward_attachment_legacy_journal_remains_loadable_without_fabricating_byte_evidence() {
        let fixture = SnapshotFixture::new();
        let unpinned = fixture.source("legacy.bin", b"current cache bytes");
        let batch = HumanForwardBatch::new(
            "account".into(), "issuer".into(), "request".into(), "source".into(), "entry".into(),
            String::new(), vec![unpinned.clone()], snapshot_context("request"), &["target".into()],
        );
        store_forward_batch(&fixture.owner, &batch).expect("preserve recognized legacy format");
        let before = fixture.owner.get_metadata(FORWARD_BATCHES_KEY).unwrap();
        let loaded = load_forward_batches(&fixture.owner, "source").expect("legacy journal must not disappear");
        assert_eq!(loaded, vec![batch]);
        assert!(read_forward_attachment_bytes(&fixture.owner, "account", "issuer", "request", 0, &loaded[0].attachments[0])
            .unwrap_err().contains("no pinned byte snapshot"));
        assert_eq!(fixture.owner.get_metadata(FORWARD_BATCHES_KEY).unwrap(), before);
        assert_eq!(loaded[0].attachments[0], unpinned);
        assert!(!fixture.root.join("forward-attachments").exists());
        let text_only = HumanForwardBatch::new(
            "account".into(), "issuer".into(), "text-only".into(), "source".into(), "entry".into(),
            "text".into(), vec![], snapshot_context("text-only"), &["target".into()],
        );
        store_forward_batch(&fixture.owner, &text_only).expect("text-only old batch remains usable");
        assert_eq!(load_forward_batches(&fixture.owner, "source").unwrap().len(), 2);
    }

    #[test]
    fn forward_nonce_is_account_issuer_and_destination_scoped() {
        let a = forward_destination_nonce("a", "prod", "request", "b");
        assert_eq!(a, forward_destination_nonce("a", "prod", "request", "b"));
        assert_ne!(a, forward_destination_nonce("a", "isolated", "request", "b"));
        assert_ne!(a, forward_destination_nonce("b", "prod", "request", "b"));
        assert_ne!(a, forward_destination_nonce("a", "prod", "request", "c"));
    }


    #[test]
    fn stale_forward_metadata_observation_cannot_erase_other_batch() {
        let dir = std::env::temp_dir().join(format!("forward-metadata-cas-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("store.db");
        let first = SandAgentDb::open(&db_path, 2_000).unwrap();
        let second = SandAgentDb::open(&db_path, 2_000).unwrap();
        let make_batch = |request: &str| HumanForwardBatch::new(
            "a".into(), "prod".into(), request.into(), "s".into(), "e".into(),
            "text".into(), vec![],
            serde_json::json!({"batchVersion":1,"requestId":request,
                "sourceConversationId":"s","sourceEntryId":"e",
                "privacy":{"dropSenderNames":false,"dropCaptions":false}}),
            &["b".into(), "c".into()],
        );
        let mut first_batch = make_batch("first");
        let second_batch = make_batch("second");
        store_forward_batch(&first, &first_batch).unwrap();
        let stale = read_forward_journal_value(&first).unwrap();
        store_forward_batch(&second, &second_batch).unwrap();
        let committed = read_forward_journal_value(&second).unwrap();
        let committed_raw = second.read_kv("metadata").unwrap().unwrap();

        first_batch.destinations[0].completed = true;
        assert!(store_forward_batch_observed(&first, &first_batch, stale).is_err(),
            "a journal writer paused after its read must not erase another committed batch");
        assert_eq!(first.read_kv("metadata").unwrap().unwrap(), committed_raw);
        // Expected-value admission must precede even the no-change shortcut.
        assert!(!first.set_metadata_if_unchanged(
            FORWARD_BATCHES_KEY, None, committed.clone().unwrap()
        ).unwrap());
        assert_eq!(first.read_kv("metadata").unwrap().unwrap(), committed_raw);

        let observed = read_forward_journal_value(&first).unwrap();
        assert!(second.set_metadata("unrelated", serde_json::json!(5)).unwrap());
        store_forward_batch_observed(&first, &first_batch, observed).unwrap();
        assert_eq!(first.get_metadata("unrelated").unwrap(), Some(serde_json::json!(5)));
        assert_eq!(load_forward_batches(&second, "s").unwrap(), vec![first_batch, second_batch]);

        assert!(second.set_metadata(FORWARD_BATCHES_KEY, Value::Null).unwrap());
        let corrupt_raw = first.read_kv("metadata").unwrap().unwrap();
        assert!(store_forward_batch(&first, &make_batch("third")).is_err(),
            "explicit null is corrupt, not an absent journal");
        assert_eq!(first.read_kv("metadata").unwrap().unwrap(), corrupt_raw);
        assert_eq!(read_forward_journal_value(&first).unwrap(), Some(Value::Null));
        first.close(false);
        second.close(false);
        drop(first);
        drop(second);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn bad_forward_journal_versions_and_destinations_fail_closed() {
        let context = serde_json::json!({"batchVersion":1,"requestId":"r","sourceConversationId":"s","sourceEntryId":"e","privacy":{"dropSenderNames":false,"dropCaptions":false}});
        let batch = HumanForwardBatch::new("a".into(),"prod".into(),"r".into(),"s".into(),"e".into(),"text".into(),vec![],context,&["b".into(),"c".into()]);
        assert!(batch.validate("s").is_ok());
        let mut invalid = batch.clone();
        invalid.version = 2;
        assert!(invalid.validate("s").is_err());
        invalid = batch.clone();
        invalid.destinations[1].client_nonce = invalid.destinations[0].client_nonce.clone();
        assert!(invalid.validate("s").is_err());
        invalid = batch.clone();
        invalid.destinations[1].conversation_id = "b".into();
        assert!(invalid.validate("s").is_err());
    }
    #[test]
    fn stale_same_batch_snapshot_cannot_regress_completed_destination() {
        let dir = std::env::temp_dir().join(format!("forward-stale-progress-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("store.db");
        let first = SandAgentDb::open(&db_path, 500).unwrap();
        let second = SandAgentDb::open(&db_path, 500).unwrap();
        let context = serde_json::json!({"batchVersion":1,"requestId":"r","sourceConversationId":"s","sourceEntryId":"e","privacy":{"dropSenderNames":false,"dropCaptions":false}});
        let original = HumanForwardBatch::new("a".into(),"prod".into(),"r".into(),"s".into(),"e".into(),"text".into(),vec![],context,&["b".into(),"c".into()]);
        store_forward_batch(&first, &original).unwrap();
        let unrelated = serde_json::json!({"keep":"unchanged"});
        assert!(first.set_metadata("unrelated", unrelated.clone()).unwrap());
        let mut other_context = original.context.clone();
        other_context["requestId"] = serde_json::json!("other");
        other_context["sourceEntryId"] = serde_json::json!("other-entry");
        let mut other = HumanForwardBatch::new("a".into(),"prod".into(),"other".into(),"s".into(),"other-entry".into(),"unrelated text".into(),vec![],other_context,&["d".into()]);
        other.destinations[0].error = Some("unrelated retryable failure".into());
        store_forward_batch(&second, &other).unwrap();
        let mut settled = original.clone();
        settled.destinations[0].completed = true;
        store_forward_batch(&second, &settled).unwrap();
        let settled_raw = first.read_kv("metadata").unwrap().expect("committed metadata");
        let error = store_forward_batch(&first, &original).unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(first.read_kv("metadata").unwrap().unwrap(), settled_raw);
        assert_eq!(load_forward_batches(&first, "s").unwrap(), vec![settled.clone(), other.clone()]);
        // A stale whole-batch writer cannot trade back a completed first
        // target for progress on the second target. Reject all of that write.
        let mut mixed = original.clone();
        mixed.destinations[1].completed = true;
        let error = store_forward_batch(&first, &mixed).unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(second.read_kv("metadata").unwrap().unwrap(), settled_raw);
        assert_eq!(load_forward_batches(&second, "s").unwrap(), vec![settled.clone(), other.clone()]);
        // Fresh pending failures remain writable and recoverable while the
        // already completed target and the other valid batch stay intact.
        let mut fresh = load_forward_batches(&first, "s").unwrap().remove(0);
        fresh.destinations[1].error = Some("retryable second destination failure".into());
        store_forward_batch(&first, &fresh).unwrap();
        assert_eq!(load_forward_batches(&second, "s").unwrap(), vec![fresh.clone(), other.clone()]);
        let pending_second = fresh.clone();
        let mut fresh = load_forward_batches(&second, "s").unwrap().remove(0);
        fresh.destinations[1].completed = true;
        fresh.destinations[1].error = None;
        store_forward_batch(&second, &fresh).unwrap();
        assert_eq!(load_forward_batches(&first, "s").unwrap(), vec![fresh.clone(), other.clone()]);
        let terminal_raw = first.read_kv("metadata").unwrap().unwrap();
        let error = store_forward_batch(&first, &pending_second).unwrap_err();
        assert!(error.contains("stale"), "{error}");
        assert_eq!(first.read_kv("metadata").unwrap().unwrap(), terminal_raw);
        assert_eq!(load_forward_batches(&second, "s").unwrap(), vec![fresh.clone(), other.clone()]);
        store_forward_batch(&first, &fresh).expect("identical terminal replay stays idempotent");
        assert_eq!(second.read_kv("metadata").unwrap().unwrap(), terminal_raw);
        first.close(false);
        second.close(false);
        drop(first); drop(second);
        let reopened = SandAgentDb::open(&db_path, 500).unwrap();
        assert_eq!(reopened.read_kv("metadata").unwrap().unwrap(), terminal_raw);
        assert_eq!(load_forward_batches(&reopened, "s").unwrap(), vec![fresh, other]);
        assert_eq!(reopened.get_metadata("unrelated").unwrap(), Some(unrelated));
        reopened.close(false); drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_forward_metadata_writers_have_one_durable_winner_and_recover() {
        use std::sync::{Arc, Barrier};
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn read_committed_metadata(path: &Path) -> (String, Value) {
            let db = rusqlite::Connection::open_with_flags(
                path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            ).expect("independent read-only SQLite connection");
            let raw: String = db.query_row(
                "SELECT value FROM kv WHERE key = 'metadata'", [], |row| row.get(0),
            ).expect("committed metadata row");
            assert_eq!(raw.len() % 2, 0);
            let bytes = (0..raw.len()).step_by(2).map(|offset|
                u8::from_str_radix(&raw[offset..offset + 2], 16).expect("metadata hex")
            ).collect::<Vec<_>>();
            let metadata = serde_json::from_slice(&bytes).expect("committed metadata JSON");
            (raw, metadata)
        }

        let dir = std::env::temp_dir().join(format!("forward-parallel-cas-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("store.db");
        let seed = SandAgentDb::open(&path, 5000).unwrap();
        assert!(seed.set_metadata("unrelated", serde_json::json!(19)).unwrap());
        seed.close(false); drop(seed);

        let busy_errors = Arc::new(AtomicUsize::new(0));
        let observed_busy_errors = Arc::clone(&busy_errors);
        let mut options = super::super::agent_db::SandAgentDbOptions::default();
        options.recovery.busy_timeout_ms = 5000;
        options.on_busy_error = Some(Arc::new(move |_, _| {
            observed_busy_errors.fetch_add(1, Ordering::SeqCst);
        }));
        let batches = (0..2).map(|index| {
            let request_id = index.to_string();
            let context = serde_json::json!({
                "batchVersion": 1, "requestId": request_id,
                "sourceConversationId": "s", "sourceEntryId": "e",
                "privacy": {"dropSenderNames": false, "dropCaptions": false},
            });
            HumanForwardBatch::new(
                "a".into(), "prod".into(), request_id, "s".into(), "e".into(),
                "text".into(), vec![], context, &["b".into(), "c".into()],
            )
        }).collect::<Vec<_>>();
        // Finish fallible owner setup, stale observations and encoding before
        // spawning either participant, so setup failure cannot strand a waiter.
        let prepared = batches.iter().map(|batch| {
            let owner = SandAgentDb::open_with_options(&path, options.clone()).unwrap();
            let observed = owner.get_metadata(FORWARD_BATCHES_KEY).unwrap();
            assert!(observed.is_none());
            let value = serde_json::to_value(vec![batch]).unwrap();
            (owner, observed, value)
        }).collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(3));
        let threads = prepared.into_iter().map(|(owner, observed, value)| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let result = owner.set_metadata_if_unchanged(
                    FORWARD_BATCHES_KEY, observed.as_ref(), value,
                );
                owner.close(false);
                result
            })
        }).collect::<Vec<_>>();
        barrier.wait();
        let outcomes = threads.into_iter().map(|thread| thread.join().unwrap()).collect::<Vec<_>>();
        assert_eq!(busy_errors.load(Ordering::SeqCst), 0,
            "SQLite busy/locked must not masquerade as a CAS loser");
        assert!(outcomes.iter().all(Result::is_ok), "SQLite write errors are not a CAS rejection: {outcomes:?}");
        assert_eq!(outcomes.iter().filter(|item| *item.as_ref().unwrap()).count(), 1);
        let reopened = SandAgentDb::open_with_options(&path, options.clone()).unwrap();
        let winner = reopened.get_metadata(FORWARD_BATCHES_KEY).unwrap().unwrap();
        let winner_id = winner[0]["requestId"].as_str().unwrap();
        assert!(winner_id == "0" || winner_id == "1");
        let winner_index: usize = winner_id.parse().unwrap();
        assert!(*outcomes[winner_index].as_ref().unwrap());
        assert_eq!(winner, serde_json::to_value(vec![batches[winner_index].clone()]).unwrap());
        assert_eq!(reopened.get_metadata("unrelated").unwrap(), Some(serde_json::json!(19)));
        let (winner_raw, metadata) = read_committed_metadata(&path);
        assert_eq!(metadata[FORWARD_BATCHES_KEY], winner);
        assert_eq!(metadata["unrelated"], serde_json::json!(19));
        assert_eq!(load_forward_batches(&reopened, "s").unwrap(), vec![batches[winner_index].clone()]);

        let loser = &batches[1 - winner_index];
        assert!(!reopened.set_metadata_if_unchanged(
            FORWARD_BATCHES_KEY, None, serde_json::to_value(vec![loser]).unwrap(),
        ).expect("old observation remains rejected after reopen"));
        assert_eq!(read_committed_metadata(&path).0, winner_raw,
            "stale loser must preserve the complete committed metadata row");
        store_forward_batch(&reopened, loser).expect("fresh journal retry recovers the losing batch");
        let recovered = load_forward_batches(&reopened, "s").unwrap();
        assert_eq!(recovered.len(), 2);
        for batch in &batches {
            assert!(recovered.contains(batch), "both immutable intents must survive recovery");
        }
        assert_eq!(busy_errors.load(Ordering::SeqCst), 0,
            "recovery controls must not be explained by a SQLite lock error");
        reopened.close(false); drop(reopened);
        let (recovered_raw, metadata) = read_committed_metadata(&path);
        assert_eq!(metadata[FORWARD_BATCHES_KEY], serde_json::to_value(&recovered).unwrap());
        assert_eq!(metadata["unrelated"], serde_json::json!(19));
        let restored = SandAgentDb::open(&path, 5000).unwrap();
        assert_eq!(load_forward_batches(&restored, "s").unwrap(), recovered);
        assert_eq!(restored.get_metadata("unrelated").unwrap(), Some(serde_json::json!(19)));
        assert_eq!(read_committed_metadata(&path).0, recovered_raw);
        restored.close(false); drop(restored);
        std::fs::remove_dir_all(dir).unwrap();
    }

}
