use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use uuid::Uuid;

use super::transcript_journal_codec::{
    DeferredTranscriptStep, PendingTranscriptCheckpoint, PendingTranscriptCursor,
    TranscriptCheckpoint, checkpoint_identity, file_identity, parse_deferred_step,
    parse_pending_checkpoint, write_all_at,
};
use super::transcript_mirror_router::TranscriptJournalPort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptOccurrence {
    pub id: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DerivedTranscriptOccurrences {
    pub occurrences: Vec<TranscriptOccurrence>,
    pub deferred_step: Option<DeferredTranscriptStep>,
}

pub trait TranscriptDeriver<Store>: Send + Sync {
    fn derive(
        &self,
        store: &Store,
        previous: &TranscriptCheckpoint,
        checkpoint: &TranscriptCheckpoint,
        finalize_checkpoint: bool,
        deferred: Option<DeferredTranscriptStep>,
    ) -> Result<DerivedTranscriptOccurrences, String>;

    fn initial(
        &self,
        store: &Store,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<Vec<TranscriptOccurrence>, String>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct JournalOutcome {
    pub op: String,
    pub outcome: String,
    pub conversation_id: String,
    pub entry_count: Option<usize>,
    pub bytes: Option<u64>,
    pub duration_ms: f64,
    pub cause: Option<String>,
}

pub type JournalOutcomeReporter =
    Arc<dyn Fn(&JournalOutcome) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileTranscriptMirrorError {
    #[error("transcript journal corruption: {0}")]
    Corruption(String),
    #[error("transcript journal I/O failed: {0}")]
    Io(String),
    #[error("transcript occurrence derivation failed: {0}")]
    Deriver(String),
    #[error("transcript journal state is poisoned")]
    Poisoned,
}

impl From<std::io::Error> for FileTranscriptMirrorError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileState {
    bytes: u64,
    device: String,
    inode: String,
}

pub struct FileTranscriptMirror<Store> {
    transcripts_dir: PathBuf,
    report_outcome: JournalOutcomeReporter,
    deriver: Arc<dyn TranscriptDeriver<Store>>,
    states: Mutex<HashMap<String, FileState>>,
    durable_checkpoints: Mutex<HashMap<String, TranscriptCheckpoint>>,
    prepared_checkpoints: Mutex<HashMap<String, TranscriptCheckpoint>>,
    deferred_steps: Mutex<HashMap<String, DeferredTranscriptStep>>,
    prepared_deferred_steps: Mutex<HashMap<String, Option<DeferredTranscriptStep>>>,
    write_lanes: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl<Store> FileTranscriptMirror<Store> {
    pub fn new(
        transcripts_dir: impl Into<PathBuf>,
        deriver: Arc<dyn TranscriptDeriver<Store>>,
    ) -> Self {
        Self::with_reporter(transcripts_dir, Arc::new(|_| {}), deriver)
    }

    pub fn with_reporter(
        transcripts_dir: impl Into<PathBuf>,
        report_outcome: JournalOutcomeReporter,
        deriver: Arc<dyn TranscriptDeriver<Store>>,
    ) -> Self {
        Self {
            transcripts_dir: transcripts_dir.into(),
            report_outcome,
            deriver,
            states: Mutex::new(HashMap::new()),
            durable_checkpoints: Mutex::new(HashMap::new()),
            prepared_checkpoints: Mutex::new(HashMap::new()),
            deferred_steps: Mutex::new(HashMap::new()),
            prepared_deferred_steps: Mutex::new(HashMap::new()),
            write_lanes: Mutex::new(HashMap::new()),
        }
    }

    pub fn jsonl_path_for(&self, conversation_id: &str) -> Result<PathBuf, FileTranscriptMirrorError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.jsonl")))
    }

    pub fn pending_path_for(&self, conversation_id: &str) -> Result<PathBuf, FileTranscriptMirrorError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-pending.json")))
    }

    pub fn cursor_path_for(&self, conversation_id: &str) -> Result<PathBuf, FileTranscriptMirrorError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-cursor.json")))
    }

    pub fn mode_path_for(&self, conversation_id: &str) -> Result<PathBuf, FileTranscriptMirrorError> {
        let safe = safe_id(conversation_id)?;
        Ok(self
            .transcripts_dir
            .join(safe)
            .join(format!("{safe}.journal-mode")))
    }

    pub fn owns_conversation(&self, conversation_id: &str) -> Result<bool, FileTranscriptMirrorError> {
        let path = self.mode_path_for(conversation_id)?;
        match fs::metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn claim_conversation(&self, conversation_id: &str) -> Result<(), FileTranscriptMirrorError> {
        let path = self.mode_path_for(conversation_id)?;
        if self.owns_conversation(conversation_id)? {
            return Ok(());
        }
        self.install_atomic(&path, b"1\n")
    }

    pub fn recover(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
    ) -> Result<(), FileTranscriptMirrorError> {
        let started = Instant::now();
        let result = self.serialized(conversation_id, || {
            let pending = self.read_pending(conversation_id)?;
            let mut state = self.state(conversation_id)?;
            let hash = checkpoint_identity(checkpoint);
            if let Some(pending) = pending {
                if pending.checkpoint_hash == hash {
                    state = Some(self.append_pending(conversation_id, &pending)?);
                    self.write_deferred(
                        conversation_id,
                        pending.cursor.deferred_step,
                    )?;
                    self.remove_pending(conversation_id)?;
                } else if pending.previous_checkpoint_hash == hash {
                    self.remove_pending(conversation_id)?;
                } else {
                    return Err(FileTranscriptMirrorError::Corruption(
                        "pending transcript WAL does not match the durable checkpoint".into(),
                    ));
                }
            }

            let state = match state {
                Some(state) => state,
                None => self.initialize(conversation_id, checkpoint, store)?,
            };
            self.set_state(conversation_id, state)?;
            self.durable_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .insert(conversation_id.to_string(), checkpoint.clone());
            self.prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.prepared_deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);

            match self.read_deferred(conversation_id)? {
                Some(deferred) => {
                    if checkpoint.turns.get(deferred.turn_index).is_none() {
                        return Err(FileTranscriptMirrorError::Corruption(
                            "deferred transcript step is absent from the durable checkpoint".into(),
                        ));
                    }
                    self.deferred_steps
                        .lock()
                        .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                        .insert(conversation_id.to_string(), deferred);
                }
                None => {
                    self.deferred_steps
                        .lock()
                        .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                        .remove(conversation_id);
                }
            }
            Ok(())
        });
        self.report("replay", conversation_id, started, &result, None, None);
        result
    }

    pub fn prepare_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), FileTranscriptMirrorError> {
        let started = Instant::now();
        let result = self.serialized(conversation_id, || {
            if self.read_pending(conversation_id)?.is_some() {
                return Err(FileTranscriptMirrorError::Corruption(
                    "pending transcript checkpoint must recover before preparing another".into(),
                ));
            }
            let previous = self
                .durable_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .get(conversation_id)
                .cloned()
                .ok_or_else(|| {
                    FileTranscriptMirrorError::Corruption(
                        "transcript checkpoint must recover before preparing".into(),
                    )
                })?;
            let state = self
                .state(conversation_id)?
                .ok_or_else(|| {
                    FileTranscriptMirrorError::Corruption(
                        "transcript checkpoint must recover before preparing".into(),
                    )
                })?;
            let identity = file_identity(&fs::metadata(self.jsonl_path_for(conversation_id)?)?);
            if identity.size != state.bytes
                || identity.device != state.device
                || identity.inode != state.inode
            {
                return Err(FileTranscriptMirrorError::Corruption(
                    "canonical transcript changed outside the journal".into(),
                ));
            }
            let deferred = self
                .deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .get(conversation_id)
                .copied();
            let derived = self
                .deriver
                .derive(
                    store,
                    &previous,
                    checkpoint,
                    finalize_checkpoint,
                    deferred,
                )
                .map_err(FileTranscriptMirrorError::Deriver)?;
            let mut seen = HashSet::new();
            let mut lines = Vec::with_capacity(derived.occurrences.len());
            for occurrence in derived.occurrences {
                if !seen.insert(occurrence.id.clone()) {
                    return Err(FileTranscriptMirrorError::Corruption(format!(
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
                cursor: PendingTranscriptCursor {
                    turn_count: checkpoint.turns.len(),
                    deferred_step: derived.deferred_step,
                },
            };
            let bytes = serde_json::to_vec(&pending).map_err(|error| {
                FileTranscriptMirrorError::Corruption(error.to_string())
            })?;
            self.install_atomic(
                &self.pending_path_for(conversation_id)?,
                &bytes,
            )?;
            self.prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .insert(conversation_id.to_string(), checkpoint.clone());
            self.prepared_deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .insert(conversation_id.to_string(), derived.deferred_step);
            Ok((pending.lines.len(), bytes.len() as u64))
        });
        let public = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let counts = result.as_ref().ok().copied();
        self.report(
            "checkpoint",
            conversation_id,
            started,
            &public,
            counts.map(|value| value.0),
            counts.map(|value| value.1),
        );
        public
    }

    pub fn commit_checkpoint(
        &self,
        conversation_id: &str,
    ) -> Result<(), FileTranscriptMirrorError> {
        let started = Instant::now();
        let result = self.serialized(conversation_id, || {
            let pending = self.read_pending(conversation_id)?.ok_or_else(|| {
                FileTranscriptMirrorError::Corruption(
                    "prepared transcript WAL is missing at commit".into(),
                )
            })?;
            let checkpoint = self
                .prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .get(conversation_id)
                .cloned()
                .ok_or_else(|| {
                    FileTranscriptMirrorError::Corruption(
                        "prepared transcript checkpoint is missing in memory".into(),
                    )
                })?;
            let deferred = {
                let prepared = self
                    .prepared_deferred_steps
                    .lock()
                    .map_err(|_| FileTranscriptMirrorError::Poisoned)?;
                if !prepared.contains_key(conversation_id) {
                    return Err(FileTranscriptMirrorError::Corruption(
                        "prepared transcript checkpoint is missing in memory".into(),
                    ));
                }
                prepared.get(conversation_id).copied().flatten()
            };

            let state = self.append_pending(conversation_id, &pending)?;
            self.write_deferred(conversation_id, deferred)?;
            self.remove_pending(conversation_id)?;
            self.prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.prepared_deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.durable_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .insert(conversation_id.to_string(), checkpoint);
            match deferred {
                Some(deferred) => {
                    self.deferred_steps
                        .lock()
                        .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                        .insert(conversation_id.to_string(), deferred);
                }
                None => {
                    self.deferred_steps
                        .lock()
                        .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                        .remove(conversation_id);
                }
            }
            Ok((
                pending.lines.len(),
                state.bytes.saturating_sub(pending.append_offset),
            ))
        });
        let public = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let counts = result.as_ref().ok().copied();
        self.report(
            "append",
            conversation_id,
            started,
            &public,
            counts.map(|value| value.0),
            counts.map(|value| value.1),
        );
        public
    }

    pub fn abort_checkpoint(
        &self,
        conversation_id: &str,
    ) -> Result<(), FileTranscriptMirrorError> {
        self.serialized(conversation_id, || {
            self.prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.prepared_deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.remove_pending(conversation_id)
        })
    }

    pub fn skip_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
    ) -> Result<(), FileTranscriptMirrorError> {
        self.serialized(conversation_id, || {
            self.prepared_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.prepared_deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.deferred_steps
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .remove(conversation_id);
            self.remove_pending(conversation_id)?;
            self.write_deferred(conversation_id, None)?;
            self.durable_checkpoints
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?
                .insert(conversation_id.to_string(), checkpoint.clone());
            Ok(())
        })
    }

    pub fn durable_checkpoint(
        &self,
        conversation_id: &str,
    ) -> Option<TranscriptCheckpoint> {
        self.durable_checkpoints
            .lock()
            .ok()
            .and_then(|checkpoints| checkpoints.get(conversation_id).cloned())
    }

    fn initialize(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        store: &Store,
    ) -> Result<FileState, FileTranscriptMirrorError> {
        let path = self.jsonl_path_for(conversation_id)?;
        let parent = path.parent().ok_or_else(|| {
            FileTranscriptMirrorError::Io("transcript JSONL path has no parent".into())
        })?;
        fs::create_dir_all(parent)?;

        let needs_build = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() == 0 => !checkpoint.turns.is_empty(),
            Ok(metadata) => {
                let mut file = File::open(&path)?;
                file.seek(SeekFrom::Start(metadata.len().saturating_sub(1)))?;
                let mut tail = [0u8; 1];
                file.read_exact(&mut tail)?;
                tail[0] != b'\n'
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
        };

        if needs_build {
            let occurrences = self
                .deriver
                .initial(store, checkpoint)
                .map_err(FileTranscriptMirrorError::Deriver)?;
            let mut bytes = Vec::new();
            for occurrence in occurrences {
                bytes.extend_from_slice(occurrence.line.as_bytes());
                bytes.push(b'\n');
            }
            self.install_atomic(&path, &bytes)?;
        }

        let identity = file_identity(&fs::metadata(&path)?);
        Ok(FileState {
            bytes: identity.size,
            device: identity.device,
            inode: identity.inode,
        })
    }

    fn append_pending(
        &self,
        conversation_id: &str,
        pending: &PendingTranscriptCheckpoint,
    ) -> Result<FileState, FileTranscriptMirrorError> {
        let path = self.jsonl_path_for(conversation_id)?;
        let identity = file_identity(&fs::metadata(&path)?);
        if identity.device != pending.file_device
            || identity.inode != pending.file_inode
            || identity.size < pending.append_offset
        {
            return Err(FileTranscriptMirrorError::Corruption(
                "canonical transcript changed before WAL commit".into(),
            ));
        }

        let mut expected = Vec::new();
        for line in &pending.lines {
            expected.extend_from_slice(line.as_bytes());
            expected.push(b'\n');
        }
        let tail_length_u64 = identity.size.saturating_sub(pending.append_offset);
        let tail_length = usize::try_from(tail_length_u64).map_err(|_| {
            FileTranscriptMirrorError::Corruption(
                "canonical transcript tail is too large".into(),
            )
        })?;
        if tail_length > expected.len() {
            return Err(FileTranscriptMirrorError::Corruption(
                "canonical transcript has data beyond the pending WAL".into(),
            ));
        }

        let mut handle = OpenOptions::new().read(true).write(true).open(&path)?;
        if tail_length > 0 {
            handle.seek(SeekFrom::Start(pending.append_offset))?;
            let mut tail = vec![0u8; tail_length];
            handle.read_exact(&mut tail)?;
            if tail != expected[..tail_length] {
                return Err(FileTranscriptMirrorError::Corruption(
                    "canonical transcript tail conflicts with the pending WAL".into(),
                ));
            }
        }
        if tail_length < expected.len() {
            handle.set_len(pending.append_offset)?;
            write_all_at(&mut handle, &expected, pending.append_offset)
                .map_err(|error| FileTranscriptMirrorError::Io(error.to_string()))?;
            handle.sync_all()?;
        }
        let updated = file_identity(&handle.metadata()?);
        let state = FileState {
            bytes: pending
                .append_offset
                .saturating_add(expected.len() as u64),
            device: updated.device,
            inode: updated.inode,
        };
        self.set_state(conversation_id, state.clone())?;
        Ok(state)
    }

    fn read_pending(
        &self,
        conversation_id: &str,
    ) -> Result<Option<PendingTranscriptCheckpoint>, FileTranscriptMirrorError> {
        let path = self.pending_path_for(conversation_id)?;
        match fs::read_to_string(path) {
            Ok(raw) => parse_pending_checkpoint(&raw)
                .map(Some)
                .map_err(|error| FileTranscriptMirrorError::Corruption(error.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn remove_pending(&self, conversation_id: &str) -> Result<(), FileTranscriptMirrorError> {
        let path = self.pending_path_for(conversation_id)?;
        match fs::remove_file(&path) {
            Ok(()) => self.sync_parent(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn read_deferred(
        &self,
        conversation_id: &str,
    ) -> Result<Option<DeferredTranscriptStep>, FileTranscriptMirrorError> {
        let path = self.cursor_path_for(conversation_id)?;
        match fs::read_to_string(path) {
            Ok(raw) => {
                let value: serde_json::Value = serde_json::from_str(&raw)
                    .map_err(|error| FileTranscriptMirrorError::Corruption(error.to_string()))?;
                parse_deferred_step(Some(&value))
                    .map_err(|error| FileTranscriptMirrorError::Corruption(error.to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write_deferred(
        &self,
        conversation_id: &str,
        step: Option<DeferredTranscriptStep>,
    ) -> Result<(), FileTranscriptMirrorError> {
        let path = self.cursor_path_for(conversation_id)?;
        match step {
            Some(step) => {
                let bytes = serde_json::to_vec(&step)
                    .map_err(|error| FileTranscriptMirrorError::Corruption(error.to_string()))?;
                self.install_atomic(&path, &bytes)
            }
            None => match fs::remove_file(&path) {
                Ok(()) => self.sync_parent(&path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            },
        }
    }

    fn install_atomic(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), FileTranscriptMirrorError> {
        let parent = path.parent().ok_or_else(|| {
            FileTranscriptMirrorError::Io("transcript journal path has no parent".into())
        })?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".transcript.{}.part", Uuid::new_v4()));
        let result = (|| -> Result<(), FileTranscriptMirrorError> {
            let mut handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            handle.write_all(bytes)?;
            handle.sync_all()?;
            drop(handle);
            fs::rename(&temporary, path)?;
            self.sync_parent(path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn sync_parent(&self, path: &Path) -> Result<(), FileTranscriptMirrorError> {
        #[cfg(unix)]
        {
            let parent = path.parent().ok_or_else(|| {
                FileTranscriptMirrorError::Io("transcript journal path has no parent".into())
            })?;
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    fn serialized<T>(
        &self,
        conversation_id: &str,
        operation: impl FnOnce() -> Result<T, FileTranscriptMirrorError>,
    ) -> Result<T, FileTranscriptMirrorError> {
        let lane = {
            let mut lanes = self
                .write_lanes
                .lock()
                .map_err(|_| FileTranscriptMirrorError::Poisoned)?;
            Arc::clone(
                lanes
                    .entry(conversation_id.to_string())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let guard = lane.lock().map_err(|_| FileTranscriptMirrorError::Poisoned)?;
        let result = operation();
        drop(guard);
        if Arc::strong_count(&lane) == 2 {
            if let Ok(mut lanes) = self.write_lanes.lock() {
                if lanes
                    .get(conversation_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &lane))
                {
                    lanes.remove(conversation_id);
                }
            }
        }
        result
    }

    fn state(&self, conversation_id: &str) -> Result<Option<FileState>, FileTranscriptMirrorError> {
        Ok(self
            .states
            .lock()
            .map_err(|_| FileTranscriptMirrorError::Poisoned)?
            .get(conversation_id)
            .cloned())
    }

    fn set_state(
        &self,
        conversation_id: &str,
        state: FileState,
    ) -> Result<(), FileTranscriptMirrorError> {
        self.states
            .lock()
            .map_err(|_| FileTranscriptMirrorError::Poisoned)?
            .insert(conversation_id.to_string(), state);
        Ok(())
    }

    fn report<T>(
        &self,
        op: &str,
        conversation_id: &str,
        started: Instant,
        result: &Result<T, FileTranscriptMirrorError>,
        entry_count: Option<usize>,
        bytes: Option<u64>,
    ) {
        (self.report_outcome)(&JournalOutcome {
            op: op.to_string(),
            outcome: if result.is_ok() { "ok" } else { "failed" }.to_string(),
            conversation_id: conversation_id.to_string(),
            entry_count,
            bytes,
            duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            cause: result.as_ref().err().map(ToString::to_string),
        });
    }
}

impl<Store> TranscriptJournalPort<TranscriptCheckpoint, Store> for FileTranscriptMirror<Store>
where
    Store: Clone + Send + Sync + 'static,
{
    fn owns_conversation(&self, conversation_id: &str) -> Result<bool, String> {
        FileTranscriptMirror::owns_conversation(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn claim_conversation(&self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::claim_conversation(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn recover(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        FileTranscriptMirror::recover(self, conversation_id, checkpoint, blob_store)
            .map_err(|error| error.to_string())
    }

    fn prepare_checkpoint(
        &self,
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

    fn commit_checkpoint(&self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::commit_checkpoint(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn abort_checkpoint(&self, conversation_id: &str) -> Result<(), String> {
        FileTranscriptMirror::abort_checkpoint(self, conversation_id)
            .map_err(|error| error.to_string())
    }

    fn skip_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &TranscriptCheckpoint,
        _blob_store: &Store,
    ) -> Result<(), String> {
        FileTranscriptMirror::skip_checkpoint(self, conversation_id, checkpoint)
            .map_err(|error| error.to_string())
    }
}

fn safe_id(id: &str) -> Result<&str, FileTranscriptMirrorError> {
    if !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(id)
    } else {
        Err(FileTranscriptMirrorError::Corruption(
            "unsafe conversation id".into(),
        ))
    }
}
