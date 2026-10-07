use crate::transcript_journal_codec::{
    DeferredTranscriptStep, PendingTranscriptCheckpoint, TranscriptCheckpoint,
    TranscriptJournalCorruptionError, checkpoint_identity, file_identity, parse_deferred_step,
    parse_pending_checkpoint, pending_checkpoint_json,
};
use crate::transcript_mirror_router::TranscriptJournalPort;
use crate::transcript_occurrence_deriver::{
    TranscriptDeriver, TranscriptOccurrenceBlobStore,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq)]
pub struct JournalOutcome {
    pub op: &'static str,
    pub outcome: &'static str,
    pub conversation_id: String,
    pub entry_count: Option<usize>,
    pub bytes: Option<u64>,
    pub duration_ms: f64,
    pub cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileState {
    bytes: u64,
    device: String,
    inode: String,
}

type OutcomeReporter = Arc<dyn Fn(JournalOutcome) + Send + Sync>;

fn report_operation<T>(
    reporter: &OutcomeReporter,
    op: &'static str,
    conversation_id: &str,
    operation: impl FnOnce() -> Result<(T, Option<usize>, Option<u64>), TranscriptJournalCorruptionError>,
) -> Result<T, TranscriptJournalCorruptionError> {
    let started = Instant::now();
    match operation() {
        Ok((value, entry_count, bytes)) => {
            reporter(JournalOutcome {
                op,
                outcome: "ok",
                conversation_id: conversation_id.to_owned(),
                entry_count,
                bytes,
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
                cause: None,
            });
            Ok(value)
        }
        Err(error) => {
            reporter(JournalOutcome {
                op,
                outcome: "failed",
                conversation_id: conversation_id.to_owned(),
                entry_count: None,
                bytes: None,
                duration_ms: started.elapsed().as_secs_f64() * 1000.0,
                cause: Some(error.to_string()),
            });
            Err(error)
        }
    }
}

fn corruption(message: impl Into<String>) -> TranscriptJournalCorruptionError {
    TranscriptJournalCorruptionError(message.into())
}

fn io_corruption(context: &str, error: std::io::Error) -> TranscriptJournalCorruptionError {
    corruption(format!("{context}: {error}"))
}

fn safe_id(id: &str) -> Result<&str, TranscriptJournalCorruptionError> {
    if id != "."
        && id != ".."
        && !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(id)
    } else {
        Err(corruption("unsafe conversation id"))
    }
}

fn sync_parent(path: &Path) -> Result<(), TranscriptJournalCorruptionError> {
    let parent = path
        .parent()
        .ok_or_else(|| corruption("transcript path has no parent"))?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_corruption("sync transcript parent", error))
}

fn install_atomic(path: &Path, bytes: &[u8]) -> Result<(), TranscriptJournalCorruptionError> {
    let parent = path
        .parent()
        .ok_or_else(|| corruption("transcript path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| io_corruption("create transcript parent", error))?;

    for _ in 0..16 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".transcript.{}.{}.part",
            std::process::id(),
            sequence
        ));
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_corruption("create transcript temporary", error)),
        };

        let result = (|| {
            file.write_all(bytes)
                .map_err(|error| io_corruption("write transcript temporary", error))?;
            file.sync_all()
                .map_err(|error| io_corruption("sync transcript temporary", error))?;
            drop(file);
            fs::rename(&temporary, path)
                .map_err(|error| io_corruption("install transcript file", error))?;
            sync_parent(path)
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }

    Err(corruption(
        "could not allocate a unique transcript temporary file",
    ))
}

fn remove_durable_file(path: &Path) -> Result<(), TranscriptJournalCorruptionError> {
    match fs::remove_file(path) {
        Ok(()) => sync_parent(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_corruption("remove transcript journal file", error)),
    }
}

pub struct FileTranscriptMirror<Deriver> {
    transcripts_dir: PathBuf,
    report_outcome: OutcomeReporter,
    deriver: Deriver,
    states: HashMap<String, FileState>,
    durable_checkpoints: HashMap<String, TranscriptCheckpoint>,
    prepared_checkpoints: HashMap<String, TranscriptCheckpoint>,
    deferred_steps: HashMap<String, DeferredTranscriptStep>,
    prepared_deferred_steps: HashMap<String, Option<DeferredTranscriptStep>>,
}

