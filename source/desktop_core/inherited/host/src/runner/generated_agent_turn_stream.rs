use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError,
};
use crate::extensions::session::typed_replay::TypedReplayEntry;
use crate::transcript_mirror::conversation_state_binary::{
    ConversationStateRecoveryFields, decode_conversation_state_recovery_fields,
};
use crate::transcript_mirror::generated_occurrence_codec::encode_generated_agent_tool_step;

use super::inactive_turn_agent_stream::{
    InactiveTurnAgentLifecycleHooks, InactiveTurnAgentOutputSink,
    InactiveTurnAgentStreamPath, InactiveTurnAgentStreamSource,
    InactiveTurnCheckpointSink, InactiveTurnStreamFuture,
    NoopInactiveTurnAgentLifecycleHooks,
};
use super::production_agent_checkpoint::{
    AgentStateCheckpointSink, TextTurnCheckpointArtifacts,
};
use super::prompt_collector_glue::PromptCollectorTurnAction;
use super::production_turn_input_projection::ProductionTurnAgentInputProjection;
use super::routed_provider_runtime::RoutedProviderCancellation;
use super::turn_agent_composition::{CompletedRoutedToolCall, TurnAgentComposition};
use super::{
    OuterStreamFuture, OuterStreamPersistence, StreamAttemptPolicy,
    StreamAttemptRuntime, StreamCancelReason, TurnRunOptions,
};

pub struct GeneratedAgentTurnContext<'a> {
    pub data_dir: &'a Path,
    pub lifecycle_messages: &'a [ProviderMessage],
    pub provider_messages: &'a [ProviderMessage],
    pub typed_replay: Option<&'a [TypedReplayEntry]>,
    pub prompt_action: &'a PromptCollectorTurnAction,
    pub options: &'a TurnRunOptions,
    pub base_state_bytes: &'a [u8],
    pub ack_token: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedAgentTurnState {
    pub checkpoint: Option<TextTurnCheckpointArtifacts>,
    pub assistant_content: String,
}

#[derive(Debug)]
pub enum GeneratedAgentTurnStreamError {
    Provider(ProviderSessionError),
    Persistence(String),
}

impl From<String> for GeneratedAgentTurnStreamError {
    fn from(value: String) -> Self {
        Self::Persistence(value)
    }
}

impl GeneratedAgentTurnStreamError {
    pub fn into_provider_error(self) -> ProviderSessionError {
        match self {
            Self::Provider(error) => error,
            Self::Persistence(error) => ProviderSessionError::Protocol(format!(
                "generated Agent stream persistence failed: {error}"
            )),
        }
    }
}

struct ProductionGeneratedAgentStreamSource {
    composition: TurnAgentComposition,
    checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
}

fn validate_canonical_state_blob_references(
    checkpoint_sink: &dyn AgentStateCheckpointSink,
    base_state_bytes: &[u8],
) -> Result<(), GeneratedAgentTurnStreamError> {
    if base_state_bytes.is_empty() {
        return Ok(());
    }
    let fields = decode_conversation_state_recovery_fields(base_state_bytes).map_err(|error| {
        GeneratedAgentTurnStreamError::Provider(ProviderSessionError::Protocol(format!(
            "Runner Agent could not decode canonical ConversationStateStructure: {error}"
        )))
    })?;

    fn validate_group(
        checkpoint_sink: &dyn AgentStateCheckpointSink,
        kind: &str,
        ids: &[Vec<u8>],
    ) -> Result<(), GeneratedAgentTurnStreamError> {
        for id in ids {
            if id.is_empty() {
                return Err(GeneratedAgentTurnStreamError::Provider(
                    ProviderSessionError::Protocol(format!(
                        "Runner Agent canonical state contains an empty {kind} blob reference"
                    )),
                ));
            }
            let present = checkpoint_sink
                .read_state_blob(id)
                .map_err(GeneratedAgentTurnStreamError::Provider)?;
            if present.is_none() {
                return Err(GeneratedAgentTurnStreamError::Provider(
                    ProviderSessionError::Protocol(format!(
                        "Runner Agent canonical state is missing referenced {kind} blob"
                    )),
                ));
            }
        }
        Ok(())
    }

    let ConversationStateRecoveryFields {
        root_prompt_messages_json,
        turns,
        todos,
        summary,
        summary_archives,
        ..
    } = fields;
    validate_group(
        checkpoint_sink,
        "root prompt message",
        &root_prompt_messages_json,
    )?;
    validate_group(checkpoint_sink, "conversation turn", &turns)?;
    validate_group(checkpoint_sink, "todo", &todos)?;
    if let Some(summary) = summary {
        validate_group(checkpoint_sink, "summary", &[summary])?;
    }
    validate_group(checkpoint_sink, "summary archive", &summary_archives)?;
    Ok(())
}


