use crate::conversation_blob_gc::to_hex_id;
use crate::conversation_state_binary::decode_transcript_mirror_conversation_state;
use crate::legacy_transcript_mirror::LegacyFileTranscriptMirror;
use crate::transcript_mirror_offload::TranscriptMirrorJob;
use crate::transcript_occurrence_deriver::TranscriptOccurrenceBlobStore;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_MIRROR_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingTranscriptStateError {
    pub conversation_id: String,
}

impl fmt::Display for MissingTranscriptStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "transcript mirror checkpoint blob is unavailable for {}",
            self.conversation_id
        )
    }
}

impl std::error::Error for MissingTranscriptStateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOnlyTranscriptBlobStoreWriteError;

impl fmt::Display for ReadOnlyTranscriptBlobStoreWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("transcript mirror reader never writes conversation blobs")
    }
}

impl std::error::Error for ReadOnlyTranscriptBlobStoreWriteError {}

/// Lifecycle-safe iOS replacement for Grok's transcript-mirror worker DB reader.
///
/// The reference worker keeps read-only SQLite connections in a worker_thread.
/// iOS opens each candidate DB on demand instead, so suspension or scene recovery
/// never relies on a long-lived background thread/process. Lookup order and
/// read-only semantics are preserved.
#[derive(Debug, Clone)]
pub struct ReadOnlySqliteBlobStore {
    db_paths: Vec<PathBuf>,
    busy_timeout: Duration,
}

impl ReadOnlySqliteBlobStore {
    pub fn new<I, P>(db_paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            db_paths: db_paths.into_iter().map(Into::into).collect(),
            busy_timeout: Duration::from_millis(DEFAULT_MIRROR_BUSY_TIMEOUT_MS),
        }
    }

    pub fn get_blob(&self, blob_id: &[u8]) -> Option<Vec<u8>> {
        let key = to_hex_id(blob_id);
        for db_path in &self.db_paths {
            if !db_path.is_file() {
                continue;
            }
            let Ok(connection) =
                Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            else {
                continue;
            };
            let _ = connection.busy_timeout(self.busy_timeout);
            let row = connection
                .query_row(
                    "SELECT data FROM blobs WHERE id = ?1",
                    [key.as_str()],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional();
            if let Ok(Some(data)) = row {
                return Some(data);
            }
        }
        None
    }

    pub fn load_state_blob(
        &self,
        conversation_id: &str,
        state_blob_id: &[u8],
    ) -> Result<Vec<u8>, MissingTranscriptStateError> {
        self.get_blob(state_blob_id)
            .ok_or_else(|| MissingTranscriptStateError {
                conversation_id: conversation_id.to_owned(),
            })
    }

    pub fn reject_write(&self) -> Result<(), ReadOnlyTranscriptBlobStoreWriteError> {
        Err(ReadOnlyTranscriptBlobStoreWriteError)
    }
}

impl TranscriptOccurrenceBlobStore for ReadOnlySqliteBlobStore {
    fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
        ReadOnlySqliteBlobStore::get_blob(self, id)
    }
}

#[derive(Debug)]
pub enum TranscriptMirrorExecutionError<WriterError> {
    MissingState(MissingTranscriptStateError),
    Writer(WriterError),
}

/// Executes one already-scheduled mirror job.
///
/// Decoding the conversation state and materializing the legacy/full transcript
/// remain Host responsibilities and are injected as write_full. This keeps the
/// storage worker independent of generated protobuf code while preserving the
/// Grok boundary: checkpoint load happens from read-only blob DBs and one full
/// mirror write settles the dispatch.
pub fn execute_mirror_job<Writer, WriterError>(
    job: &TranscriptMirrorJob,
    write_full: Writer,
) -> Result<bool, TranscriptMirrorExecutionError<WriterError>>
where
    Writer: FnOnce(
        &[u8],
        &ReadOnlySqliteBlobStore,
        &Path,
        &str,
    ) -> Result<bool, WriterError>,
{
    let store = ReadOnlySqliteBlobStore::new(job.blob_db_paths.iter().cloned());
    let state = store
        .load_state_blob(&job.conversation_id, &job.state_blob_id)
        .map_err(TranscriptMirrorExecutionError::MissingState)?;
    write_full(
        &state,
        &store,
        &job.transcripts_dir,
        &job.conversation_id,
    )
    .map_err(TranscriptMirrorExecutionError::Writer)
}

