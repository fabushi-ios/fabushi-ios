use crate::actor::ActorId;
use crate::blob_store::{BlobStoreError, FileBlobStore};
use crate::engine::{Command, EngineError, Event, MessagingEngine};
use crate::message::{
    ClientMessageId, DeliveryState, MessageId,
};
use crate::payment::Money;
use crate::protocol::{
    ClientCommand, ClientEnvelope, ServerEnvelope, ServerEvent, FABUSHI_MESSAGING_PROTOCOL_VERSION,
};
use crate::settlement::{SettlementError, SettlementVerifier, SignedSettlement};
use crate::store::{JournalEntry, MessagingSnapshot, MessagingStateStore, StoreError};
use crate::wallet::LedgerEntry;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use thiserror::Error;

pub(crate) const TYPING_TTL_MS: i64 = 5_000;

#[derive(Debug, Error)]
pub enum MessagingServiceError {
    #[error("unsupported messaging protocol version {actual}; expected {expected}")]
    ProtocolVersion { expected: u16, actual: u16 },
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Blob(#[from] BlobStoreError),
    #[error("blob storage is unavailable for this messaging service")]
    BlobStoreUnavailable,
    #[error("blob chunk is not valid base64: {0}")]
    InvalidBlobBase64(String),
    #[error("messaging service invariant failed: {0}")]
    Invariant(String),
    #[error("messaging command is not authorized for the authenticated actor: {0}")]
    UnauthorizedCommand(String),
    #[error("client message id {0} was replayed with conflicting message content")]
    IdempotencyConflict(String),
    #[error(transparent)]
    Settlement(#[from] SettlementError),
}

pub(crate) fn sanitize_invocation_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':') {
                character
            } else {
                '_'
            }
        })
        .take(160)
        .collect()
}

pub(crate) fn stable_message_id(actor_id: &ActorId, client_message_id: &ClientMessageId) -> MessageId {
    let mut hasher = Sha256::new();
    hasher.update(actor_id.0.as_bytes());
    hasher.update([0]);
    hasher.update(client_message_id.0.as_bytes());
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(encoded, "{byte:02x}");
    }
    MessageId::new(format!("msg:{encoded}"))
}

pub struct MessagingService<S: MessagingStateStore> {
    pub(crate) engine: MessagingEngine,
    pub(crate) store: S,
    pub(crate) blob_store: Option<FileBlobStore>,
    pub(crate) cursor: u64,
}

impl<S: MessagingStateStore> MessagingService<S> {
    pub fn load(store: S) -> Result<Self, MessagingServiceError> {
        let snapshot = store.load()?;
        let (engine, cursor) = match snapshot {
            Some(snapshot) => (MessagingEngine::from_state(snapshot.state), snapshot.cursor),
            None => (MessagingEngine::new(), 0),
        };
        Ok(Self {
            engine,
            store,
            blob_store: None,
            cursor,
        })
    }

    pub fn load_with_blob_store(
        store: S,
        blob_store: FileBlobStore,
    ) -> Result<Self, MessagingServiceError> {
        let mut service = Self::load(store)?;
        service.blob_store = Some(blob_store);
        Ok(service)
    }

    pub fn engine(&self) -> &MessagingEngine {
        &self.engine
    }

    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    pub fn into_store(self) -> S {
        self.store
    }

