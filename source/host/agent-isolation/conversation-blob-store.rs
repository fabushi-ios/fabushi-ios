use crate::conversation_blob_db::{
    ConversationBlobDb, ConversationBlobMigrationState, ConversationBlobRecoveryError,
};
use crate::conversation_blob_gc::{
    BlobReferenceDecoder, collect_reachable_blob_hex_ids, from_hex_id, to_hex_id,
};
use crate::legacy_blob_retirement::{
    LegacyBlobRetirementVerdict, verify_legacy_blob_retirement,
};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_TRACKED_RECENT_WRITES: usize = 16_384;
const VACUUM_MIN_DELETED_BYTES: u64 = 64 * 1024 * 1024;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GarbageCollectionOutcome {
    SkippedNoRoot,
    SkippedRootUndecodable,
    SkippedUnresolvedRefs { unresolved_refs: usize },
    Collected {
        deleted_rows: usize,
        deleted_bytes: u64,
        live_rows: usize,
        live_bytes: u64,
        retained_pending_rows: usize,
        vacuumed: bool,
    },
}

#[derive(Debug)]
pub struct ConversationBlobStoreDb {
    pub agent_id: String,
    pub blob_db_path: PathBuf,
    pub legacy_blob_db_path: Option<PathBuf>,
    pub db: ConversationBlobDb,
    recent_write_ms_by_hex_id: HashMap<String, u64>,
    closed: bool,
}

impl ConversationBlobStoreDb {
    pub fn open(
        agent_id: impl Into<String>,
        blob_db_path: impl Into<PathBuf>,
        busy_timeout_ms: u64,
        legacy_blob_db_path: Option<PathBuf>,
    ) -> Result<Self, ConversationBlobRecoveryError> {
        let blob_db_path = blob_db_path.into();
        let db = ConversationBlobDb::open(&blob_db_path, busy_timeout_ms)?;
        let mut store = Self {
            agent_id: agent_id.into(),
            blob_db_path,
            legacy_blob_db_path,
            db,
            recent_write_ms_by_hex_id: HashMap::new(),
            closed: false,
        };
        store.adopt_legacy_blobs()?;
        Ok(store)
    }

    fn adopt_legacy_blobs(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        if !matches!(
            self.db.migration_state(),
            ConversationBlobMigrationState::Unstarted
                | ConversationBlobMigrationState::RecoveryRebuilt
        ) {
            return Ok(());
        }
        let Some(path) = self.legacy_blob_db_path.as_ref() else {
            return Ok(());
        };
        if !path.exists() || path == &self.blob_db_path {
            return Ok(());
        }

        let legacy = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(connection) => connection,
            Err(_) => return Ok(()),
        };
        let mut statement = match legacy.prepare("SELECT id, data FROM blobs") {
            Ok(statement) => statement,
            Err(_) => return Ok(()),
        };
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;

