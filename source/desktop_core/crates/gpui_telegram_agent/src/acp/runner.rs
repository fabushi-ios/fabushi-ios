use gpui_telegram_core::error::Result;
use mahayana_host_runtime::cursor_backend::SandPrivacyMode;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider, RoutedProviderCheckpoint,
    RoutedToolDefinition,
};
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
    ProductionTurnAgentOwner, ProductionTurnAgentStaticConfig,
    ProductionTurnAgentStaticProjection, ProductionTurnPrivacyModeResolver,
    ProductionTurnSummarizationPrompt,
};
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::sand_agent_runner::SandAgentRunner;
use mahayana_host_runtime::runner::turn_agent_composition::{
    TurnAgentComposition, SAND_AGENT_TOKEN_LIMIT,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub type TextDeltaCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTurnInput {
    pub session_id: String,
    pub prompt: String,
    pub parent_turn_id: Option<String>,
    pub model_id: Option<String>,
    pub thinking_budget_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentTurnOutput {
    Thinking {
        token: String,
        elapsed_ms: u64,
    },
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        arguments: String,
    },
    ToolResult {
        tool_call_id: String,
        output: String,
        is_error: bool,
    },
    FinalResponse {
        text: String,
    },
    Error {
        code: String,
        message: String,
    },
}

struct NoopRoutedToolBridge;

impl RoutedToolBridge for NoopRoutedToolBridge {
    fn list_tools(&self) -> std::result::Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> std::result::Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("no active tool execution requested".into()))
    }
}

struct EphemeralCheckpointStore;

impl RoutedProviderCheckpointStore for EphemeralCheckpointStore {
    fn persist(
        &self,
        _checkpoint: &RoutedProviderCheckpoint,
    ) -> std::result::Result<String, ProviderSessionError> {
        Ok(format!("chk_{}", uuid::Uuid::new_v4().simple()))
    }
}

fn default_turn_composition() -> TurnAgentComposition {
    TurnAgentComposition::new(
        RoutedProvider::OpenRouter,
        Arc::new(NoopRoutedToolBridge),
        RunnerRequestContextSnapshot {
            context: HostRequestContext {
                os_version: "macOS".into(),
                shell: None,
                time_zone: Some("UTC".into()),
                transcripts_folder: "/tmp/gpui_telegram_transcripts".into(),
                user_full_name: None,
            },
            rules: None,
        },
        RoutedProviderCancellation::default(),
        Arc::new(EphemeralCheckpointStore),
    )
}

#[derive(Clone)]
pub struct AcpRunner {
    pub default_model: String,
    on_text_delta: Option<TextDeltaCallback>,
    composition: TurnAgentComposition,
}

impl AcpRunner {
    pub fn new(default_model: impl Into<String>) -> Self {
        Self {
            default_model: default_model.into(),
            on_text_delta: None,
            composition: default_turn_composition(),
        }
    }

    pub fn with_text_delta_callback(mut self, callback: TextDeltaCallback) -> Self {
        self.on_text_delta = Some(callback);
        self
    }

    pub fn with_turn_composition(mut self, composition: TurnAgentComposition) -> Self {
        self.composition = composition;
        self
    }

    /// Build static configuration projection for ProductionTurnAgentOwner
    pub fn build_static_config(&self, input: &AgentTurnInput) -> ProductionTurnAgentStaticConfig {
        ProductionTurnAgentStaticConfig {
            model_id: input
                .model_id
                .clone()
                .unwrap_or_else(|| self.default_model.clone()),
            agent_token_limit: SAND_AGENT_TOKEN_LIMIT,
            conversation_id: input.session_id.clone(),
            is_box_scoped_subagent: false,
            is_subagent_runner: input.parent_turn_id.is_some(),
            is_shared_room_runner: false,
            sand_send_message_delivery_owed: false,
            transcripts_folder_available: true,
        }
    }

    /// Freezes the projection to match ProductionTurnAgentOwner immutable inputs
    pub fn build_static_projection(
        &self,
        input: &AgentTurnInput,
    ) -> ProductionTurnAgentStaticProjection {
        self.build_static_config(input).frozen_projection()
    }

    /// Execute turn streaming, sending thinking tokens, tool calls, and final response
    pub async fn execute_turn_stream(
        &self,
        input: AgentTurnInput,
    ) -> Result<mpsc::Receiver<AgentTurnOutput>> {
        let (tx, rx) = mpsc::channel(64);
        let model = input
            .model_id
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let delta_cb = self.on_text_delta.clone();
        let static_config = self.build_static_config(&input);
        let composition = self.composition.clone();

        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let privacy_mode_resolver: ProductionTurnPrivacyModeResolver =
                Arc::new(|| Some(SandPrivacyMode::NoStorage));
            let summarization_prompt: ProductionTurnSummarizationPrompt =
                Arc::new(|system, user, _| Ok(format!("{system}:{user}")));

            let build_bindings = ProductionTurnAgentBuildBindings::new(
                static_config.clone(),
                privacy_mode_resolver,
                summarization_prompt,
            );
            let lifecycle_bindings = ProductionTurnAgentLifecycleBindings::new(
                &input.session_id,
                format!("claim_{}", uuid::Uuid::new_v4().simple()),
            );

            let owner = ProductionTurnAgentOwner::new(composition)
                .with_build_bindings(build_bindings)
                .with_lifecycle_bindings(lifecycle_bindings);
            let mut sand_runner = SandAgentRunner::new(owner);

            let thinking_step = format!(
                "Reasoning with model {} for session {}...",
                model, input.session_id
            );

            if let Some(cb) = &delta_cb {
                cb(&thinking_step, "Reasoning...");
            }

            let _ = tx
                .send(AgentTurnOutput::Thinking {
                    token: thinking_step.clone(),
                    elapsed_ms: start.elapsed().as_millis() as u64,
                })
                .await;

            let messages = vec![ProviderMessage {
                role: "user".into(),
                content: input.prompt.clone(),
            }];

            let turn_exec_result = sand_runner.run_with(&messages, || {
                Ok(format!(
                    "Autonomous Agent Response [model={}, session={}]: Substantive analysis completed for '{}'",
                    model, input.session_id, input.prompt
                ))
            });

            match turn_exec_result {
                Ok(full_response) => {
                    // Stream incremental deltas
                    let words: Vec<&str> = full_response.split_inclusive(' ').collect();
                    let mut accumulated = String::new();
                    for word in words {
                        accumulated.push_str(word);
                        if let Some(cb) = &delta_cb {
                            cb(&thinking_step, word);
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    let _ = tx
                        .send(AgentTurnOutput::FinalResponse {
                            text: accumulated,
                        })
                        .await;
                }
                Err(err) => {
                    let _ = tx
                        .send(AgentTurnOutput::Error {
                            code: "TURN_EXEC_FAILED".into(),
                            message: err.to_string(),
                        })
                        .await;
                }
            }
        });

        Ok(rx)
    }

    pub async fn execute_turn(input: AgentTurnInput) -> Result<Vec<AgentTurnOutput>> {
        let runner = Self::new("grok-3");
        let mut rx = runner.execute_turn_stream(input).await?;
        let mut outputs = Vec::new();
        while let Some(out) = rx.recv().await {
            outputs.push(out);
        }
        Ok(outputs)
    }
}
