use std::sync::Arc;

use serde_json::Value;

use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedMcpMetaToolDefinition, RoutedToolDefinition,
};
use crate::extensions::session::typed_replay::TypedReplayEntry;

use super::conversation_state::RecentUserMessage;
use super::production_agent_checkpoint::AgentStateCheckpointSink;
use super::prompt_collector_glue::{
    PromptCollectorTurnAction, collect_turn_action_for_projection,
};
use super::routed_provider_runtime::RoutedProviderCancellation;
use super::turn_agent_composition::TurnAgentComposition;
use super::TurnRunOptions;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductionTurnInputProjection {
    pub options: TurnRunOptions,
    pub ack_token: Option<String>,
    pub prompt_action: PromptCollectorTurnAction,
    pub typed_replay: Option<Vec<TypedReplayEntry>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductionTurnActionProjection {
    pub lifecycle_messages: Vec<ProviderMessage>,
    pub provider_messages: Vec<ProviderMessage>,
    pub typed_replay: Option<Vec<TypedReplayEntry>>,
    pub prompt_action: PromptCollectorTurnAction,
}

pub struct ProductionTurnAgentInputProjection<'a> {
    pub action: ProductionTurnActionProjection,
    pub mcp_tools: Vec<RoutedToolDefinition>,
    pub mcp_meta_tools: Vec<RoutedMcpMetaToolDefinition>,
    pub base_state_bytes: Vec<u8>,
    pub ack_token: Option<String>,
    pub cancel_this_run: RoutedProviderCancellation,
    pub emit_update: &'a mut dyn FnMut(&str, &str),
    pub options: TurnRunOptions,
}

pub fn create_production_turn_agent_input_projection<'a>(
    composition: &TurnAgentComposition,
    checkpoint_sink: Option<&Arc<dyn AgentStateCheckpointSink>>,
    lifecycle_messages: &[ProviderMessage],
    provider_messages: &[ProviderMessage],
    turn_input: &ProductionTurnInputProjection,
    emit_update: &'a mut dyn FnMut(&str, &str),
) -> Result<ProductionTurnAgentInputProjection<'a>, ProviderSessionError> {
    let (mcp_tools, mcp_meta_tools) =
        composition.snapshot_mcp_projection().unwrap_or_default();
    let base_state_bytes = match checkpoint_sink {
        Some(sink) => sink.base_state_bytes()?,
        None => Vec::new(),
    };
    Ok(ProductionTurnAgentInputProjection {
        action: ProductionTurnActionProjection {
            lifecycle_messages: lifecycle_messages.to_vec(),
            provider_messages: provider_messages.to_vec(),
            typed_replay: turn_input.typed_replay.clone(),
            prompt_action: turn_input.prompt_action.clone(),
        },
        mcp_tools,
        mcp_meta_tools,
        base_state_bytes,
        ack_token: turn_input.ack_token.clone(),
        cancel_this_run: composition.cancellation(),
        emit_update,
        options: turn_input.options.clone(),
    })
}

pub fn create_production_turn_input_projection(
    args: &Value,
    stream_id: &str,
    messages: &[ProviderMessage],
) -> Result<ProductionTurnInputProjection, String> {
    let request_id = stream_id.trim();
    if request_id.is_empty() {
        return Err("production turn input projection requires streamId".into());
    }

    let latest_user_text = messages
        .iter()
        .rev()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| message.content.clone());
    let recent_user_messages = recent_user_messages(args);
    let message_id = optional_non_empty(args, "messageId").map(ToOwned::to_owned);
    let current_message_text = message_id.as_deref().and_then(|message_id| {
        recent_user_messages
            .iter()
            .find(|message| message.id == message_id)
            .map(|message| message.text.clone())
    });

    let prompt_action = collect_turn_action_for_projection(args, messages)?;

    Ok(ProductionTurnInputProjection {
        options: TurnRunOptions {
            inference_request_id: Some(request_id.to_string()),
            message_id,
            recent_message_text: optional_non_empty(args, "recentMessageText")
                .map(ToOwned::to_owned)
                .or(current_message_text)
                .or(latest_user_text),
            recent_message_rich_text: prompt_action.rich_text.clone(),
            recent_user_messages,
            turn_started_at_ms: None,
            is_fork: args.get("isFork").and_then(Value::as_bool).unwrap_or(false),
            attachment_count: array_len(args, "attachmentPaths"),
            image_count: array_len(args, "selectedImages"),
            video_count: array_len(args, "selectedVideos"),
            has_reply_context: args
                .get("replyContext")
                .is_some_and(|value| !value.is_null()),
        },
        ack_token: None,
        prompt_action,
        typed_replay: None,
    })
}

fn recent_user_messages(value: &Value) -> Vec<RecentUserMessage> {
    value
        .get("recentUserMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| {
            let id = message.get("id")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let text = message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Some(RecentUserMessage {
                id: id.to_string(),
                text,
            })
        })
        .collect()
}

fn optional_non_empty<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn array_len(value: &Value, field: &str) -> usize {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}
