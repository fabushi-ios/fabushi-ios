use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent_isolation::{
    AgentBlobWorkerBackend, OffloadingTranscriptMirror, OffloadingTranscriptMirrorOptions,
    ProductionAgentStoreWorkerBackend, TranscriptMirrorOffloadPool, WorkerBlobStore,
};
use crate::extensions::session::production_agent_store::{
    ProductionAgentStore, ProductionWorkerBlobStore,
};
use crate::runner::{
    DurableTurnCheckpointStore, TranscriptCheckpointMirror, TurnCheckpointFuture,
};

use super::conversation_state_binary::{
    TranscriptMirrorConversationState, decode_transcript_mirror_conversation_state,
};
use super::legacy_transcript_mirror::LegacyTranscriptState;
use super::transcript_journal_codec::TranscriptCheckpoint;
use super::transcript_mirror::{
    FileTranscriptMirror, JournalOutcomeReporter, TranscriptDeriver,
};
use super::transcript_mirror_router::{
    JournalEnabledReader, LegacyTranscriptMirrorPort, RoutedTranscriptMirror,
    TranscriptJournalPort, TranscriptMirrorRoute,
};
use super::transcript_occurrence_deriver::{
    ArtifactTranscriptOccurrenceDeriver, TranscriptOccurrenceBlobStore,
    TranscriptOccurrenceCodec,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProductionTranscriptCheckpoint {
    pub state_bytes: Vec<u8>,
    pub journal: TranscriptCheckpoint,
    pub legacy: LegacyTranscriptState,
}

impl ProductionTranscriptCheckpoint {
    pub fn from_state_bytes(bytes: &[u8]) -> Result<Self, String> {
        let state = decode_transcript_mirror_conversation_state(bytes)
            .map_err(|error| error.to_string())?;
        Ok(Self::from_decoded_state_with_bytes(state, bytes.to_vec()))
    }

    pub fn from_decoded_state(state: TranscriptMirrorConversationState) -> Self {
        Self::from_decoded_state_with_bytes(state, Vec::new())
    }

    fn from_decoded_state_with_bytes(
        state: TranscriptMirrorConversationState,
        state_bytes: Vec<u8>,
    ) -> Self {
        Self {
            state_bytes,
            journal: TranscriptCheckpoint {
                turns: state.turns.clone(),
            },
            legacy: LegacyTranscriptState {
                root_prompt_messages_json: state.root_prompt_messages_json,
                summary_archives: state.summary_archives,
                turns: state.turns,
            },
        }
    }

    pub fn root_prompt_count(&self) -> usize {
        self.legacy.root_prompt_messages_json.len()
    }
}

impl<Backend> TranscriptOccurrenceBlobStore for Arc<WorkerBlobStore<Backend>>
where
    Backend: AgentBlobWorkerBackend + 'static,
    Backend::Error: std::fmt::Display,
{
    fn get_blob(&self, id: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.as_ref()
            .get_blob_blocking(id)
            .map_err(|error| error.to_string())
    }
}

struct ProductionJournalAdapter<Store> {
    journal: Arc<FileTranscriptMirror<Store>>,
}

impl<Store> ProductionJournalAdapter<Store> {
    fn new(journal: Arc<FileTranscriptMirror<Store>>) -> Self {
        Self { journal }
    }
}

impl<Store> TranscriptJournalPort<ProductionTranscriptCheckpoint, Store>
    for ProductionJournalAdapter<Store>
where
    Store: Send + Sync,
{
    fn owns_conversation(&self, conversation_id: &str) -> Result<bool, String> {
        self.journal
            .owns_conversation(conversation_id)
            .map_err(|error| error.to_string())
    }

    fn claim_conversation(&self, conversation_id: &str) -> Result<(), String> {
        self.journal
            .claim_conversation(conversation_id)
            .map_err(|error| error.to_string())
    }

    fn recover(
        &self,
        conversation_id: &str,
        checkpoint: &ProductionTranscriptCheckpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        self.journal
            .recover(conversation_id, &checkpoint.journal, blob_store)
            .map_err(|error| error.to_string())
    }

    fn prepare_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &ProductionTranscriptCheckpoint,
        blob_store: &Store,
        finalize_checkpoint: bool,
    ) -> Result<(), String> {
        self.journal
            .prepare_checkpoint(
                conversation_id,
                &checkpoint.journal,
                blob_store,
                finalize_checkpoint,
            )
            .map_err(|error| error.to_string())
    }

    fn commit_checkpoint(&self, conversation_id: &str) -> Result<(), String> {
        self.journal
            .commit_checkpoint(conversation_id)
            .map_err(|error| error.to_string())
    }

    fn abort_checkpoint(&self, conversation_id: &str) -> Result<(), String> {
        self.journal
            .abort_checkpoint(conversation_id)
            .map_err(|error| error.to_string())
    }

    fn skip_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint: &ProductionTranscriptCheckpoint,
        blob_store: &Store,
    ) -> Result<(), String> {
        let _ = blob_store;
        self.journal
            .skip_checkpoint(conversation_id, &checkpoint.journal)
            .map_err(|error| error.to_string())
    }
}