#[derive(Debug)]
pub enum LegacyTranscriptMirrorExecutionError {
    MissingState(MissingTranscriptStateError),
    Decode(String),
    Writer(String),
}

impl fmt::Display for LegacyTranscriptMirrorExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingState(error) => error.fmt(formatter),
            Self::Decode(error) | Self::Writer(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for LegacyTranscriptMirrorExecutionError {}

pub fn execute_legacy_mirror_job(
    job: &TranscriptMirrorJob,
) -> Result<bool, LegacyTranscriptMirrorExecutionError> {
    let store = ReadOnlySqliteBlobStore::new(job.blob_db_paths.iter().cloned());
    let state_blob = store
        .load_state_blob(&job.conversation_id, &job.state_blob_id)
        .map_err(LegacyTranscriptMirrorExecutionError::MissingState)?;
    let state = decode_transcript_mirror_conversation_state(&state_blob)
        .map_err(|error| LegacyTranscriptMirrorExecutionError::Decode(error.to_string()))?;
    LegacyFileTranscriptMirror::new(&job.transcripts_dir)
        .write_full(&job.conversation_id, &state, &store)
        .map_err(|error| LegacyTranscriptMirrorExecutionError::Writer(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-transcript-mirror-worker-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn create_blob_db(path: &Path, blob_id: &[u8], data: &[u8]) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params![to_hex_id(blob_id), data],
            )
            .unwrap();
    }

    #[test]
    fn reads_first_available_checkpoint_without_write_capability() {
        let dir = temp_dir();
        let db = dir.join("conversation.sqlite");
        create_blob_db(&db, &[1, 2, 3], b"state");
        let store =
            ReadOnlySqliteBlobStore::new([dir.join("missing.sqlite"), db]);
        assert_eq!(
            store.get_blob(&[1, 2, 3]).as_deref(),
            Some(b"state".as_slice())
        );
        assert_eq!(
            store.reject_write().unwrap_err(),
            ReadOnlyTranscriptBlobStoreWriteError
        );
    }

    fn bytes_field(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut tag = field * 8 + 2;
        loop {
            let mut byte = (tag & 0x7f) as u8;
            tag >>= 7;
            if tag != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if tag == 0 {
                break;
            }
        }
        let mut length = value.len() as u64;
        loop {
            let mut byte = (length & 0x7f) as u8;
            length >>= 7;
            if length != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if length == 0 {
                break;
            }
        }
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn production_legacy_executor_decodes_checkpoint_and_writes_jsonl() {
        let dir = temp_dir();
        let db = dir.join("conversation.sqlite");
        let state_id = [9u8];
        let root_id = [7u8];
        create_blob_db(&db, &state_id, &bytes_field(1, &root_id));
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params![
                    to_hex_id(&root_id),
                    br#"{"role":"user","content":"hello from checkpoint"}"#.as_slice()
                ],
            )
            .unwrap();

        let job = TranscriptMirrorJob {
            conversation_id: "conversation-a".into(),
            state_blob_id: state_id.to_vec(),
            blob_db_paths: vec![db],
            transcripts_dir: dir.join("transcripts"),
        };
        assert!(execute_legacy_mirror_job(&job).unwrap());
        let jsonl = fs::read_to_string(
            job.transcripts_dir
                .join("conversation-a")
                .join("conversation-a.jsonl"),
        )
        .unwrap();
        assert!(jsonl.contains("hello from checkpoint"));
    }

    #[test]
    fn missing_checkpoint_is_explicit_and_full_writer_receives_loaded_state() {
        let dir = temp_dir();
        let db = dir.join("conversation.sqlite");
        create_blob_db(&db, &[9], b"checkpoint");

        let missing = ReadOnlySqliteBlobStore::new([db.clone()])
            .load_state_blob("missing-conversation", &[8])
            .unwrap_err();
        assert_eq!(missing.conversation_id, "missing-conversation");

        let job = TranscriptMirrorJob {
            conversation_id: "conversation-a".into(),
            state_blob_id: vec![9],
            blob_db_paths: vec![db],
            transcripts_dir: dir.join("transcripts"),
        };
        let written = execute_mirror_job(&job, |state, store, transcripts_dir, id| {
            assert_eq!(state, b"checkpoint");
            assert!(store.reject_write().is_err());
            assert_eq!(transcripts_dir, job.transcripts_dir.as_path());
            assert_eq!(id, "conversation-a");
            Ok::<bool, ()>(true)
        })
        .unwrap();
        assert!(written);
    }
}