impl<Deriver> FileTranscriptMirror<Deriver> {
    pub fn new(
        transcripts_dir: impl Into<PathBuf>,
        deriver: Deriver,
        report_outcome: impl Fn(JournalOutcome) + Send + Sync + 'static,
    ) -> Self {
        Self {
            transcripts_dir: transcripts_dir.into(),
            report_outcome: Arc::new(report_outcome),
            deriver,
            states: HashMap::new(),
            durable_checkpoints: HashMap::new(),
            prepared_checkpoints: HashMap::new(),
            deferred_steps: HashMap::new(),
            prepared_deferred_steps: HashMap::new(),
        }
    }

    pub fn without_reporting(transcripts_dir: impl Into<PathBuf>, deriver: Deriver) -> Self {
        Self::new(transcripts_dir, deriver, |_| {})
    }

    pub fn jsonl_path_for(
        &self,
        conversation_id: &str,
    ) -> Result<PathBuf, TranscriptJournalCorruptionError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.jsonl")))
    }

    pub fn pending_path_for(
        &self,
        conversation_id: &str,
    ) -> Result<PathBuf, TranscriptJournalCorruptionError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-pending.json")))
    }

    pub fn cursor_path_for(
        &self,
        conversation_id: &str,
    ) -> Result<PathBuf, TranscriptJournalCorruptionError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-cursor.json")))
    }

    pub fn mode_path_for(
        &self,
        conversation_id: &str,
    ) -> Result<PathBuf, TranscriptJournalCorruptionError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-mode")))
    }

    pub fn owns_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<bool, TranscriptJournalCorruptionError> {
        let path = self.mode_path_for(conversation_id)?;
        match fs::metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_corruption("read transcript mode marker", error)),
        }
    }

    pub fn claim_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        if self.owns_conversation(conversation_id)? {
            return Ok(());
        }
        install_atomic(&self.mode_path_for(conversation_id)?, b"1\n")
    }

    fn read_pending(
        &self,
        conversation_id: &str,
    ) -> Result<Option<PendingTranscriptCheckpoint>, TranscriptJournalCorruptionError> {
        let path = self.pending_path_for(conversation_id)?;
        match fs::read_to_string(path) {
            Ok(raw) => parse_pending_checkpoint(&raw).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_corruption("read transcript pending WAL", error)),
        }
    }

    fn remove_pending(
        &self,
        conversation_id: &str,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        remove_durable_file(&self.pending_path_for(conversation_id)?)
    }

    fn read_deferred(
        &self,
        conversation_id: &str,
    ) -> Result<Option<DeferredTranscriptStep>, TranscriptJournalCorruptionError> {
        let path = self.cursor_path_for(conversation_id)?;
        match fs::read_to_string(path) {
            Ok(raw) => {
                let value: Value = serde_json::from_str(&raw)
                    .map_err(|_| corruption("transcript deferred cursor is invalid"))?;
                parse_deferred_step(&value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_corruption("read transcript deferred cursor", error)),
        }
    }

    fn write_deferred(
        &self,
        conversation_id: &str,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        let path = self.cursor_path_for(conversation_id)?;
        match deferred {
            Some(step) => install_atomic(
                &path,
                json!({
                    "turnIndex": step.turn_index,
                    "stepIndex": step.step_index
                })
                .to_string()
                .as_bytes(),
            ),
            None => remove_durable_file(&path),
        }
    }
}

impl<Deriver> FileTranscriptMirror<Deriver> {
    fn initialize<Store>(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
    ) -> Result<FileState, TranscriptJournalCorruptionError>
    where
        Store: TranscriptOccurrenceBlobStore,
        Deriver: TranscriptDeriver<Store>,
    {
        let path = self.jsonl_path_for(conversation_id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| io_corruption("create transcript directory", error))?;
        }

