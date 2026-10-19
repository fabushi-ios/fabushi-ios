use crate::agent_worker_pool::AgentWorkerPool;
use crate::conversation_blob_gc::to_hex_id;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RECOVERY_AGENT_ID: &str = "mobile-canonical-conversation-recovery";
const RECOVERY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileConversationPersistenceLease {
    pub account_scope: String,
    pub account_generation: String,
}

impl MobileConversationPersistenceLease {
    pub fn new(
        account_scope: impl Into<String>,
        account_generation: impl Into<String>,
    ) -> Option<Self> {
        let account_scope = account_scope.into();
        let account_generation = account_generation.into();
        if !strict_digest(&account_scope) || !strict_digest(&account_generation) {
            return None;
        }
        Some(Self { account_scope, account_generation })
    }

    pub fn from_identity_value(value: &serde_json::Value) -> Option<Self> {
        Self::new(
            value.get("accountScope")?.as_str()?.to_string(),
            value.get("accountGeneration")?.as_str()?.to_string(),
        )
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MobileConversationRecoveryCheckpoint {
    schema_version: u32,
    account_scope: String,
    account_generation: String,
    transcript_sha256: String,
    transcript_base64: String,
}

pub struct MobileConversationPersistenceOwner {
    app_data_dir: PathBuf,
    pool: AgentWorkerPool,
    last_checkpoint: Option<(String, String, String)>,
}

impl MobileConversationPersistenceOwner {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            app_data_dir: app_data_dir.into(),
            pool: AgentWorkerPool::default(),
            last_checkpoint: None,
        }
    }

    pub fn recover_before_host(
        &mut self,
        lease: &MobileConversationPersistenceLease,
    ) -> Result<bool, String> {
        let canonical_path = self.canonical_transcript_path();
        let existing = match fs::read(&canonical_path) {
            Ok(bytes) if valid_transcript(&bytes) => return Ok(false),
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!(
                "read canonical conversation transcript {}: {error}",
                canonical_path.display()
            )),
        };

        let db_path = self.blob_db_path(lease);
        let Some(root_id) = self.pool
            .find_latest_root_blob_id(RECOVERY_AGENT_ID, &db_path, None)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        let Some(checkpoint_bytes) = self.pool
            .get_blob_sync(RECOVERY_AGENT_ID, &db_path, &root_id, None)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        let checkpoint: MobileConversationRecoveryCheckpoint =
            serde_json::from_slice(&checkpoint_bytes)
                .map_err(|error| format!("decode conversation recovery checkpoint: {error}"))?;
        if checkpoint.schema_version != RECOVERY_SCHEMA_VERSION
            || checkpoint.account_scope != lease.account_scope
            || checkpoint.account_generation != lease.account_generation
        {
            return Ok(false);
        }
        let transcript = STANDARD
            .decode(checkpoint.transcript_base64.as_bytes())
            .map_err(|error| format!("decode recovery transcript payload: {error}"))?;
        let digest = sha256_hex(&transcript);
        if digest != checkpoint.transcript_sha256 || !valid_transcript(&transcript) {
            return Ok(false);
        }

