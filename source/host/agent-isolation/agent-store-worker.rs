use crate::conversation_blob_gc::BlobReferenceDecoder;
use crate::conversation_blob_store::{ConversationBlobStoreDb, GarbageCollectionOutcome};
use crate::conversation_blob_db::ConversationBlobRecoveryError;
use crate::legacy_blob_retirement::LegacyBlobRetirementVerdict;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// iOS-native replacement for Grok's Node worker-thread store worker.
///
/// iOS does not spawn a JavaScript worker process for blob persistence. Instead,
/// each agent/blob database gets a serialized Rust lane owned by the Host. The
/// lane preserves the same store isolation and lifecycle responsibilities while
/// remaining compatible with iOS background suspension and process relaunch.
#[derive(Debug)]
pub struct AgentStoreWorker {
    agent_id: String,
    blob_db_path: PathBuf,
    legacy_blob_db_path: Option<PathBuf>,
    store: ConversationBlobStoreDb,
    last_activity_at: Instant,
    closed: bool,
}

impl AgentStoreWorker {
    pub fn open(
        agent_id: impl Into<String>,
        blob_db_path: impl Into<PathBuf>,
        busy_timeout_ms: u64,
        legacy_blob_db_path: Option<PathBuf>,
    ) -> Result<Self, ConversationBlobRecoveryError> {
        let agent_id = agent_id.into();
        let blob_db_path = blob_db_path.into();
        let store = ConversationBlobStoreDb::open(
            agent_id.clone(),
            blob_db_path.clone(),
            busy_timeout_ms,
            legacy_blob_db_path.clone(),
        )?;
        Ok(Self {
            agent_id,
            blob_db_path,
            legacy_blob_db_path,
            store,
            last_activity_at: Instant::now(),
            closed: false,
        })
    }

    fn ensure_open(&self) -> Result<(), ConversationBlobRecoveryError> {
        if self.closed {
            Err(ConversationBlobRecoveryError::new(
                "SAND_AGENT_STORE_WORKER_CLOSED",
                "agent store worker lane is closed",
            ))
        } else {
            Ok(())
        }
    }

    fn touch(&mut self) {
        self.last_activity_at = Instant::now();
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn blob_db_path(&self) -> &Path {
        &self.blob_db_path
    }

    pub fn legacy_blob_db_path(&self) -> Option<&Path> {
        self.legacy_blob_db_path.as_deref()
    }

    pub fn idle_for(&self) -> std::time::Duration {
        self.last_activity_at.elapsed()
    }

    pub fn get_blob(
        &mut self,
        blob_id: &[u8],
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.get_blob(blob_id)
    }

    pub fn set_blob(
        &mut self,
        blob_id: &[u8],
        blob_data: &[u8],
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.set_blob(blob_id, blob_data)
    }

    pub fn find_latest_root_blob_id(
        &mut self,
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.find_latest_root_blob_id()
    }

    pub fn mark_checkpoint_root(
        &mut self,
        blob_id: &[u8],
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.mark_checkpoint_root(blob_id)
    }

    pub fn clear_blobs(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.clear_blobs()
    }

    pub fn clear_stale_checkpoint_roots(
        &mut self,
        retained_root_id_hex: &str,
    ) -> Result<usize, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store
            .clear_stale_checkpoint_roots(retained_root_id_hex)
    }

    pub fn collect_conversation_garbage<Decoder>(
        &mut self,
        retained_root_id_hex: &str,
        pending_write_retention_ms: u64,
        decoder: &Decoder,
    ) -> Result<GarbageCollectionOutcome, ConversationBlobRecoveryError>
    where
        Decoder: BlobReferenceDecoder,
    {
        self.ensure_open()?;
        self.touch();
        self.store.collect_garbage(
            retained_root_id_hex,
            pending_write_retention_ms,
            decoder,
        )
    }

    pub fn verify_legacy_blob_retirement(
        &mut self,
        retained_root_id_hex: &str,
        legacy_blob_db_path: &Path,
    ) -> Result<LegacyBlobRetirementVerdict, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store
            .verify_legacy_blob_retirement(retained_root_id_hex, legacy_blob_db_path)
    }

    pub fn flush(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.touch();
        self.store.flush()
    }

    pub fn close(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        if !self.closed {
            self.store.close()?;
            self.closed = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-ios-agent-store-worker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root.join("conversation-blobs.sqlite")
    }

    #[test]
    fn serial_lane_persists_agent_scoped_blob_operations() {
        let mut worker = AgentStoreWorker::open("agent-a", temp_path(), 500, None).unwrap();
        worker.set_blob(&[1, 2], b"payload").unwrap();
        assert_eq!(
            worker.get_blob(&[1, 2]).unwrap().as_deref(),
            Some(b"payload".as_slice())
        );
        assert_eq!(worker.agent_id(), "agent-a");
        worker.flush().unwrap();
        worker.close().unwrap();
        assert_eq!(
            worker.get_blob(&[1, 2]).unwrap_err().code,
            "SAND_AGENT_STORE_WORKER_CLOSED"
        );
    }
}
