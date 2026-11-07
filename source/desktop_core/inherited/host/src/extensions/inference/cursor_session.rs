use std::sync::Arc;

use prost::Message;
use serde_json::Value;

use crate::cursor_backend::{resolve_sand_ghost_mode_header, send_cursor_unary};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;

use super::generated_inference_codec::{
    GetSignedUrlForAttachedMediaRequest, GetSignedUrlForAttachedMediaResponse, InferenceReason,
};

use super::sand_model_experiment::{
    SandAgentModelParameter, SandAgentModelSelection,
};

pub const SAND_DEFAULT_MODEL_ID: &str = "grok-4.5";
pub const SAND_SUMMARIZATION_MODEL_ID: &str = "gemini-2.5-flash";
pub const SAND_COMPUTER_USE_SUBAGENT_MODEL_ID: &str = "claude-opus-4-8";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SandSessionOptions {
    pub model_id: Option<String>,
    pub session_model_override: bool,
    pub thinking_level: Option<String>,
    pub is_summarization_session: bool,
    pub is_computer_use_subagent: bool,
    pub is_browser_use_subagent: bool,
    pub request_source: Option<String>,
    pub conversation_id: Option<String>,
    pub inference_reason: Option<InferenceReason>,
    pub lineage: Option<RequestLineage>,
    pub skip_labeling: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLineage {
    pub parent_request_id: String,
    pub root_parent_request_id: String,
    pub parent_agent_tool_call_id: Option<String>,
}

impl RequestLineage {
    pub fn sanitized_headers(&self) -> Vec<(String, String)> {
        let clean = |value: &str| value.replace(['\r', '\n'], "");
        let mut headers = vec![
            ("x-parent-request-id".into(), clean(&self.parent_request_id)),
            (
                "x-root-parent-request-id".into(),
                clean(&self.root_parent_request_id),
            ),
        ];
        if let Some(tool_call_id) = self.parent_agent_tool_call_id.as_deref() {
            headers.push((
                "x-parent-agent-tool-call-id".into(),
                clean(tool_call_id),
            ));
        }
        headers
    }
}

pub const GET_SIGNED_URL_FOR_ATTACHED_MEDIA_PATH: &str =
    "/agent.v1.AgentService/GetSignedUrlForAttachedMedia";
pub const ATTACHED_MEDIA_RPC_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedMediaUrlRequest {
    pub conversation_id: String,
    pub key: String,
    pub mime_type: String,
    pub content_length_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedMediaUrl {
    pub key: String,
    pub put_url: String,
    pub get_url: String,
    pub expires_at_unix_ms: i64,
    pub refresh_after_unix_ms: i64,
}

#[derive(Clone)]
pub struct SandAttachedMediaUrlProvider {
    auth: Arc<HostAuthExtension>,
}

impl SandAttachedMediaUrlProvider {
    pub fn production(auth: Arc<HostAuthExtension>) -> Self {
        Self { auth }
    }

    pub fn get_signed_url(
        &self,
        request: &AttachedMediaUrlRequest,
    ) -> Result<AttachedMediaUrl, String> {
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
        let proto = GetSignedUrlForAttachedMediaRequest {
            key: Some(request.key.clone()),
            mime_type: Some(request.mime_type.clone()),
            conversation_id: request.conversation_id.clone(),
            content_length_bytes: request.content_length_bytes,
        };
        let response = send_cursor_unary(
            &backend_url,
            &access_token,
            &machine_id,
            GET_SIGNED_URL_FOR_ATTACHED_MEDIA_PATH,
            &proto.encode_to_vec(),
            ATTACHED_MEDIA_RPC_TIMEOUT_MS,
            ghost_mode,
        )
        .map_err(|error| error.to_string())?;
        let decoded =
            GetSignedUrlForAttachedMediaResponse::decode(response.as_slice())
                .map_err(|error| error.to_string())?;
        Ok(AttachedMediaUrl {
            key: decoded.key,
            put_url: decoded.put_url,
            get_url: decoded.get_url,
            expires_at_unix_ms: decoded.expires_at_unix_ms,
            refresh_after_unix_ms: decoded.refresh_after_unix_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedModel {
    pub model_id: String,
    pub max_mode: Option<bool>,
    pub parameters: Vec<SandAgentModelParameter>,
}

pub fn sand_default_model_selection() -> SandAgentModelSelection {
    SandAgentModelSelection {
        model_id: SAND_DEFAULT_MODEL_ID.into(),
        max_mode: true,
        parameters: vec![
            SandAgentModelParameter {
                id: "effort".into(),
                value: "high".into(),
            },
            SandAgentModelParameter {
                id: "fast".into(),
                value: "true".into(),
            },
        ],
    }
}

pub fn sand_computer_use_model_selection() -> SandAgentModelSelection {
    SandAgentModelSelection {
        model_id: SAND_COMPUTER_USE_SUBAGENT_MODEL_ID.into(),
        max_mode: false,
        parameters: vec![
            SandAgentModelParameter {
                id: "thinking".into(),
                value: "false".into(),
            },
            SandAgentModelParameter {
                id: "effort".into(),
                value: "low".into(),
            },
        ],
    }
}

fn from_selection(selection: &SandAgentModelSelection) -> RequestedModel {
    RequestedModel {
        model_id: selection.model_id.clone(),
        max_mode: Some(selection.max_mode),
        parameters: selection.parameters.clone(),
    }
}

fn subagent(model_id: &str) -> RequestedModel {
    RequestedModel {
        model_id: model_id.to_string(),
        max_mode: Some(true),
        parameters: Vec::new(),
    }
}

pub struct ResolveRequestedModelInputs<'a> {
    pub session_options: Option<&'a SandSessionOptions>,
    pub env_model_override: Option<&'a str>,
    pub stored_default_model: Option<&'a SandAgentModelSelection>,
    pub stored_computer_use_model: Option<&'a SandAgentModelSelection>,
    pub stored_browser_use_model: Option<&'a SandAgentModelSelection>,
    pub experiment_model_override: Option<&'a SandAgentModelSelection>,
}

pub fn resolve_sand_requested_model(inputs: ResolveRequestedModelInputs<'_>) -> RequestedModel {
    let effective_default = inputs
        .experiment_model_override
        .or(inputs.stored_default_model);
    let effective_default_id = inputs
        .env_model_override
        .or_else(|| effective_default.map(|model| model.model_id.as_str()))
        .unwrap_or(SAND_DEFAULT_MODEL_ID);
    let session = inputs.session_options;
    let subagent_model_id = session.and_then(|options| options.model_id.as_deref());

    if session
        .map(|options| options.is_summarization_session)
        .unwrap_or(false)
    {
        if let Some(model_id) = subagent_model_id {
            return subagent(model_id);
        }
    }

    if session
        .map(|options| options.is_computer_use_subagent)
        .unwrap_or(false)
    {
        return from_selection(
            inputs
                .stored_computer_use_model
                .unwrap_or(&sand_computer_use_model_selection()),
        );
    }

    if session
        .map(|options| options.is_browser_use_subagent)
        .unwrap_or(false)
    {
        if let Some(model) = inputs.stored_browser_use_model {
            return from_selection(model);
        }
    }

    if session
        .map(|options| options.session_model_override)
        .unwrap_or(false)
    {
        if let Some(model_id) = subagent_model_id {
            let parameters = session
                .and_then(|options| options.thinking_level.as_deref())
                .filter(|level| !level.trim().is_empty())
                .map(|level| {
                    vec![SandAgentModelParameter {
                        id: "thinking".into(),
                        value: level.trim().into(),
                    }]
                })
                .unwrap_or_default();
            return RequestedModel {
                model_id: model_id.to_string(),
                max_mode: Some(true),
                parameters,
            };
        }
    }

    if let Some(model_id) = subagent_model_id {
        if model_id != effective_default_id {
            return subagent(model_id);
        }
    }

    if let Some(model_id) = inputs.env_model_override {
        return RequestedModel {
            model_id: model_id.to_string(),
            max_mode: None,
            parameters: Vec::new(),
        };
    }

    let fallback = sand_default_model_selection();
    from_selection(effective_default.unwrap_or(&fallback))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SandMockToolCall {
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub args: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SandMockScript {
    pub tool_calls: Vec<SandMockToolCall>,
}

pub fn parse_sand_mock_script(raw: &str) -> Option<SandMockScript> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;

    if let Some(send_message) = object.get("sendMessage").and_then(Value::as_str) {
        if !send_message.is_empty() {
            let mut args = serde_json::Map::new();
            args.insert("type".into(), Value::String("text".into()));
            args.insert("content".into(), Value::String(send_message.to_string()));
            return Some(SandMockScript {
                tool_calls: vec![SandMockToolCall {
                    tool_call_id: None,
                    tool_name: "SendMessage".into(),
                    args,
                }],
            });
        }
    }

    let calls = object.get("toolCalls")?.as_array()?;
    if calls.is_empty() {
        return None;
    }

    let mut tool_calls = Vec::with_capacity(calls.len());
    for value in calls {
        let call = value.as_object()?;
        let tool_name = call.get("toolName")?.as_str()?;
        if tool_name.is_empty() {
            return None;
        }
        let args = call.get("args")?.as_object()?.clone();
        let tool_call_id = match call.get("toolCallId") {
            None => None,
            Some(value) => {
                let id = value.as_str()?;
                if id.is_empty() {
                    return None;
                }
                Some(id.to_string())
            }
        };
        tool_calls.push(SandMockToolCall {
            tool_call_id,
            tool_name: tool_name.to_string(),
            args,
        });
    }

    Some(SandMockScript { tool_calls })
}