        let tx = self.db.connection_mut().transaction()?;
        for row in rows.flatten() {
            tx.execute(
                "INSERT OR IGNORE INTO blobs (id, data) VALUES (?1, ?2)",
                params![row.0, row.1],
            )?;
        }
        tx.commit()?;
        self.db
            .set_migration_state(ConversationBlobMigrationState::AdoptionComplete)?;
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), ConversationBlobRecoveryError> {
        if self.closed {
            Err(ConversationBlobRecoveryError::new(
                "SAND_BLOB_STORE_CLOSED",
                "conversation blob store is closed",
            ))
        } else {
            Ok(())
        }
    }

    pub fn get_blob(
        &self,
        blob_id: &[u8],
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.db.get_blob_hex(&to_hex_id(blob_id))
    }

    pub fn set_blob(
        &mut self,
        blob_id: &[u8],
        blob_data: &[u8],
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        let id = to_hex_id(blob_id);
        self.db.set_blob_hex(&id, blob_data)?;
        self.recent_write_ms_by_hex_id.insert(id, now_ms());
        if self.recent_write_ms_by_hex_id.len() > MAX_TRACKED_RECENT_WRITES {
            if let Some(oldest) = self
                .recent_write_ms_by_hex_id
                .iter()
                .min_by_key(|(_, written)| *written)
                .map(|(id, _)| id.clone())
            {
                self.recent_write_ms_by_hex_id.remove(&oldest);
            }
        }
        Ok(())
    }

    pub fn clear_blobs(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.recent_write_ms_by_hex_id.clear();
        self.db.clear_blobs()
    }

    pub fn mark_checkpoint_root(
        &self,
        blob_id: &[u8],
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.db.mark_checkpoint_root(&to_hex_id(blob_id), now_ms())
    }

    pub fn find_latest_root_blob_id(
        &self,
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        Ok(self
            .db
            .latest_checkpoint_root()?
            .as_deref()
            .and_then(from_hex_id))
    }

    pub fn clear_stale_checkpoint_roots(
        &mut self,
        retained_root_id_hex: &str,
    ) -> Result<usize, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        let roots = self.db.list_checkpoint_roots()?;
        let mut deleted = 0usize;
        for root in roots {
            if root == retained_root_id_hex {
                continue;
            }
            if self.db.delete_blob_hex(&root)? {
                deleted += 1;
            }
            self.db.unmark_checkpoint_root(&root)?;
            self.recent_write_ms_by_hex_id.remove(&root);
        }
        Ok(deleted)
    }

    pub fn collect_garbage<Decoder>(
        &mut self,
        retained_root_id_hex: &str,
        pending_write_retention_ms: u64,
        decoder: &Decoder,
    ) -> Result<GarbageCollectionOutcome, ConversationBlobRecoveryError>
    where
        Decoder: BlobReferenceDecoder,
    {
        self.ensure_open()?;
        let Some(root) = self.db.get_blob_hex(retained_root_id_hex)? else {
            return Ok(GarbageCollectionOutcome::SkippedNoRoot);
        };
        let walk = match collect_reachable_blob_hex_ids(&root, decoder, |id| {
            self.db.get_blob_hex(id).ok().flatten()
        }) {
            Ok(value) => value,
            Err(_) => return Ok(GarbageCollectionOutcome::SkippedRootUndecodable),
        };
        if walk.unresolved_refs > 0 {
            return Ok(GarbageCollectionOutcome::SkippedUnresolvedRefs {
                unresolved_refs: walk.unresolved_refs,
            });
        }

        let floor = now_ms().saturating_sub(pending_write_retention_ms);
        let mut deletable = Vec::new();
        let mut live_rows = 0usize;
        let mut retained_pending_rows = 0usize;
        let mut deleted_bytes = 0u64;
        let mut live_bytes = 0u64;

        for row in self.db.list_blob_index()? {
            if row.id == retained_root_id_hex || walk.reachable_hex_ids.contains(&row.id) {
                live_rows += 1;
                live_bytes = live_bytes.saturating_add(row.len);
                continue;
            }
            if self
                .recent_write_ms_by_hex_id
                .get(&row.id)
                .is_some_and(|written| *written > floor)
            {
                retained_pending_rows += 1;
                live_rows += 1;
                live_bytes = live_bytes.saturating_add(row.len);
                continue;
            }
            deleted_bytes = deleted_bytes.saturating_add(row.len);
            deletable.push(row.id);
        }

        if !deletable.is_empty() {
            let tx = self.db.connection_mut().transaction()?;
            for id in &deletable {
                tx.execute("DELETE FROM blobs WHERE id = ?1", [id])?;
            }
            tx.commit()?;
            for id in &deletable {
                self.recent_write_ms_by_hex_id.remove(id);
            }
        }

        let total_bytes = deleted_bytes.saturating_add(live_bytes);
        let vacuumed = deleted_bytes >= VACUUM_MIN_DELETED_BYTES
            || (deleted_bytes > 0
                && total_bytes > 0
                && deleted_bytes.saturating_mul(8) >= total_bytes);
        if vacuumed {
            self.db.connection().execute_batch("VACUUM")?;
        }
        self.db.checkpoint()?;

        Ok(GarbageCollectionOutcome::Collected {
            deleted_rows: deletable.len(),
            deleted_bytes,
            live_rows,
            live_bytes,
            retained_pending_rows,
            vacuumed,
        })
    }

    pub fn verify_legacy_blob_retirement(
        &self,
        retained_root_id_hex: &str,
        legacy_blob_db_path: &Path,
    ) -> Result<LegacyBlobRetirementVerdict, ConversationBlobRecoveryError> {
        self.ensure_open()?;
        verify_legacy_blob_retirement(&self.db, legacy_blob_db_path, retained_root_id_hex)
    }

    pub fn flush(&self) -> Result<(), ConversationBlobRecoveryError> {
        self.ensure_open()?;
        self.db.checkpoint()
    }

    pub fn close(&mut self) -> Result<(), ConversationBlobRecoveryError> {
        if !self.closed {
            self.db.checkpoint()?;
            self.closed = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_blob_gc::BlobReferenceDecoder;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FixedWidthDecoder;
    impl BlobReferenceDecoder for FixedWidthDecoder {
        type Error = &'static str;

        fn decode_blob_references(&self, blob: &[u8]) -> Result<Vec<Vec<u8>>, Self::Error> {
            if blob.is_empty() {
                return Ok(Vec::new());
            }
            if blob.len() % 32 != 0 {
                return Err("invalid reference stream");
            }
            Ok(blob.chunks(32).map(|chunk| chunk.to_vec()).collect())
        }
    }

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-ios-blob-store-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn adopts_legacy_blob_rows_and_marks_adoption_complete() {
        let dir = temp_dir();
        let legacy_path = dir.join("legacy.sqlite");
        {
            let legacy = ConversationBlobDb::open(&legacy_path, 500).unwrap();
            legacy.set_blob_hex(&to_hex_id(&[7; 32]), b"old").unwrap();
        }

        let store = ConversationBlobStoreDb::open(
            "agent-a",
            dir.join("new.sqlite"),
            500,
            Some(legacy_path),
        )
        .unwrap();
        assert_eq!(
            store.get_blob(&[7; 32]).unwrap().as_deref(),
            Some(b"old".as_slice())
        );
        assert_eq!(
            store.db.migration_state(),
            ConversationBlobMigrationState::AdoptionComplete
        );
    }

    #[test]
    fn garbage_collection_keeps_root_and_reachable_blobs() {
        let dir = temp_dir();
        let mut store =
            ConversationBlobStoreDb::open("agent-a", dir.join("db.sqlite"), 500, None).unwrap();
        let root_id = [1u8; 32];
        let child_id = [2u8; 32];
        let orphan_id = [3u8; 32];
        store.set_blob(&root_id, &child_id).unwrap();
        store.set_blob(&child_id, b"").unwrap();
        store.set_blob(&orphan_id, b"orphan").unwrap();
        store.mark_checkpoint_root(&root_id).unwrap();

        let root_hex = to_hex_id(&root_id);
        let outcome = store
            .collect_garbage(&root_hex, 0, &FixedWidthDecoder)
            .unwrap();
        assert!(matches!(
            outcome,
            GarbageCollectionOutcome::Collected { deleted_rows: 1, .. }
        ));
        assert_eq!(store.get_blob(&child_id).unwrap().as_deref(), Some(&[][..]));
        assert!(store.get_blob(&orphan_id).unwrap().is_none());
        assert_eq!(
            store.find_latest_root_blob_id().unwrap().as_deref(),
            Some(root_id.as_slice())
        );
    }
}