struct ProductionLegacyAdapter {
    legacy: Arc<OffloadingTranscriptMirror>,
}

impl ProductionLegacyAdapter {
    fn new(legacy: Arc<OffloadingTranscriptMirror>) -> Self {
        Self { legacy }
    }
}

impl LegacyTranscriptMirrorPort<
    ProductionTranscriptCheckpoint,
    Arc<ProductionWorkerBlobStore>,
> for ProductionLegacyAdapter {
    fn write(
        &self,
        conversation_id: &str,
        checkpoint: &ProductionTranscriptCheckpoint,
        blob_store: &Arc<ProductionWorkerBlobStore>,
        state_blob_id: &[u8],
    ) -> Result<(), String> {
        self.legacy.write_worker(
            conversation_id,
            &checkpoint.legacy,
            blob_store.as_ref(),
            Some(state_blob_id),
        )
    }
}

pub type ProductionRoutedTranscriptMirror = RoutedTranscriptMirror<
    ProductionTranscriptCheckpoint,
    Arc<ProductionWorkerBlobStore>,
>;

impl TranscriptCheckpointMirror<
    ProductionTranscriptCheckpoint,
    Arc<ProductionWorkerBlobStore>,
> for ProductionRoutedTranscriptMirror {
    fn prepare_checkpoint<'a>(
        &'a self,
        transcript_id: &'a str,
        checkpoint: &'a ProductionTranscriptCheckpoint,
        blob_store: &'a Arc<ProductionWorkerBlobStore>,
        finalize: bool,
        force: bool,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        Box::pin(async move {
            RoutedTranscriptMirror::prepare_checkpoint(
                self,
                transcript_id,
                checkpoint,
                blob_store,
                finalize,
                force,
            )
        })
    }

    fn abort_checkpoint<'a>(
        &'a self,
        transcript_id: &'a str,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        Box::pin(async move {
            RoutedTranscriptMirror::abort_checkpoint(self, transcript_id)
        })
    }

    fn commit_checkpoint<'a>(
        &'a self,
        transcript_id: &'a str,
        latest_root_blob_id: Option<&'a str>,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let root = match RoutedTranscriptMirror::route(self, transcript_id)? {
                TranscriptMirrorRoute::Journal => Vec::new(),
                TranscriptMirrorRoute::Legacy => {
                    let value = latest_root_blob_id.ok_or_else(|| {
                        "legacy transcript mirror commit requires latestRootBlobId".to_string()
                    })?;
                    decode_hex_id(value)?
                }
            };
            RoutedTranscriptMirror::commit_checkpoint(self, transcript_id, &root)
        })
    }

    fn skip_checkpoint<'a>(
        &'a self,
        transcript_id: &'a str,
        checkpoint: &'a ProductionTranscriptCheckpoint,
        blob_store: &'a Arc<ProductionWorkerBlobStore>,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        Box::pin(async move {
            RoutedTranscriptMirror::skip_checkpoint(
                self,
                transcript_id,
                checkpoint,
                blob_store,
            )
        })
    }
}

impl DurableTurnCheckpointStore<ProductionTranscriptCheckpoint>
    for ProductionAgentStore
{
    fn handle_checkpoint<'a>(
        &'a self,
        checkpoint: &'a ProductionTranscriptCheckpoint,
    ) -> TurnCheckpointFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.handle_checkpoint_bytes_async(&checkpoint.state_bytes)
                .await
                .map(|_| ())
        })
    }

    fn latest_root_blob_id(&self) -> Option<String> {
        let root = ProductionAgentStore::latest_root_blob_id(self);
        (!root.is_empty()).then(|| encode_hex_id(&root))
    }
}