        let quarantine = existing.as_ref().map(|_| corrupt_path(&canonical_path));
        write_recovered_transcript(&canonical_path, &transcript, quarantine.as_deref())?;
        self.last_checkpoint = Some((
            lease.account_scope.clone(),
            lease.account_generation.clone(),
            digest,
        ));
        Ok(true)
    }

    pub fn checkpoint(
        &mut self,
        lease: &MobileConversationPersistenceLease,
    ) -> Result<bool, String> {
        let canonical_path = self.canonical_transcript_path();
        let transcript = match fs::read(&canonical_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!(
                "read canonical conversation transcript {}: {error}",
                canonical_path.display()
            )),
        };
        if !valid_transcript(&transcript) {
            return Err("canonical conversation transcript is not a valid JSON array".into());
        }
        let transcript_sha256 = sha256_hex(&transcript);
        if self.last_checkpoint.as_ref().is_some_and(|(scope, generation, digest)| {
            scope == &lease.account_scope
                && generation == &lease.account_generation
                && digest == &transcript_sha256
        }) {
            return Ok(false);
        }

        let checkpoint = MobileConversationRecoveryCheckpoint {
            schema_version: RECOVERY_SCHEMA_VERSION,
            account_scope: lease.account_scope.clone(),
            account_generation: lease.account_generation.clone(),
            transcript_sha256: transcript_sha256.clone(),
            transcript_base64: STANDARD.encode(&transcript),
        };
        let checkpoint_bytes = serde_json::to_vec(&checkpoint)
            .map_err(|error| format!("encode conversation recovery checkpoint: {error}"))?;
        let root_id = Sha256::digest(&checkpoint_bytes).to_vec();
        let db_path = self.blob_db_path(lease);
        self.pool
            .set_blob_sync(RECOVERY_AGENT_ID, &db_path, &root_id, &checkpoint_bytes, None)
            .map_err(|error| error.to_string())?;
        self.pool
            .mark_checkpoint_root(RECOVERY_AGENT_ID, &db_path, &root_id, None)
            .map_err(|error| error.to_string())?;
        self.pool
            .clear_stale_checkpoint_roots(
                RECOVERY_AGENT_ID,
                &db_path,
                &to_hex_id(&root_id),
                None,
            )
            .map_err(|error| error.to_string())?;
        self.pool
            .flush_store(&db_path)
            .map_err(|error| error.to_string())?;
        self.last_checkpoint = Some((
            lease.account_scope.clone(),
            lease.account_generation.clone(),
            transcript_sha256,
        ));
        Ok(true)
    }

    pub fn shutdown(
        &mut self,
        lease: Option<&MobileConversationPersistenceLease>,
    ) -> Result<(), String> {
        let checkpoint = lease.map(|lease| self.checkpoint(lease).map(|_| ())).transpose();
        let close = self.pool.close_all().map_err(|error| error.to_string());
        match (checkpoint, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(_), Ok(())) => Ok(()),
        }
    }

    fn canonical_transcript_path(&self) -> PathBuf {
        self.app_data_dir
            .join("feature-host")
            .join("runtime")
            .join("provider-neutral-assistant-transcript.json")
    }

    fn blob_db_path(&self, lease: &MobileConversationPersistenceLease) -> PathBuf {
        self.app_data_dir
            .join("feature-host")
            .join("runtime")
            .join("conversation-recovery")
            .join("accounts")
            .join(&lease.account_scope)
            .join("conversation-blobs.db")
    }
}

impl Drop for MobileConversationPersistenceOwner {
    fn drop(&mut self) {
        let _ = self.pool.close_all();
    }
}

fn strict_digest(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| {
            byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
        })
}

fn valid_transcript(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .is_some_and(|value| value.is_array())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn corrupt_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("json.corrupt-{stamp}"))
}

fn write_recovered_transcript(
    path: &Path,
    bytes: &[u8],
    quarantine: Option<&Path>,
) -> Result<(), String> {
    let parent = path.parent()
        .ok_or_else(|| "canonical transcript path has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create canonical transcript parent: {error}"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("json.recovery-{stamp}.tmp"));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)
        .map_err(|error| format!("create recovery transcript temporary file: {error}"))?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write recovery transcript temporary file: {error}"));
    }

    if let Some(quarantine) = quarantine {
        if let Err(error) = fs::rename(path, quarantine) {
            let _ = fs::remove_file(&temporary);
            return Err(format!("quarantine corrupt canonical transcript: {error}"));
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        if let Some(quarantine) = quarantine {
            let _ = fs::rename(quarantine, path);
        }
        return Err(format!("install recovered canonical transcript: {error}"));
    }
    Ok(())
}