    pub fn handle(
        &mut self,
        envelope: ClientEnvelope,
        server_time_ms: i64,
    ) -> Result<Vec<ServerEnvelope>, MessagingServiceError> {
        if envelope.protocol_version != FABUSHI_MESSAGING_PROTOCOL_VERSION {
            return Err(MessagingServiceError::ProtocolVersion {
                expected: FABUSHI_MESSAGING_PROTOCOL_VERSION,
                actual: envelope.protocol_version,
            });
        }

        let actor_id = envelope.context.actor_id;
        let command = envelope.command;
        self.validate_command_authorization(&actor_id, &command)?;
        match command {
            ClientCommand::BeginBlobUpload { metadata } => {
                let status = self.blob_store()?.begin_upload(&metadata)?;
                self.single_service_event(
                    &actor_id,
                    ServerEvent::BlobUploadChanged { status },
                    server_time_ms,
                )
            }
            ClientCommand::AppendBlobChunk {
                blob_id,
                offset,
                data_base64,
            } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data_base64.as_bytes())
                    .map_err(|error| MessagingServiceError::InvalidBlobBase64(error.to_string()))?;
                let status = self.blob_store()?.append_chunk(&blob_id, offset, &bytes)?;
                self.single_service_event(
                    &actor_id,
                    ServerEvent::BlobUploadChanged { status },
                    server_time_ms,
                )
            }
            ClientCommand::FinishBlobUpload { blob_id } => {
                let metadata = self.blob_store()?.finish_upload(&blob_id)?;
                self.single_service_event(
                    &actor_id,
                    ServerEvent::BlobReady { metadata },
                    server_time_ms,
                )
            }
            ClientCommand::DeleteBlob { blob_id } => {
                self.blob_store()?.delete(&blob_id)?;
                self.single_service_event(
                    &actor_id,
                    ServerEvent::BlobDeleted { blob_id },
                    server_time_ms,
                )
            }
            ClientCommand::WalletStatus => {
                Ok(vec![self.wallet_status_envelope(&actor_id, server_time_ms)])
            }
            ClientCommand::Sync { cursor, limit } => {
                self.mark_direct_messages_delivered(&actor_id, server_time_ms)?;
                self.sync_response(&actor_id, cursor.as_deref(), limit, server_time_ms)
            }
            ClientCommand::Search { query } => {
                Ok(vec![self.search_envelope(&actor_id, query, server_time_ms)])
            }
            ClientCommand::SearchRecipients {
                query,
                requirements,
            } => Ok(vec![self.recipient_search_envelope(
                &actor_id,
                query,
                requirements,
                server_time_ms,
            )]),
            ClientCommand::ListCommunityMembers {
                conversation_id,
                cursor,
                limit,
            } => Ok(vec![self.community_members_page(
                &actor_id,
                conversation_id,
                cursor.as_deref(),
                limit,
                server_time_ms,
            )?]),
            ClientCommand::ListCommunityAuditLog {
                conversation_id,
                cursor,
                limit,
            } => Ok(vec![self.community_audit_page(
                &actor_id,
                conversation_id,
                cursor.as_deref(),
                limit,
                server_time_ms,
            )?]),
            ClientCommand::SetPresence { presence } => {
                self.set_presence_and_release_triggers(&actor_id, presence, server_time_ms)
            }
            ClientCommand::StartTyping {
                conversation_id,
                action,
            } => self.typing_event(
                &actor_id,
                conversation_id,
                Some(action),
                Some(server_time_ms.saturating_add(TYPING_TTL_MS)),
                server_time_ms,
            ),
            ClientCommand::StopTyping { conversation_id } => self.typing_event(
                &actor_id,
                conversation_id,
                None,
                Some(server_time_ms.saturating_add(TYPING_TTL_MS)),
                server_time_ms,
            ),
            command => {
                if let Some(replay) =
                    self.idempotent_send_replay(&actor_id, &command, server_time_ms)?
                {
                    return Ok(replay);
                }

                let commands = self.project_command(&actor_id, command, server_time_ms);
                let mut events = Vec::new();
                for command in commands {
                    events.extend(self.engine.execute(command)?);
                }
                if events.is_empty() {
                    return Ok(Vec::new());
                }

                let bot_invocations = events
                    .iter()
                    .filter_map(|event| match event {
                        Event::MessageQueued { message }
                            if !matches!(message.delivery_state, DeliveryState::Failed { .. }) =>
                        {
                            Some(message)
                        }
                        _ => None,
                    })
                    .flat_map(|message| self.bot_invocations_for_message(message))
                    .collect::<Vec<_>>();
                self.cursor = self.cursor.saturating_add(events.len() as u64);
                let mut responses = events
                    .into_iter()
                    .filter_map(|event| self.project_event(&actor_id, event, server_time_ms))
                    .collect::<Vec<_>>();
                responses.extend(
                    bot_invocations
                        .into_iter()
                        .map(|invocation| ServerEnvelope {
                            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
                            cursor: Some(self.cursor.to_string()),
                            server_time_ms,
                            event: ServerEvent::BotInvocationRequested { invocation },
                        }),
                );
                let journal = self.journal_entries(&actor_id, &responses);
                self.persist_with_events(server_time_ms, &journal)?;
                Ok(responses)
            }
        }
    }


    pub(crate) fn blob_store(&self) -> Result<&FileBlobStore, MessagingServiceError> {
        self.blob_store
            .as_ref()
            .ok_or(MessagingServiceError::BlobStoreUnavailable)
    }

    pub(crate) fn single_service_event(
        &mut self,
        actor_id: &ActorId,
        event: ServerEvent,
        server_time_ms: i64,
    ) -> Result<Vec<ServerEnvelope>, MessagingServiceError> {
        self.cursor = self.cursor.saturating_add(1);
        let response = ServerEnvelope {
            protocol_version: FABUSHI_MESSAGING_PROTOCOL_VERSION,
            cursor: Some(self.cursor.to_string()),
            server_time_ms,
            event,
        };
        let journal = self.journal_entries(actor_id, std::slice::from_ref(&response));
        self.persist_with_events(server_time_ms, &journal)?;
        Ok(vec![response])
    }

    pub fn apply_signed_settlement(
        &mut self,
        verifier: &SettlementVerifier,
        signed: &SignedSettlement,
        server_time_ms: i64,
    ) -> Result<LedgerEntry, MessagingServiceError> {
        let event = verifier.verify(signed, server_time_ms)?;
        self.credit_wallet_from_settlement(
            event.idempotency_key(),
            event.actor_id,
            event.amount,
            Some(event.provider_reference),
            server_time_ms,
        )
    }

    pub fn credit_wallet_from_settlement(
        &mut self,
        request_id: String,
        owner_id: ActorId,
        amount: Money,
        reference: Option<String>,
        server_time_ms: i64,
    ) -> Result<LedgerEntry, MessagingServiceError> {
        let events = self.engine.execute(Command::CreditWalletSettlement {
            request_id,
            owner_id,
            amount,
            reference,
            settled_at_ms: server_time_ms,
        })?;
        let entry = events
            .iter()
            .find_map(|event| match event {
                Event::WalletChanged { entry, .. } => Some(entry.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MessagingServiceError::Invariant(
                    "wallet settlement produced no ledger entry".into(),
                )
            })?;
        self.cursor = self.cursor.saturating_add(events.len() as u64);
        self.persist(server_time_ms)?;
        Ok(entry)
    }


    pub(crate) fn persist_with_events(
        &mut self,
        now_ms: i64,
        events: &[JournalEntry],
    ) -> Result<(), MessagingServiceError> {
        let snapshot = MessagingSnapshot::new(self.engine.state().clone(), self.cursor, now_ms);
        self.store.save_with_events(&snapshot, events)?;
        Ok(())
    }

    pub(crate) fn persist(&mut self, now_ms: i64) -> Result<(), MessagingServiceError> {
        let snapshot = MessagingSnapshot::new(self.engine.state().clone(), self.cursor, now_ms);
        self.store.save(&snapshot)?;
        Ok(())
    }
}