fn encode_hex_id(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex_id(value: &str) -> Result<Vec<u8>, String> {
    let clean = value.trim();
    if clean.len() % 2 != 0 {
        return Err("latestRootBlobId has odd-length hex".into());
    }
    let mut output = Vec::with_capacity(clean.len() / 2);
    for index in (0..clean.len()).step_by(2) {
        output.push(
            u8::from_str_radix(&clean[index..index + 2], 16)
                .map_err(|_| format!("latestRootBlobId has invalid hex at offset {index}"))?,
        );
    }
    Ok(output)
}

pub struct ProductionTranscriptMirrorProvider<Codec>
where
    Codec: TranscriptOccurrenceCodec + 'static,
{
    transcripts_dir: PathBuf,
    journal: Arc<FileTranscriptMirror<Arc<ProductionWorkerBlobStore>>>,
    offload_pool: Arc<TranscriptMirrorOffloadPool>,
    _codec: std::marker::PhantomData<Codec>,
}

impl<Codec> ProductionTranscriptMirrorProvider<Codec>
where
    Codec: TranscriptOccurrenceCodec + 'static,
{
    pub fn new(
        transcripts_dir: impl Into<PathBuf>,
        codec: Codec,
    ) -> Self {
        Self::with_offload_pool(
            transcripts_dir,
            codec,
            Arc::new(TranscriptMirrorOffloadPool::production()),
        )
    }

    pub fn with_reporter(
        transcripts_dir: impl Into<PathBuf>,
        codec: Codec,
        report_outcome: JournalOutcomeReporter,
    ) -> Self {
        Self::with_offload_pool_and_reporter(
            transcripts_dir,
            codec,
            Arc::new(TranscriptMirrorOffloadPool::production()),
            report_outcome,
        )
    }

    pub fn with_offload_pool(
        transcripts_dir: impl Into<PathBuf>,
        codec: Codec,
        offload_pool: Arc<TranscriptMirrorOffloadPool>,
    ) -> Self {
        Self::with_offload_pool_and_reporter(
            transcripts_dir,
            codec,
            offload_pool,
            Arc::new(|_| {}),
        )
    }

    pub fn with_offload_pool_and_reporter(
        transcripts_dir: impl Into<PathBuf>,
        codec: Codec,
        offload_pool: Arc<TranscriptMirrorOffloadPool>,
        report_outcome: JournalOutcomeReporter,
    ) -> Self {
        let transcripts_dir = transcripts_dir.into();
        let deriver: Arc<
            dyn TranscriptDeriver<Arc<ProductionWorkerBlobStore>>
        > = Arc::new(ArtifactTranscriptOccurrenceDeriver::new(codec));
        let journal = Arc::new(FileTranscriptMirror::with_reporter(
            &transcripts_dir,
            report_outcome,
            deriver,
        ));
        Self {
            transcripts_dir,
            journal,
            offload_pool,
            _codec: std::marker::PhantomData,
        }
    }

    pub fn transcripts_dir(&self) -> &Path {
        &self.transcripts_dir
    }

    pub fn route_for_session(
        &self,
        blob_store: Arc<ProductionWorkerBlobStore>,
        prior_state_bytes: &[u8],
        is_journal_enabled: JournalEnabledReader,
    ) -> Result<ProductionRoutedTranscriptMirror, String> {
        let prior = ProductionTranscriptCheckpoint::from_state_bytes(
            prior_state_bytes,
        )?;
        let mut blob_db_paths = vec![blob_store.blob_db_path.clone()];
        if let Some(legacy) = blob_store.legacy_blob_db_path.clone() {
            blob_db_paths.push(legacy);
        }
        let legacy = Arc::new(OffloadingTranscriptMirror::for_transcripts_dir(
            Arc::clone(&self.offload_pool),
            OffloadingTranscriptMirrorOptions {
                blob_db_paths,
                transcripts_dir: self.transcripts_dir.clone(),
            },
            prior.root_prompt_count(),
        ));
        let journal: Arc<
            dyn TranscriptJournalPort<
                ProductionTranscriptCheckpoint,
                Arc<ProductionWorkerBlobStore>,
            >,
        > = Arc::new(ProductionJournalAdapter::new(Arc::clone(&self.journal)));
        let legacy: Arc<
            dyn LegacyTranscriptMirrorPort<
                ProductionTranscriptCheckpoint,
                Arc<ProductionWorkerBlobStore>,
            >,
        > = Arc::new(ProductionLegacyAdapter::new(legacy));
        Ok(RoutedTranscriptMirror::new(
            journal,
            legacy,
            is_journal_enabled,
        ))
    }
}
