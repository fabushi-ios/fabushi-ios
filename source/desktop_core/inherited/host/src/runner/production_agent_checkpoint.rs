use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError,
};
use crate::extensions::session::production_agent_store::{
    ProductionAgentStore, ProductionWorkerBlobStore,
};
use crate::transcript_mirror::conversation_state_binary::{
    decode_conversation_state_recovery_fields, remove_conversation_state_token_details,
};
use crate::transcript_mirror::production_provider::{
    ProductionRoutedTranscriptMirror, ProductionTranscriptCheckpoint,
};

use super::{SettledTokenDetails, TokenDetailsPersistenceTracker, TurnRunOptions};

pub trait AgentStateCheckpointSink: Send + Sync {
    fn base_state_bytes(&self) -> Result<Vec<u8>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn read_state_blob(
        &self,
        _blob_id: &[u8],
    ) -> Result<Option<Vec<u8>>, ProviderSessionError> {
        Err(ProviderSessionError::Protocol(
            "Agent checkpoint sink does not support canonical state blob reads".into(),
        ))
    }

    fn stage_text_turn(
        &self,
        _messages: &[ProviderMessage],
        _options: &TurnRunOptions,
        _assistant_content: &str,
    ) -> Result<TextTurnCheckpointArtifacts, ProviderSessionError> {
        Err(ProviderSessionError::Protocol(
            "Agent checkpoint sink does not support staged generated state".into(),
        ))
    }

    fn stage_generated_turn(
        &self,
        messages: &[ProviderMessage],
        options: &TurnRunOptions,
        generated_step_bytes: &[Vec<u8>],
        assistant_content: &str,
    ) -> Result<TextTurnCheckpointArtifacts, ProviderSessionError> {
        if generated_step_bytes.is_empty() {
            return self.stage_text_turn(messages, options, assistant_content);
        }
        Err(ProviderSessionError::Protocol(
            "Agent checkpoint sink does not support generated tool steps".into(),
        ))
    }

    fn persist_staged_text_turn(
        &self,
        _options: &TurnRunOptions,
        _checkpoint: &TextTurnCheckpointArtifacts,
    ) -> Result<(), ProviderSessionError> {
        Err(ProviderSessionError::Protocol(
            "Agent checkpoint sink does not support staged generated state".into(),
        ))
    }