fn is_internal_tool_provider(provider: &str) -> bool {
    provider.trim().is_empty()
        || provider.starts_with("mahayana")
        || provider.starts_with("fabushi")
        || provider.starts_with("sand")
}

fn merge_tool_call_identity(args: &Value, tool_call_id: &str) -> Value {
    let mut object = args.as_object().cloned().unwrap_or_default();
    object
        .entry("toolCallId".to_string())
        .or_insert_with(|| Value::String(tool_call_id.to_string()));
    Value::Object(object)
}

fn terminal_generated_result(value: &Value, args: &Value) -> Value {
    if let Some(result) = value.get("result").and_then(Value::as_object)
        && let Some(case) = result.get("case").and_then(Value::as_str)
        && let Some(payload) = result.get("value")
    {
        return json!({case: payload});
    }
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        let mut payload = args.as_object().cloned().unwrap_or_default();
        if let Some(result) = value.as_object() {
            for (key, value) in result {
                if key != "kind" {
                    payload.insert(key.clone(), value.clone());
                }
            }
        }
        return Value::Object(Map::from_iter([(
            kind.to_string(),
            Value::Object(payload),
        )]));
    }
    value.clone()
}

fn mcp_generated_result(value: &Value) -> Value {
    if value.get("success").is_some()
        || value.get("error").is_some()
        || value.get("rejected").is_some()
        || value.get("permissionDenied").is_some()
    {
        return value.clone();
    }

    let content = value
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let kind = item.get("type").and_then(Value::as_str)?;
                    match kind {
                        "text" => Some(json!({
                            "text": {
                                "text": item.get("text").and_then(Value::as_str).unwrap_or_default()
                            }
                        })),
                        "image" => Some(json!({
                            "image": {
                                "data": item.get("data").and_then(Value::as_str).unwrap_or_default(),
                                "mimeType": item.get("mimeType")
                                    .or_else(|| item.get("mime_type"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                            }
                        })),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let structured_content = value
        .get("structuredContent")
        .or_else(|| value.get("structured_content"))
        .cloned();
    let mut success = Map::new();
    if !content.is_empty() {
        success.insert("content".to_string(), Value::Array(content));
    }
    success.insert(
        "isError".to_string(),
        Value::Bool(value.get("isError").and_then(Value::as_bool).unwrap_or(false)),
    );
    if let Some(structured_content) = structured_content {
        success.insert("structuredContent".to_string(), structured_content);
    } else if success.get("content").is_none() && value.is_object() {
        success.insert("structuredContent".to_string(), value.clone());
    }
    json!({"success": Value::Object(success)})
}

fn checkpoint_tool_step(call: CompletedRoutedToolCall) -> Result<Vec<u8>, ProviderSessionError> {
    let effective = if call.tool.tool_name.trim().is_empty() {
        call.tool.name.as_str()
    } else {
        call.tool.tool_name.as_str()
    };
    let external_mcp = !is_internal_tool_provider(&call.tool.provider_identifier);
    let (tool_name, input, result) = if external_mcp {
        let input = json!({
            "name": if call.tool.name.trim().is_empty() { effective } else { call.tool.name.as_str() },
            "args": call.args,
            "toolCallId": call.tool_call_id,
            "providerIdentifier": call.tool.provider_identifier,
            "toolName": effective,
            "serverIdentifier": call.tool.provider_identifier,
        });
        let result = match call.result {
            Ok(value) => Some(mcp_generated_result(&value)),
            Err(error) => Some(json!({"error":{"error":error}})),
        };
        ("mcp".to_string(), input, result)
    } else {
        let input = merge_tool_call_identity(&call.args, &call.tool_call_id);
        let result = match call.result {
            Ok(value) => Some(terminal_generated_result(&value, &input)),
            Err(_) => None,
        };
        (effective.to_string(), input, result)
    };

    encode_generated_agent_tool_step(
        &tool_name,
        &input,
        result.as_ref(),
        &call.tool_call_id,
        Some(call.started_at_ms),
        Some(call.completed_at_ms),
    )
    .map_err(|error| {
        ProviderSessionError::Protocol(format!(
            "Runner Agent could not encode generated tool checkpoint for {tool_name}: {error}"
        ))
    })
}

impl<'ctx>
    InactiveTurnAgentStreamSource<
        GeneratedAgentTurnContext<'ctx>,
        GeneratedAgentTurnState,
    > for ProductionGeneratedAgentStreamSource
{
    type Error = GeneratedAgentTurnStreamError;

    fn start_stream<'a>(
        &'a self,
        context: &'a GeneratedAgentTurnContext<'ctx>,
        _resume_from: Option<&'a GeneratedAgentTurnState>,
        _persist_checkpoint: &'a mut dyn InactiveTurnCheckpointSink<
            GeneratedAgentTurnContext<'ctx>,
            GeneratedAgentTurnState,
        >,
        output: &'a mut dyn InactiveTurnAgentOutputSink,
    ) -> InactiveTurnStreamFuture<
        'a,
        Result<GeneratedAgentTurnState, Self::Error>,
    > {
        Box::pin(async move {
            if let Some(checkpoint_sink) = self.checkpoint_sink.as_ref() {
                let live_base_state = checkpoint_sink
                    .base_state_bytes()
                    .map_err(GeneratedAgentTurnStreamError::Provider)?;
                if live_base_state != context.base_state_bytes {
                    return Err(GeneratedAgentTurnStreamError::Provider(
                        ProviderSessionError::Protocol(
                            "Runner Agent base state changed after turn input projection".into(),
                        ),
                    ));
                }
                validate_canonical_state_blob_references(
                    checkpoint_sink.as_ref(),
                    &live_base_state,
                )?;
            }
            let mut forward_delta = |delta: &str, accumulated: &str| {
                output.on_text_delta(delta, accumulated);
            };
            let assistant_content = self
                .composition
                .run(
                    context.data_dir,
                    context.provider_messages,
                    context.typed_replay,
                    &mut forward_delta,
                )
                .map_err(GeneratedAgentTurnStreamError::Provider)?;
            let generated_tool_steps = self
                .composition
                .take_completed_tool_calls()
                .into_iter()
                .map(checkpoint_tool_step)
                .collect::<Result<Vec<_>, _>>()
                .map_err(GeneratedAgentTurnStreamError::Provider)?;
            let checkpoint = match self.checkpoint_sink.as_ref() {
                Some(checkpoint_sink) => Some(
                    checkpoint_sink
                        .stage_generated_turn(
                            context.lifecycle_messages,
                            context.options,
                            &generated_tool_steps,
                            &assistant_content,
                        )
                        .map_err(GeneratedAgentTurnStreamError::Provider)?,
                ),
                None => None,
            };
            Ok(GeneratedAgentTurnState {
                checkpoint,
                assistant_content,
            })
        })
    }
}

pub struct ProductionGeneratedAgentPersistence {
    checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
    cancellation: RoutedProviderCancellation,
    generation: u64,
    upgrade_quiescing: Arc<AtomicBool>,
    turn_quiesced: Arc<AtomicBool>,
}

impl ProductionGeneratedAgentPersistence {
    pub fn new(
        checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
        cancellation: RoutedProviderCancellation,
        generation: u64,
        upgrade_quiescing: Arc<AtomicBool>,
        turn_quiesced: Arc<AtomicBool>,
    ) -> Self {
        Self {
            checkpoint_sink,
            cancellation,
            generation,
            upgrade_quiescing,
            turn_quiesced,
        }
    }

    fn persist_generated_checkpoint(
        &self,
        context: &GeneratedAgentTurnContext<'_>,
        checkpoint: &GeneratedAgentTurnState,
    ) -> Result<(), String> {
        match (
            self.checkpoint_sink.as_ref(),
            checkpoint.checkpoint.as_ref(),
        ) {
            (Some(checkpoint_sink), Some(staged)) => checkpoint_sink
                .persist_staged_text_turn(context.options, staged)
                .map_err(|error| error.to_string()),
            (None, None) => Ok(()),
            (Some(_), None) => Err(
                "generated Agent stream lost its required private checkpoint".into(),
            ),
            (None, Some(_)) => Err(
                "generated Agent stream produced a private checkpoint for a transient turn".into(),
            ),
        }
    }
}

impl<'ctx>
    OuterStreamPersistence<
        GeneratedAgentTurnContext<'ctx>,
        GeneratedAgentTurnState,
    > for ProductionGeneratedAgentPersistence
{
    fn generation(&self) -> u64 {
        self.generation
    }

    fn run_generation(&self) -> u64 {
        self.generation
    }

    fn prepare_checkpoint_for_persistence(
        &self,
        _checkpoint: &mut GeneratedAgentTurnState,
    ) {}

    fn persist_step_checkpoint<'a>(
        &'a self,
        context: &'a GeneratedAgentTurnContext<'ctx>,
        checkpoint: &'a GeneratedAgentTurnState,
    ) -> OuterStreamFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.persist_generated_checkpoint(context, checkpoint)
        })
    }

    fn persist_final_state<'a>(
        &'a self,
        context: &'a GeneratedAgentTurnContext<'ctx>,
        checkpoint: &'a GeneratedAgentTurnState,
    ) -> OuterStreamFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.persist_generated_checkpoint(context, checkpoint)
        })
    }

    fn commit_disk_pressure_reminder(&self) {}

    fn release_disk_pressure_reminder(&self) {}

    fn is_awaiting_user_selection(&self) -> bool {
        false
    }

    fn is_quiescing_for_upgrade(&self) -> bool {
        self.upgrade_quiescing.load(Ordering::Acquire)
    }

    fn mark_quiesced_for_upgrade(&self) {
        self.turn_quiesced.store(true, Ordering::Release);
    }

    fn cancel_run(&self, cancellation: StreamCancelReason) {
        self.cancellation.cancel(cancellation.reason);
    }
}

