use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::thread;

use prost::Message;

use crate::cursor_backend::{resolve_sand_ghost_mode_header, send_cursor_unary};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;

use super::generated_inference_codec::{
    AgentFollowupCategorizationRequest, AgentPostTurnLabelingRequest, core_message_to_proto,
};
use super::provider_session::ProviderMessage;

pub const SAND_AGENT_MODE: &str = "sand-agent";
pub const SAND_SUMMARIZATION_REASON: &str = "agent-summarization";
pub const RECORD_AGENT_FOLLOWUP_CLASSIFICATION_PATH: &str =
    "/aiserver.v1.InferenceService/RecordAgentFollowupClassification";
pub const RECORD_AGENT_POST_TURN_LABELING_PATH: &str =
    "/aiserver.v1.InferenceService/RecordAgentPostTurnLabeling";
pub const LABELING_RPC_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LabelMessage {
    pub inference_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowupClassification {
    pub request_id: String,
    pub replying_to_request_id: String,
    pub conversation_id: String,
    pub agent_mode: &'static str,
    pub model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostTurnLabeling {
    pub request_id: String,
    pub conversation_id: String,
    pub agent_mode: &'static str,
    pub model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelingDiagnostic {
    pub kind: &'static str,
    pub stage: &'static str,
    pub error_class: String,
}

pub type LabelingDiagnosticSink = Arc<dyn Fn(LabelingDiagnostic) + Send + Sync>;

fn default_diagnostic_sink() -> LabelingDiagnosticSink {
    Arc::new(|diagnostic| {
        eprintln!(
            "mahayana-host labeling_failed stage={} errorClass={}",
            diagnostic.stage, diagnostic.error_class
        );
    })
}

fn error_class(error: &str) -> String {
    error
        .split(':')
        .next()
        .unwrap_or("unknown")
        .trim()
        .chars()
        .take(80)
        .collect()
}

#[derive(Clone)]
pub struct SandLabelingClient {
    auth: Arc<HostAuthExtension>,
    diagnostic_sink: LabelingDiagnosticSink,
}

impl SandLabelingClient {
    pub fn production(auth: Arc<HostAuthExtension>) -> Self {
        Self {
            auth,
            diagnostic_sink: default_diagnostic_sink(),
        }
    }

    pub fn with_diagnostic_sink(
        auth: Arc<HostAuthExtension>,
        diagnostic_sink: LabelingDiagnosticSink,
    ) -> Self {
        Self {
            auth,
            diagnostic_sink,
        }
    }

    fn send(&self, path: &str, body: Vec<u8>) -> Result<(), String> {
        let backend_url =
            get_configured_backend_url().map_err(|error| error.to_string())?;
        let access_token = self
            .auth
            .get_access_token()
            .map_err(|error| error.to_string())?;
        let machine_id = self
            .auth
            .get_machine_id()
            .map_err(|error| error.to_string())?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&backend_url, &access_token, &machine_id);
        send_cursor_unary(
            &backend_url,
            &access_token,
            &machine_id,
            path,
            &body,
            LABELING_RPC_TIMEOUT_MS,
            ghost_mode,
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn report_error(&self, stage: &'static str, error: &str) {
        (self.diagnostic_sink)(LabelingDiagnostic {
            kind: "labeling_failed",
            stage,
            error_class: error_class(error),
        });
    }

    pub fn record_followup_classification(
        &self,
        classification: &FollowupClassification,
        messages: &[ProviderMessage],
    ) -> Result<(), String> {
        let request = AgentFollowupCategorizationRequest {
            request_id: classification.request_id.clone(),
            replying_to_request_id: classification.replying_to_request_id.clone(),
            messages: messages.iter().map(core_message_to_proto).collect(),
            conversation_id: Some(classification.conversation_id.clone()),
            agent_mode: Some(classification.agent_mode.to_string()),
            model_name: Some(classification.model_name.clone()),
        };
        self.send(RECORD_AGENT_FOLLOWUP_CLASSIFICATION_PATH, request.encode_to_vec())
    }

    pub fn record_post_turn_labeling(
        &self,
        labeling: &PostTurnLabeling,
        messages: &[ProviderMessage],
    ) -> Result<(), String> {
        let request = AgentPostTurnLabelingRequest {
            request_id: labeling.request_id.clone(),
            messages: messages.iter().map(core_message_to_proto).collect(),
            conversation_id: Some(labeling.conversation_id.clone()),
            agent_mode: Some(labeling.agent_mode.to_string()),
            model_name: Some(labeling.model_name.clone()),
        };
        self.send(RECORD_AGENT_POST_TURN_LABELING_PATH, request.encode_to_vec())
    }

    pub fn spawn_followup_classification(
        &self,
        classification: FollowupClassification,
        messages: Vec<ProviderMessage>,
    ) {
        let client = self.clone();
        let _ = thread::Builder::new()
            .name("sand-followup-labeling".into())
            .spawn(move || {
                if let Err(error) =
                    client.record_followup_classification(&classification, &messages)
                {
                    client.report_error("followup_classification", &error);
                }
            });
    }

    pub fn spawn_post_turn_labeling(
        &self,
        labeling: PostTurnLabeling,
        messages: Vec<ProviderMessage>,
    ) {
        let client = self.clone();
        let _ = thread::Builder::new()
            .name("sand-post-turn-labeling".into())
            .spawn(move || {
                if let Err(error) = client.record_post_turn_labeling(&labeling, &messages) {
                    client.report_error("post_turn_labeling", &error);
                }
            });
    }
}

#[derive(Debug, Default)]
pub struct FollowupLabelingTracker {
    last_request_id_by_conversation: BTreeMap<String, String>,
    recorded_request_ids: BTreeSet<String>,
}

impl FollowupLabelingTracker {
    pub fn observe_stream(
        &mut self,
        conversation_id: &str,
        request_id: &str,
        model_name: &str,
        messages: &[LabelMessage],
    ) -> Option<FollowupClassification> {
        if conversation_id.is_empty()
            || request_id.is_empty()
            || messages.is_empty()
            || messages.iter().any(|message| {
                message.inference_reason.as_deref() == Some(SAND_SUMMARIZATION_REASON)
            })
        {
            return None;
        }

        let previous_request_id = self
            .last_request_id_by_conversation
            .insert(conversation_id.to_string(), request_id.to_string());

        let previous_request_id = previous_request_id?;
        if previous_request_id.is_empty()
            || previous_request_id == request_id
            || self.recorded_request_ids.contains(request_id)
        {
            return None;
        }

        self.recorded_request_ids.insert(request_id.to_string());
        Some(FollowupClassification {
            request_id: request_id.to_string(),
            replying_to_request_id: previous_request_id,
            conversation_id: conversation_id.to_string(),
            agent_mode: SAND_AGENT_MODE,
            model_name: model_name.to_string(),
        })
    }

    pub fn last_request_id(&self, conversation_id: &str) -> Option<&str> {
        self.last_request_id_by_conversation
            .get(conversation_id)
            .map(String::as_str)
    }
}

pub fn prepare_post_turn_labeling(
    conversation_id: &str,
    request_id: &str,
    model_name: &str,
    messages: &[LabelMessage],
) -> Option<PostTurnLabeling> {
    if messages.is_empty() || request_id.trim().is_empty() || conversation_id.trim().is_empty() {
        return None;
    }

    Some(PostTurnLabeling {
        request_id: request_id.to_string(),
        conversation_id: conversation_id.to_string(),
        agent_mode: SAND_AGENT_MODE,
        model_name: model_name.to_string(),
    })
}