    fn checkpoint_text_turn(
        &self,
        messages: &[ProviderMessage],
        options: &TurnRunOptions,
        assistant_content: &str,
    ) -> Result<(), ProviderSessionError> {
        let checkpoint = self.stage_text_turn(messages, options, assistant_content)?;
        self.persist_staged_text_turn(options, &checkpoint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextTurnCheckpointArtifacts {
    pub prior_state_bytes: Vec<u8>,
    pub user_message_id: Vec<u8>,
    pub user_message_bytes: Vec<u8>,
    pub step_ids: Vec<Vec<u8>>,
    pub step_bytes: Vec<Vec<u8>>,
    pub turn_id: Vec<u8>,
    pub turn_bytes: Vec<u8>,
    pub state_bytes: Vec<u8>,
}

pub fn build_text_turn_checkpoint(
    prior_state_bytes: &[u8],
    user_text: &str,
    message_id: &str,
    request_id: Option<&str>,
    assistant_content: &str,
) -> TextTurnCheckpointArtifacts {
    build_text_turn_checkpoint_with_rich_text(
        prior_state_bytes,
        user_text,
        None,
        message_id,
        request_id,
        assistant_content,
    )
}

pub fn build_text_turn_checkpoint_with_rich_text(
    prior_state_bytes: &[u8],
    user_text: &str,
    user_rich_text: Option<&str>,
    message_id: &str,
    request_id: Option<&str>,
    assistant_content: &str,
) -> TextTurnCheckpointArtifacts {
    build_generated_turn_checkpoint_with_rich_text(
        prior_state_bytes,
        user_text,
        user_rich_text,
        message_id,
        request_id,
        &[],
        assistant_content,
    )
}

pub fn build_generated_turn_checkpoint_with_rich_text(
    prior_state_bytes: &[u8],
    user_text: &str,
    user_rich_text: Option<&str>,
    message_id: &str,
    request_id: Option<&str>,
    generated_step_bytes: &[Vec<u8>],
    assistant_content: &str,
) -> TextTurnCheckpointArtifacts {
    let prior_root_id = (!prior_state_bytes.is_empty())
        .then(|| Sha256::digest(prior_state_bytes).to_vec());

    let mut user_message_bytes = Vec::new();
    push_length_delimited(1, user_text.as_bytes(), &mut user_message_bytes);
    push_length_delimited(2, message_id.as_bytes(), &mut user_message_bytes);
    if let Some(user_rich_text) = user_rich_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_length_delimited(8, user_rich_text.as_bytes(), &mut user_message_bytes);
    }
    if let Some(prior_root_id) = prior_root_id.as_deref() {
        push_length_delimited(10, prior_root_id, &mut user_message_bytes);
    }
    let user_message_id = Sha256::digest(&user_message_bytes).to_vec();

    let mut step_bytes = generated_step_bytes.to_vec();
    let mut step_ids = step_bytes
        .iter()
        .map(|step| Sha256::digest(step).to_vec())
        .collect::<Vec<_>>();
    if !assistant_content.is_empty() {
        let mut assistant_message = Vec::new();
        push_length_delimited(1, assistant_content.as_bytes(), &mut assistant_message);
        let mut step = Vec::new();
        push_length_delimited(1, &assistant_message, &mut step);
        step_ids.push(Sha256::digest(&step).to_vec());
        step_bytes.push(step);
    }

    let mut agent_turn = Vec::new();
    push_length_delimited(1, &user_message_id, &mut agent_turn);
    for step_id in &step_ids {
        push_length_delimited(2, step_id, &mut agent_turn);
    }
    if let Some(request_id) = request_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_length_delimited(3, request_id.as_bytes(), &mut agent_turn);
    }

    let mut turn_bytes = Vec::new();
    push_length_delimited(1, &agent_turn, &mut turn_bytes);
    let turn_id = Sha256::digest(&turn_bytes).to_vec();

    // Preserve the exact prior ConversationStateStructure wire image,
    // including fields this migration has not decoded yet, and append one
    // legal repeated turns field (field 8).
    let mut state_bytes = prior_state_bytes.to_vec();
    push_length_delimited(8, &turn_id, &mut state_bytes);

    TextTurnCheckpointArtifacts {
        prior_state_bytes: prior_state_bytes.to_vec(),
        user_message_id,
        user_message_bytes,
        step_ids,
        step_bytes,
        turn_id,
        turn_bytes,
        state_bytes,
    }
}

pub struct ProductionAgentStateCheckpointSink {
    agent_id: String,
    agent_store: Arc<ProductionAgentStore>,
    blob_store: Arc<ProductionWorkerBlobStore>,
    transcript_mirror: Arc<ProductionRoutedTranscriptMirror>,
    transcript_persistence_enabled: bool,
    prior_state_bytes: Mutex<Vec<u8>>,
    token_details_persistence: Mutex<TokenDetailsPersistenceTracker>,
}

impl ProductionAgentStateCheckpointSink {
    pub fn new(
        agent_id: impl Into<String>,
        agent_store: Arc<ProductionAgentStore>,
        blob_store: Arc<ProductionWorkerBlobStore>,
        transcript_mirror: Arc<ProductionRoutedTranscriptMirror>,
        prior_state_bytes: Vec<u8>,
        transcript_persistence_enabled: bool,
    ) -> Result<Self, String> {
        let agent_id = agent_id.into();
        let prior = ProductionTranscriptCheckpoint::from_state_bytes(&prior_state_bytes)?;
        let observed_summary_archive_count = if prior_state_bytes.is_empty() {
            0
        } else {
            decode_conversation_state_recovery_fields(&prior_state_bytes)
                .map_err(|error| format!(
                    "Runner Agent could not decode base ConversationStateStructure for turn settlement: {error}"
                ))?
                .summary_archives
                .len()
        };
        transcript_mirror.recover(&agent_id, &prior, &blob_store)?;
        Ok(Self {
            agent_id,
            agent_store,
            blob_store,
            transcript_mirror,
            transcript_persistence_enabled,
            prior_state_bytes: Mutex::new(prior_state_bytes),
            token_details_persistence: Mutex::new(TokenDetailsPersistenceTracker::new(
                observed_summary_archive_count,
            )),
        })
    }

