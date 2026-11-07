use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

use crate::runner::RecoveryUserMessage;

use super::box_request_entries::{
    ActiveBoxRequest, BoxRequestTrackDecision,
    track_box_request_entry as decide_box_request_tracking,
};
use super::prompt_acceptance_ledger::{
    AcceptanceRecord, PromptAcceptanceError, PromptAcceptanceLedger, SendAdmission, SendInput,
    send_input_digest,
};
use super::send_message_shaping::create_send_message_entry;
use super::send_thread_stamping::{apply_auto_reply_thread, validate_ai_reply_target};
use super::transcript_entry_ids::{TranscriptEntryIdKind, next_entry_id};

pub const HOST_ACCOUNT_SLOT: &str = "host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendBegin {
    EmptyNoop,
    Dispatch {
        client_nonce: Option<String>,
        input_digest: Option<String>,
    },
    Coalesced {
        client_nonce: String,
    },
    DuplicateNoop {
        record: AcceptanceRecord,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendEchoIdentity {
    pub agent_id: String,
    pub echo_entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedAcceptedEcho {
    pub entry: Value,
    pub is_on_active_transcript: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersistedSendContext {
    pub echo_entry_id: Option<String>,
    pub user_message_id: Option<String>,
    pub recent_user_messages: Vec<RecoveryUserMessage>,
    /// Echoes created by this exact durable admission. Duplicate/replayed
    /// admissions intentionally leave this empty so Host does not re-emit.
    pub accepted_echoes: Vec<PersistedAcceptedEcho>,
    /// Frozen send-acceptance roster side effects require a refresh when the
    /// first default profile is seeded or an awaiting-user state is cleared.
    pub needs_roster_refresh: bool,
    /// True only when this invocation actually applied the send-acceptance
    /// persistence/roster side effects rather than replaying a prior nonce.
    pub acceptance_effects_applied: bool,
    /// The addressed echo append completed durably.
    pub accepted_durably: bool,
}

impl PersistedSendContext {
    pub fn mark_accepted_echoes_on_active_transcript(&mut self, is_active: bool) {
        for echo in &mut self.accepted_echoes {
            echo.is_on_active_transcript = is_active;
        }
    }
}

impl From<Option<String>> for PersistedSendContext {
    fn from(echo_entry_id: Option<String>) -> Self {
        Self {
            echo_entry_id,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverySend {
    pub epoch: u64,
    pub message_id: String,
    pub recent_user_messages: Vec<RecoveryUserMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InFlightSend {
    digest: String,
}

#[derive(Debug, Default)]
pub struct SendPipelineState {
    in_flight: HashMap<String, InFlightSend>,
    turn_epochs: HashMap<String, u64>,
    latest_recovery_sends: HashMap<String, RecoverySend>,
    recovery_break_epochs: HashMap<String, u64>,
    attachment_batch_ids: HashMap<String, String>,
    active_box_request: Option<ActiveBoxRequest>,
}

impl SendPipelineState {
    pub fn begin_send(
        &mut self,
        ledger: &mut PromptAcceptanceLedger,
        input: &SendInput,
        client_nonce: Option<&str>,
    ) -> Result<SendBegin, PromptAcceptanceError> {
        let has_prompt = !input.prompt.trim().is_empty();
        if !has_prompt && input.attachment_paths.is_empty() {
            return Ok(SendBegin::EmptyNoop);
        }

        let nonce = client_nonce
            .filter(|nonce| !nonce.is_empty())
            .map(str::to_owned);
        let Some(nonce) = nonce else {
            return Ok(SendBegin::Dispatch {
                client_nonce: None,
                input_digest: None,
            });
        };

        let digest = send_input_digest(input);
        if let Some(in_flight) = self.in_flight.get(&nonce) {
            // Only a byte-for-byte equivalent admission may share an in-flight
            // nonce. Different Agent/session or message content must fail
            // closed instead of inheriting the first send's settlement.
            if in_flight.digest != digest {
                return Err(PromptAcceptanceError::DigestMismatch {
                    client_nonce: nonce,
                });
            }
            return Ok(SendBegin::Coalesced {
                client_nonce: nonce,
            });
        }
        match ledger.admit_send(HOST_ACCOUNT_SLOT, &nonce, &digest)? {
            SendAdmission::Duplicate(record) => Ok(SendBegin::DuplicateNoop { record }),
            SendAdmission::Dispatch => {
                self.in_flight.insert(
                    nonce.clone(),
                    InFlightSend {
                        digest: digest.clone(),
                    },
                );
                Ok(SendBegin::Dispatch {
                    client_nonce: Some(nonce),
                    input_digest: Some(digest),
                })
            }
        }
    }

    pub fn record_pending_acceptance(
        &mut self,
        ledger: &mut PromptAcceptanceLedger,
        client_nonce: &str,
        echo: SendEchoIdentity,
    ) -> Result<AcceptanceRecord, PromptAcceptanceError> {
        let digest = self
            .in_flight
            .get(client_nonce)
            .map(|send| send.digest.clone())
            .ok_or_else(|| PromptAcceptanceError::InvalidPendingIdentity)?;
        ledger.record_pending(
            HOST_ACCOUNT_SLOT,
            client_nonce,
            digest,
            echo.agent_id,
            echo.echo_entry_id,
        )
    }

    pub fn mark_send_accepted(
        &mut self,
        ledger: &mut PromptAcceptanceLedger,
        client_nonce: Option<&str>,
    ) {
        if let Some(client_nonce) = client_nonce {
            ledger.mark_accepted(HOST_ACCOUNT_SLOT, client_nonce);
        }
    }

    pub fn mark_send_rejected(
        &mut self,
        ledger: &mut PromptAcceptanceLedger,
        client_nonce: Option<&str>,
        rejection_code: impl Into<String>,
    ) {
        if let Some(client_nonce) = client_nonce {
            ledger.mark_rejected(HOST_ACCOUNT_SLOT, client_nonce, rejection_code);
        }
    }

    pub fn finish_send(
        &mut self,
        ledger: &mut PromptAcceptanceLedger,
        client_nonce: Option<&str>,
        succeeded: bool,
    ) {
        let Some(client_nonce) = client_nonce else {
            return;
        };
        if !succeeded {
            ledger.clear_unless_accepted(HOST_ACCOUNT_SLOT, client_nonce);
        }
        self.in_flight.remove(client_nonce);
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_in_flight(&self, client_nonce: &str) -> bool {
        self.in_flight.contains_key(client_nonce)
    }

    pub fn next_turn_epoch(&mut self, agent_id: &str) -> u64 {
        let epoch = self.turn_epochs.entry(agent_id.to_string()).or_insert(0);
        *epoch = epoch.saturating_add(1);
        *epoch
    }

    pub fn current_turn_epoch(&self, agent_id: &str) -> u64 {
        self.turn_epochs.get(agent_id).copied().unwrap_or(0)
    }

    pub fn register_recovery_turn(
        &mut self,
        agent_id: &str,
        epoch: u64,
        context: &PersistedSendContext,
        is_fork: bool,
    ) {
        let message_id = context
            .user_message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if !is_fork && !context.recent_user_messages.is_empty() {
            if let Some(message_id) = message_id {
                self.latest_recovery_sends.insert(
                    agent_id.to_string(),
                    RecoverySend {
                        epoch,
                        message_id: message_id.to_string(),
                        recent_user_messages: context.recent_user_messages.clone(),
                    },
                );
                return;
            }
        }
        self.recovery_break_epochs.insert(agent_id.to_string(), epoch);
    }

    pub fn latest_recovery_send(&self, agent_id: &str) -> Option<&RecoverySend> {
        self.latest_recovery_sends.get(agent_id)
    }

    pub fn recovery_break_epoch(&self, agent_id: &str) -> u64 {
        self.recovery_break_epochs
            .get(agent_id)
            .copied()
            .unwrap_or_default()
    }

    pub fn claim_attachment_batch_id(&mut self, agent_id: &str) -> String {
        self.attachment_batch_ids
            .entry(agent_id.to_string())
            .or_insert_with(|| Uuid::new_v4().to_string())
            .clone()
    }

    pub fn clear_attachment_batch_id(&mut self, agent_id: &str) {
        self.attachment_batch_ids.remove(agent_id);
    }

    pub fn prepare_generated_send_message_entry(
        &mut self,
        agent_id: &str,
        entries: &[Value],
        message: &Value,
        reply_thread_target: Option<&str>,
        is_fork: bool,
        timestamp_ms: u64,
    ) -> Value {
        let entry_id = next_entry_id(entries, TranscriptEntryIdKind::SendMessage);
        let validated = validate_ai_reply_target(message, Some(&entry_id), entries);
        let threaded = apply_auto_reply_thread(&validated, reply_thread_target, entries);
        let attachment_batch_id = (threaded.get("type").and_then(Value::as_str)
            == Some("attachment"))
            .then(|| self.claim_attachment_batch_id(agent_id));
        let mut entry = create_send_message_entry(entry_id, threaded, timestamp_ms as f64);
        if let Some(object) = entry.as_object_mut() {
            if let Some(batch_id) = attachment_batch_id {
                object.insert("batchId".into(), Value::String(batch_id));
            }
            if is_fork {
                object.insert("branched".into(), Value::Bool(true));
            }
        }
        entry
    }

    pub fn track_box_request_entry(
        &mut self,
        agent_id: &str,
        entry: &Value,
    ) -> BoxRequestTrackDecision {
        let decision = decide_box_request_tracking(
            self.active_box_request.as_ref(),
            agent_id,
            entry,
        );
        if let Some(next) = decision.next.as_ref() {
            self.active_box_request = Some(next.clone());
        }
        decision
    }

    pub fn resolve_box_request_tracking(&mut self, request_id: &str) -> bool {
        let matches = self
            .active_box_request
            .as_ref()
            .is_some_and(|active| active.request_id == request_id);
        if matches {
            self.active_box_request = None;
        }
        matches
    }

    pub fn active_box_request(&self) -> Option<&ActiveBoxRequest> {
        self.active_box_request.as_ref()
    }
}