        let needs_build = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() == 0 => !checkpoint.turns.is_empty(),
            Ok(metadata) => {
                let mut file = File::open(&path)
                    .map_err(|error| io_corruption("open transcript tail", error))?;
                file.seek(SeekFrom::Start(metadata.len() - 1))
                    .map_err(|error| io_corruption("seek transcript tail", error))?;
                let mut tail = [0u8; 1];
                file.read_exact(&mut tail)
                    .map_err(|error| io_corruption("read transcript tail", error))?;
                tail[0] != b'\n'
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(io_corruption("stat transcript", error)),
        };

        if needs_build {
            let occurrences = self.deriver.initial(store, checkpoint)?;
            let mut content = String::new();
            for occurrence in occurrences {
                content.push_str(&occurrence.line);
                content.push('\n');
            }
            install_atomic(&path, content.as_bytes())?;
        }

        let identity = file_identity(
            &fs::metadata(&path)
                .map_err(|error| io_corruption("stat initialized transcript", error))?,
        );
        let state = FileState {
            bytes: identity.size,
            device: identity.device,
            inode: identity.inode,
        };
        self.states
            .insert(conversation_id.to_owned(), state.clone());
        Ok(state)
    }

    fn append_pending(
        &mut self,
        conversation_id: &str,
        pending: &PendingTranscriptCheckpoint,
    ) -> Result<FileState, TranscriptJournalCorruptionError> {
        let path = self.jsonl_path_for(conversation_id)?;
        let identity = file_identity(
            &fs::metadata(&path)
                .map_err(|error| io_corruption("stat canonical transcript", error))?,
        );
        if identity.device != pending.file_device
            || identity.inode != pending.file_inode
            || identity.size < pending.append_offset
        {
            return Err(corruption(
                "canonical transcript changed before WAL commit",
            ));
        }

        let expected = pending
            .lines
            .iter()
            .flat_map(|line| [line.as_bytes(), b"\n".as_slice()])
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        let tail_length = identity.size - pending.append_offset;
        if tail_length > expected.len() as u64 {
            return Err(corruption(
                "canonical transcript has data beyond the pending WAL",
            ));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_corruption("open canonical transcript", error))?;
        if tail_length > 0 {
            let mut tail = vec![0u8; tail_length as usize];
            file.seek(SeekFrom::Start(pending.append_offset))
                .and_then(|_| file.read_exact(&mut tail))
                .map_err(|error| io_corruption("read canonical transcript tail", error))?;
            if tail != expected[..tail_length as usize] {
                return Err(corruption(
                    "canonical transcript tail conflicts with the pending WAL",
                ));
            }
        }
        if tail_length < expected.len() as u64 {
            file.set_len(pending.append_offset)
                .map_err(|error| io_corruption("truncate canonical transcript", error))?;
            file.seek(SeekFrom::Start(pending.append_offset))
                .and_then(|_| file.write_all(&expected))
                .map_err(|error| io_corruption("append canonical transcript WAL", error))?;
            file.sync_all()
                .map_err(|error| io_corruption("sync canonical transcript", error))?;
        }

        let updated = file_identity(
            &file
                .metadata()
                .map_err(|error| io_corruption("stat committed transcript", error))?,
        );
        let state = FileState {
            bytes: pending.append_offset + expected.len() as u64,
            device: updated.device,
            inode: updated.inode,
        };
        self.states
            .insert(conversation_id.to_owned(), state.clone());
        Ok(state)
    }

    pub fn recover<Store>(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
    ) -> Result<(), TranscriptJournalCorruptionError>
    where
        Store: TranscriptOccurrenceBlobStore,
        Deriver: TranscriptDeriver<Store>,
    {
        let reporter = Arc::clone(&self.report_outcome);
        report_operation(&reporter, "replay", conversation_id, || {
            let pending = self.read_pending(conversation_id)?;
            let mut state = self.states.get(conversation_id).cloned();
            let hash = checkpoint_identity(checkpoint);
            if let Some(pending) = pending {
                if pending.checkpoint_hash == hash {
                    state = Some(self.append_pending(conversation_id, &pending)?);
                    self.write_deferred(conversation_id, pending.deferred_step)?;
                    self.remove_pending(conversation_id)?;
                } else if pending.previous_checkpoint_hash == hash {
                    self.remove_pending(conversation_id)?;
                } else {
                    return Err(corruption(
                        "pending transcript WAL does not match the durable checkpoint",
                    ));
                }
            }

            let state = match state {
                Some(state) => state,
                None => self.initialize(conversation_id, checkpoint, store)?,
            };
            self.states.insert(conversation_id.to_owned(), state);
            self.durable_checkpoints
                .insert(conversation_id.to_owned(), checkpoint.clone());
            self.prepared_checkpoints.remove(conversation_id);
            self.prepared_deferred_steps.remove(conversation_id);

            match self.read_deferred(conversation_id)? {
                Some(deferred) => {
                    if checkpoint.turns.get(deferred.turn_index).is_none() {
                        return Err(corruption(
                            "deferred transcript step is absent from the durable checkpoint",
                        ));
                    }
                    self.deferred_steps
                        .insert(conversation_id.to_owned(), deferred);
                }
                None => {
                    self.deferred_steps.remove(conversation_id);
                }
            }
            Ok(((), None, None))
        })
    }

    pub fn prepare_checkpoint<Store>(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), TranscriptJournalCorruptionError>
    where
        Store: TranscriptOccurrenceBlobStore,
        Deriver: TranscriptDeriver<Store>,
    {
        let reporter = Arc::clone(&self.report_outcome);
        report_operation(&reporter, "checkpoint", conversation_id, || {
            if self.read_pending(conversation_id)?.is_some() {
                return Err(corruption(
                    "pending transcript checkpoint must recover before preparing another",
                ));
            }
            let previous = self
                .durable_checkpoints
                .get(conversation_id)
                .cloned()
                .ok_or_else(|| corruption("transcript checkpoint must recover before preparing"))?;
            let state = self
                .states
                .get(conversation_id)
                .cloned()
                .ok_or_else(|| corruption("transcript checkpoint must recover before preparing"))?;

            let path = self.jsonl_path_for(conversation_id)?;
            let identity = file_identity(
                &fs::metadata(&path)
                    .map_err(|error| io_corruption("stat transcript before checkpoint", error))?,
            );
            if identity.size != state.bytes
                || identity.device != state.device
                || identity.inode != state.inode
            {
                return Err(corruption(
                    "canonical transcript changed outside the journal",
                ));
            }

            let derived = self.deriver.derive(
                store,
                &previous,
                checkpoint,
                finalize_checkpoint,
                self.deferred_steps.get(conversation_id).copied(),
            )?;
            let mut seen = HashSet::new();
            let mut lines = Vec::with_capacity(derived.occurrences.len());
            for occurrence in derived.occurrences {
                if !seen.insert(occurrence.id.clone()) {
                    return Err(corruption(format!(
                        "transcript occurrence {} was derived twice",
                        occurrence.id
                    )));
                }
                lines.push(occurrence.line);
            }

            let pending = PendingTranscriptCheckpoint {
                version: 1,
                previous_checkpoint_hash: checkpoint_identity(&previous),
                checkpoint_hash: checkpoint_identity(checkpoint),
                append_offset: state.bytes,
                file_device: state.device,
                file_inode: state.inode,
                lines,
                turn_count: checkpoint.turns.len(),
                deferred_step: derived.deferred_step,
            };
            let bytes = pending_checkpoint_json(&pending).into_bytes();
            install_atomic(&self.pending_path_for(conversation_id)?, &bytes)?;
            self.prepared_checkpoints
                .insert(conversation_id.to_owned(), checkpoint.clone());
            self.prepared_deferred_steps
                .insert(conversation_id.to_owned(), derived.deferred_step);
            Ok((
                (),
                Some(pending.lines.len()),
                Some(bytes.len() as u64),
            ))
        })
    }

    pub fn commit_checkpoint(
        &mut self,
        conversation_id: &str,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        let reporter = Arc::clone(&self.report_outcome);
        report_operation(&reporter, "append", conversation_id, || {
            let pending = self
                .read_pending(conversation_id)?
                .ok_or_else(|| corruption("prepared transcript WAL is missing at commit"))?;
            let checkpoint = self
                .prepared_checkpoints
                .get(conversation_id)
                .cloned()
                .ok_or_else(|| corruption("prepared transcript checkpoint is missing in memory"))?;
            let deferred = self
                .prepared_deferred_steps
                .get(conversation_id)
                .copied()
                .ok_or_else(|| corruption("prepared transcript checkpoint is missing in memory"))?;
            let state = self.append_pending(conversation_id, &pending)?;
            self.write_deferred(conversation_id, deferred)?;
            self.remove_pending(conversation_id)?;
            self.prepared_checkpoints.remove(conversation_id);
            self.prepared_deferred_steps.remove(conversation_id);
            self.durable_checkpoints
                .insert(conversation_id.to_owned(), checkpoint);
            match deferred {
                Some(step) => {
                    self.deferred_steps
                        .insert(conversation_id.to_owned(), step);
                }
                None => {
                    self.deferred_steps.remove(conversation_id);
                }
            }
            Ok((
                (),
                Some(pending.lines.len()),
                Some(state.bytes.saturating_sub(pending.append_offset)),
            ))
        })
    }

    pub fn abort_checkpoint(
        &mut self,
        conversation_id: &str,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        self.prepared_checkpoints.remove(conversation_id);
        self.prepared_deferred_steps.remove(conversation_id);
        self.remove_pending(conversation_id)
    }

    pub fn skip_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<(), TranscriptJournalCorruptionError> {
        self.prepared_checkpoints.remove(conversation_id);
        self.prepared_deferred_steps.remove(conversation_id);
        self.deferred_steps.remove(conversation_id);
        self.remove_pending(conversation_id)?;
        self.write_deferred(conversation_id, None)?;
        self.durable_checkpoints
            .insert(conversation_id.to_owned(), checkpoint.clone());
        Ok(())
    }
}