    fn persist_blob(
        &self,
        id: &[u8],
        bytes: &[u8],
    ) -> Result<(), ProviderSessionError> {
        self.blob_store
            .set_blob_blocking(id, bytes)
            .map_err(|error| {
                ProviderSessionError::Protocol(format!(
                    "Runner Agent checkpoint blob write failed: {error}"
                ))
            })
    }
}

impl AgentStateCheckpointSink for ProductionAgentStateCheckpointSink {
    fn base_state_bytes(&self) -> Result<Vec<u8>, ProviderSessionError> {
        self.prior_state_bytes
            .lock()
            .map(|prior| prior.clone())
            .map_err(|_| {
                ProviderSessionError::Protocol(
                    "Runner Agent checkpoint prior state is poisoned".into(),
                )
            })
    }

    fn read_state_blob(
        &self,
        blob_id: &[u8],
    ) -> Result<Option<Vec<u8>>, ProviderSessionError> {
        self.agent_store.get_blob(blob_id).map_err(|error| {
            ProviderSessionError::Protocol(format!(
                "Runner Agent checkpoint blob read failed: {error}"
            ))
        })
    }

    fn stage_text_turn(
        &self,
        messages: &[ProviderMessage],
        options: &TurnRunOptions,
        assistant_content: &str,
    ) -> Result<TextTurnCheckpointArtifacts, ProviderSessionError> {
        self.stage_generated_turn(messages, options, &[], assistant_content)
    }

    fn stage_generated_turn(
        &self,
        messages: &[ProviderMessage],
        options: &TurnRunOptions,
        generated_step_bytes: &[Vec<u8>],
        assistant_content: &str,
    ) -> Result<TextTurnCheckpointArtifacts, ProviderSessionError> {
        let user_text = options
            .recent_message_text
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                messages
                    .iter()
                    .rev()
                    .find(|message| {
                        message.role == "user" && !message.content.trim().is_empty()
                    })
                    .map(|message| message.content.as_str())
            })
            .ok_or_else(|| {
                ProviderSessionError::Protocol(
                    "Runner Agent checkpoint requires the current user message".into(),
                )
            })?;
        let message_id = options
            .message_id
            .as_deref()
            .or(options.inference_request_id.as_deref())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Protocol(
                    "Runner Agent checkpoint requires messageId or requestId".into(),
                )
            })?;

        let mut prior = self
            .prior_state_bytes
            .lock()
            .map_err(|_| {
                ProviderSessionError::Protocol(
                    "Runner Agent checkpoint prior state is poisoned".into(),
                )
            })?;
        let live_prior = self.agent_store.latest_checkpoint_bytes().unwrap_or_default();
        if live_prior != *prior {
            return Err(ProviderSessionError::Protocol(
                "Runner Agent checkpoint root changed while this turn was active".into(),
            ));
        }

        let mut artifacts = build_generated_turn_checkpoint_with_rich_text(
            &prior,
            user_text,
            options.recent_message_rich_text.as_deref(),
            message_id,
            options.inference_request_id.as_deref(),
            generated_step_bytes,
            assistant_content,
        );
        let turn_ended_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
            .unwrap_or_default();
        let mut token_details_persistence = self
            .token_details_persistence
            .lock()
            .map_err(|_| ProviderSessionError::Protocol(
                "Runner Agent token-details settlement state is poisoned".into(),
            ))?;
        artifacts.state_bytes = prepare_settled_checkpoint_state(
            &artifacts.state_bytes,
            &mut token_details_persistence,
            options.turn_started_at_ms,
            turn_ended_at_ms,
        )?;
        Ok(artifacts)
    }

    fn persist_staged_text_turn(
        &self,
        options: &TurnRunOptions,
        artifacts: &TextTurnCheckpointArtifacts,
    ) -> Result<(), ProviderSessionError> {
        let mut prior = self
            .prior_state_bytes
            .lock()
            .map_err(|_| {
                ProviderSessionError::Protocol(
                    "Runner Agent checkpoint prior state is poisoned".into(),
                )
            })?;
        let live_prior = self.agent_store.latest_checkpoint_bytes().unwrap_or_default();
        if live_prior != *prior || artifacts.prior_state_bytes != *prior {
            return Err(ProviderSessionError::Protocol(
                "Runner Agent checkpoint root changed while this turn was active".into(),
            ));
        }

        self.persist_blob(
            &artifacts.user_message_id,
            &artifacts.user_message_bytes,
        )?;
        for (id, bytes) in artifacts.step_ids.iter().zip(&artifacts.step_bytes) {
            self.persist_blob(id, bytes)?;
        }
        self.persist_blob(&artifacts.turn_id, &artifacts.turn_bytes)?;

        let checkpoint =
            ProductionTranscriptCheckpoint::from_state_bytes(&artifacts.state_bytes)
                .map_err(|error| {
                    ProviderSessionError::Protocol(format!(
                        "Runner produced invalid ConversationStateStructure: {error}"
                    ))
                })?;
        let mirror_prepared = self.transcript_persistence_enabled;
        if mirror_prepared {
            self.transcript_mirror
                .prepare_checkpoint(
                    &self.agent_id,
                    &checkpoint,
                    &self.blob_store,
                    true,
                    true,
                )
                .map_err(|error| {
                    ProviderSessionError::Protocol(format!(
                        "Runner Agent checkpoint transaction failed: transcript mirror prepare failed: {error}"
                    ))
                })?;
        }

        let durable = if let Some(confirmed_message_id) = options
            .message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.agent_store
                .handle_checkpoint_bytes_and_confirm_user_message_blocking(
                    &checkpoint.state_bytes,
                    confirmed_message_id,
                )
                .map(|_| ())
        } else {
            self.agent_store
                .handle_checkpoint_bytes_blocking(&checkpoint.state_bytes)
                .map(|_| ())
        };

        if let Err(error) = durable {
            if mirror_prepared {
                let _ = self.transcript_mirror.abort_checkpoint(&self.agent_id);
            }
            return Err(ProviderSessionError::Protocol(format!(
                "Runner Agent checkpoint transaction failed: durable Agent checkpoint failed: {error}"
            )));
        }

        if mirror_prepared {
            let root = self.agent_store.latest_root_blob_id();
            self.transcript_mirror
                .commit_checkpoint(&self.agent_id, &root)
                .map_err(|error| {
                    ProviderSessionError::Protocol(format!(
                        "Runner Agent checkpoint transaction failed: transcript mirror commit failed after durable checkpoint: {error}"
                    ))
                })?;
        } else {
            self.transcript_mirror
                .skip_checkpoint(
                    &self.agent_id,
                    &checkpoint,
                    &self.blob_store,
                )
                .map_err(|error| {
                    ProviderSessionError::Protocol(format!(
                        "Runner Agent checkpoint transaction failed: transcript mirror skip failed: {error}"
                    ))
                })?;
        }
        *prior = artifacts.state_bytes.clone();
        Ok(())
    }
}