struct LiveDeltaSink<'a> {
    callback: &'a mut dyn FnMut(&str, &str),
}

impl InactiveTurnAgentOutputSink for LiveDeltaSink<'_> {
    fn on_text_delta(&mut self, delta: &str, accumulated: &str) {
        (self.callback)(delta, accumulated);
    }
}

pub fn run_production_generated_agent_stream(
    composition: TurnAgentComposition,
    checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
    data_dir: &Path,
    projection: ProductionTurnAgentInputProjection<'_>,
    max_steps: usize,
    generation: u64,
    upgrade_quiescing: Arc<AtomicBool>,
    turn_quiesced: Arc<AtomicBool>,
) -> Result<String, ProviderSessionError> {
    let ProductionTurnAgentInputProjection {
        action,
        mcp_tools,
        mcp_meta_tools,
        base_state_bytes,
        ack_token,
        cancel_this_run,
        emit_update,
        options,
    } = projection;
    let cancellation = cancel_this_run;
    let stream_attempt_runtime = Arc::new(StreamAttemptRuntime::new(
        StreamAttemptPolicy::default(),
    ));
    let composition = composition
        .with_max_steps(max_steps)
        .with_projected_mcp_tools(mcp_tools)
        .with_projected_mcp_meta_tools(mcp_meta_tools)
        .with_stream_attempt_runtime(stream_attempt_runtime);
    let source: Arc<
        dyn InactiveTurnAgentStreamSource<
            GeneratedAgentTurnContext<'_>,
            GeneratedAgentTurnState,
            Error = GeneratedAgentTurnStreamError,
        >,
    > = Arc::new(ProductionGeneratedAgentStreamSource {
        composition,
        checkpoint_sink: checkpoint_sink.clone(),
    });
    let path = InactiveTurnAgentStreamPath::new(source);
    let persistence = ProductionGeneratedAgentPersistence::new(
        checkpoint_sink,
        cancellation,
        generation,
        upgrade_quiescing,
        turn_quiesced,
    );
    let hooks = NoopInactiveTurnAgentLifecycleHooks;
    let context = GeneratedAgentTurnContext {
        data_dir,
        lifecycle_messages: &action.lifecycle_messages,
        provider_messages: &action.provider_messages,
        typed_replay: action.typed_replay.as_deref(),
        prompt_action: &action.prompt_action,
        options: &options,
        base_state_bytes: &base_state_bytes,
        ack_token: ack_token.as_deref(),
    };
    let mut output = LiveDeltaSink {
        callback: emit_update,
    };
    futures::executor::block_on(path.run_lifecycle_with_output(
        &context,
        None,
        &persistence,
        &hooks,
        &mut output,
    ))
    .map(|state| state.assistant_content)
    .map_err(GeneratedAgentTurnStreamError::into_provider_error)
}

#[allow(dead_code)]
fn _lifecycle_hook_anchor(
    hooks: &dyn InactiveTurnAgentLifecycleHooks<GeneratedAgentTurnState>,
) {
    let _ = hooks;
}