impl<Deriver, Store> TranscriptJournalPort<TranscriptCheckpoint, Store>
    for FileTranscriptMirror<Deriver>
where
    Store: TranscriptOccurrenceBlobStore,
    Deriver: TranscriptDeriver<Store>,
{
    fn owns_conversation(&mut self, conversation_id: &str) -> Result<bool, String> {
        FileTranscriptMirror::owns_conversation(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn claim_conversation(&mut self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::claim_conversation(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn recover(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        FileTranscriptMirror::recover(self, conversation_id, checkpoint, blob_store)
            .map_err(|error| error.to_string())
    }

    fn prepare_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        blob_store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), String> {
        FileTranscriptMirror::prepare_checkpoint(
            self,
            conversation_id,
            checkpoint,
            blob_store,
            finalize_checkpoint,
        )
        .map_err(|error| error.to_string())
    }

    fn commit_checkpoint(&mut self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::commit_checkpoint(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn abort_checkpoint(&mut self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::abort_checkpoint(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn skip_checkpoint(
        &mut self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        _blob_store: &Store,
    ) -> Result<(), String> {
        FileTranscriptMirror::skip_checkpoint(self, conversation_id, checkpoint)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript_occurrence_deriver::{
        DerivedTranscriptOccurrences, TranscriptOccurrence,
    };
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct Store(HashMap<Vec<u8>, Vec<u8>>);

    impl TranscriptOccurrenceBlobStore for Store {
        fn get_blob(&self, id: &[u8]) -> Option<Vec<u8>> {
            self.0.get(id).cloned()
        }
    }

    struct Deriver;

    impl TranscriptDeriver<Store> for Deriver {
        fn initial(
            &self,
            _store: &Store,
            checkpoint: &TranscriptCheckpoint,
        ) -> Result<Vec<TranscriptOccurrence>, TranscriptJournalCorruptionError> {
            Ok(checkpoint
                .turns
                .iter()
                .enumerate()
                .map(|(index, _)| TranscriptOccurrence {
                    id: format!("turn:{index}:initial"),
                    line: json!({
                        "role":"user",
                        "message":{"content":[{"type":"text","text":format!("initial-{index}")}]}
                    })
                    .to_string(),
                })
                .collect())
        }

        fn derive(
            &self,
            _store: &Store,
            previous: &TranscriptCheckpoint,
            checkpoint: &TranscriptCheckpoint,
            _finalize_checkpoint: bool,
            _deferred: Option<DeferredTranscriptStep>,
        ) -> Result<DerivedTranscriptOccurrences, TranscriptJournalCorruptionError> {
            let mut occurrences = Vec::new();
            for index in previous.turns.len()..checkpoint.turns.len() {
                occurrences.push(TranscriptOccurrence {
                    id: format!("turn:{index}:append"),
                    line: json!({
                        "role":"assistant",
                        "message":{"content":[{"type":"text","text":format!("append-{index}")}]}
                    })
                    .to_string(),
                });
            }
            Ok(DerivedTranscriptOccurrences {
                occurrences,
                deferred_step: None,
            })
        }
    }

    fn temp_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-transcript-journal-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn mode_marker_pins_only_safe_conversation_ids() {
        let root = temp_dir();
        let mirror = FileTranscriptMirror::without_reporting(&root, Deriver);
        assert!(!mirror.owns_conversation("conversation-a").unwrap());
        mirror.claim_conversation("conversation-a").unwrap();
        assert!(mirror.owns_conversation("conversation-a").unwrap());
        assert!(mirror.claim_conversation("../escape").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recover_builds_canonical_transcript_then_prepare_commit_appends_wal() {
        let root = temp_dir();
        let store = Store::default();
        let mut mirror = FileTranscriptMirror::without_reporting(&root, Deriver);
        let first = TranscriptCheckpoint {
            turns: vec![vec![1]],
        };
        let second = TranscriptCheckpoint {
            turns: vec![vec![1], vec![2]],
        };

        mirror.recover("conversation-a", &first, &store).unwrap();
        let path = mirror.jsonl_path_for("conversation-a").unwrap();
        let initial = fs::read_to_string(&path).unwrap();
        assert!(initial.contains("initial-0"));

        mirror
            .prepare_checkpoint("conversation-a", &second, &store, true)
            .unwrap();
        assert!(mirror.pending_path_for("conversation-a").unwrap().exists());
        mirror.commit_checkpoint("conversation-a").unwrap();
        assert!(!mirror.pending_path_for("conversation-a").unwrap().exists());
        let committed = fs::read_to_string(&path).unwrap();
        assert!(committed.contains("initial-0"));
        assert!(committed.contains("append-1"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recover_replays_matching_pending_wal_after_restart() {
        let root = temp_dir();
        let store = Store::default();
        let first = TranscriptCheckpoint {
            turns: vec![vec![1]],
        };
        let second = TranscriptCheckpoint {
            turns: vec![vec![1], vec![2]],
        };

        {
            let mut mirror = FileTranscriptMirror::without_reporting(&root, Deriver);
            mirror.recover("conversation-a", &first, &store).unwrap();
            mirror
                .prepare_checkpoint("conversation-a", &second, &store, true)
                .unwrap();
        }

        let mut reopened = FileTranscriptMirror::without_reporting(&root, Deriver);
        reopened
            .recover("conversation-a", &second, &store)
            .unwrap();
        let transcript =
            fs::read_to_string(reopened.jsonl_path_for("conversation-a").unwrap()).unwrap();
        assert!(transcript.contains("append-1"));
        assert!(!reopened.pending_path_for("conversation-a").unwrap().exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn external_canonical_mutation_fails_checkpoint_closed() {
        let root = temp_dir();
        let store = Store::default();
        let first = TranscriptCheckpoint {
            turns: vec![vec![1]],
        };
        let second = TranscriptCheckpoint {
            turns: vec![vec![1], vec![2]],
        };
        let mut mirror = FileTranscriptMirror::without_reporting(&root, Deriver);
        mirror.recover("conversation-a", &first, &store).unwrap();
        let path = mirror.jsonl_path_for("conversation-a").unwrap();
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"external\n")
            .unwrap();

        let error = mirror
            .prepare_checkpoint("conversation-a", &second, &store, true)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("canonical transcript changed outside the journal"));
        let _ = fs::remove_dir_all(root);
    }
}