pub fn prepare_settled_checkpoint_state(
    state_bytes: &[u8],
    token_details_persistence: &mut TokenDetailsPersistenceTracker,
    turn_started_at_ms: Option<u64>,
    turn_ended_at_ms: u64,
) -> Result<Vec<u8>, ProviderSessionError> {
    let mut settled = state_bytes.to_vec();
    if let Some(turn_started_at_ms) = turn_started_at_ms {
        append_turn_timing(
            &mut settled,
            turn_ended_at_ms.saturating_sub(turn_started_at_ms),
            turn_ended_at_ms,
        );
    }
    let recovery = decode_conversation_state_recovery_fields(&settled)
        .map_err(|error| ProviderSessionError::Protocol(format!(
            "Runner Agent could not decode settled ConversationStateStructure: {error}"
        )))?;
    let token_details = recovery.token_details.map(|details| SettledTokenDetails {
        used_tokens: details.used_tokens,
        max_tokens: details.max_tokens,
    });
    if token_details_persistence.should_suppress(
        recovery.summary_archives.len(),
        token_details,
    ) && recovery.token_details.is_some()
    {
        settled = remove_conversation_state_token_details(&settled)
            .map_err(|error| ProviderSessionError::Protocol(format!(
                "Runner Agent could not suppress stale token details: {error}"
            )))?;
    }
    Ok(settled)
}

fn append_turn_timing(
    state_bytes: &mut Vec<u8>,
    duration_ms: u64,
    timestamp_ms: u64,
) {
    let mut timing = Vec::new();
    push_varint(1_u64 << 3, &mut timing);
    push_varint(duration_ms, &mut timing);
    push_varint(2_u64 << 3, &mut timing);
    push_varint(timestamp_ms, &mut timing);
    push_length_delimited(14, &timing, state_bytes);
}

fn push_length_delimited(field_number: u64, value: &[u8], output: &mut Vec<u8>) {
    push_varint((field_number << 3) | 2, output);
    push_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn push_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}
