use crate::acp::routed::{execute_routed_turn, resolve_routed_provider};
use crate::hermes::{HermesEngine, HermesEngineConfig, HermesEvent, HermesTurnRequest};
use crate::mcp::McpRegistry;
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
    RoutedProviderCancellation, RoutedProviderTaskRegistry, RoutedToolBridge,
    RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::sand_agent_runner::SandAgentRunner;
use mahayana_host_runtime::runner::turn_agent_composition::{
    TurnAgentComposition, SAND_AGENT_TOKEN_LIMIT,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
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

pub const DEFAULT_FIRST_TOKEN_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_STREAM_TIMEOUT: Duration = Duration::from_secs(90);

fn non_empty_trimmed(val: Option<&str>) -> Option<String> {
    val.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .and_then(|v| non_empty_trimmed(Some(&v)))
}

fn resolve_gateway_url_from(
    ai_gateway_url: Option<&str>,
    gpui_telegram_gateway_url: Option<&str>,
    fabushi_cloud_gateway_url: Option<&str>,
) -> Option<String> {
    if let Some(url) = non_empty_trimmed(ai_gateway_url) {
        return Some(url);
    }
    if let Some(url) = non_empty_trimmed(gpui_telegram_gateway_url) {
        return Some(url);
    }
    if let Some(base) = non_empty_trimmed(fabushi_cloud_gateway_url) {
        if base.contains("/api/") {
            return Some(base);
        }
        return Some(format!("{}/api/ai/chat/stream", base.trim_end_matches('/')));
    }
    None
}

fn resolve_gateway_url() -> Option<String> {
    resolve_gateway_url_from(
        env_non_empty("AI_GATEWAY_URL").as_deref(),
        env_non_empty("GPUI_TELEGRAM_GATEWAY_URL").as_deref(),
        env_non_empty("FABUSHI_CLOUD_GATEWAY_URL").as_deref(),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Meta {
        conversation_id: String,
        provider: String,
        model: String,
    },
    Delta {
        text: String,
    },
    Step {
        step_info: Value,
    },
    Done {
        message: String,
        conversation_id: String,
    },
    Error {
        message: String,
    },
    Unknown {
        event: String,
        data: String,
    },
}

impl SseEvent {
    pub fn parse(event_type: &str, data: &str) -> Option<Self> {
        let trimmed_data = data.trim();
        match event_type {
            "meta" => {
                let v: Value = serde_json::from_str(trimmed_data).ok()?;
                Some(SseEvent::Meta {
                    conversation_id: v
                        .get("conversationId")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .into(),
                    provider: v
                        .get("provider")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .into(),
                    model: v
                        .get("model")
                        .and_then(|s| s.as_str())
                        .unwrap_or_default()
                        .into(),
                })
            }
            "delta" => {
                let v: Value = serde_json::from_str(trimmed_data).ok()?;
                let text = v
                    .get("text")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some(SseEvent::Delta { text })
            }
            "step" => {
                let v: Value = serde_json::from_str(trimmed_data).ok()?;
                Some(SseEvent::Step { step_info: v })
            }
            "done" => {
                let v: Value = serde_json::from_str(trimmed_data).ok()?;
                let message = v
                    .get("message")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let conversation_id = v
                    .get("conversationId")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some(SseEvent::Done {
                    message,
                    conversation_id,
                })
            }
            "error" => {
                let v: Value = serde_json::from_str(trimmed_data).ok()?;
                let message = v
                    .get("message")
                    .and_then(|s| s.as_str())
                    .unwrap_or("Unknown gateway error")
                    .to_string();
                Some(SseEvent::Error { message })
            }
            other => Some(SseEvent::Unknown {
                event: other.to_string(),
                data: trimmed_data.to_string(),
            }),
        }
    }
}

pub struct StreamError {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = if let Some(stripped) = url.strip_prefix("http://") {
        stripped
    } else {
        url
    };
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match host_port.find(':') {
        Some(idx) => {
            let h = &host_port[..idx];
            let p = host_port[idx + 1..].parse::<u16>().ok()?;
            (h.to_string(), p)
        }
        None => (host_port.to_string(), 80),
    };
    if host.is_empty() {
        return None;
    }
    Some((host, port, path))
}

struct SseStreamState {
    accumulated_text: String,
    current_event: String,
    emitted_thinking: bool,
    pending_meta_thinking: Option<AgentTurnOutput>,
}

async fn flush_pending_meta_thinking(
    state: &mut SseStreamState,
    tx: Option<&mpsc::Sender<AgentTurnOutput>>,
) {
    if !state.emitted_thinking {
        if let Some(meta_thinking) = state.pending_meta_thinking.take() {
            state.emitted_thinking = true;
            if let Some(sender) = tx {
                let _ = sender.send(meta_thinking).await;
            }
        }
    }
}

async fn process_sse_line(
    line: &str,
    state: &mut SseStreamState,
    thinking_budget_tokens: Option<usize>,
    start: Instant,
    delta_cb: Option<&TextDeltaCallback>,
    tx: Option<&mpsc::Sender<AgentTurnOutput>>,
) -> std::result::Result<(), StreamError> {
    if line.is_empty() {
        state.current_event = "message".to_string();
        return Ok(());
    }

    if let Some(event_name) = line.strip_prefix("event:") {
        state.current_event = event_name.trim().to_string();
        return Ok(());
    }

    if let Some(data_str) = line.strip_prefix("data:") {
        if let Some(event) = SseEvent::parse(&state.current_event, data_str) {
            match event {
                SseEvent::Meta {
                    conversation_id,
                    provider,
                    model,
                } => {
                    if thinking_budget_tokens.unwrap_or(0) > 0 && !state.emitted_thinking {
                        state.pending_meta_thinking = Some(AgentTurnOutput::Thinking {
                            token: format!("[{}:{}] session={}", provider, model, conversation_id),
                            elapsed_ms: start.elapsed().as_millis() as u64,
                        });
                    }
                }
                SseEvent::Delta { text } => {
                    flush_pending_meta_thinking(state, tx).await;
                    state.accumulated_text.push_str(&text);
                    if let Some(cb) = delta_cb {
                        cb(&text, &state.accumulated_text);
                    }
                }
                SseEvent::Step { step_info } => {
                    if let Some(thought) = step_info
                        .get("thinking")
                        .or_else(|| step_info.get("thought"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        state.pending_meta_thinking = None;
                        state.emitted_thinking = true;
                        if let Some(sender) = tx {
                            let _ = sender
                                .send(AgentTurnOutput::Thinking {
                                    token: thought.to_string(),
                                    elapsed_ms: start.elapsed().as_millis() as u64,
                                })
                                .await;
                        }
                    }
                }
                SseEvent::Done { message, .. } => {
                    flush_pending_meta_thinking(state, tx).await;
                    if !message.is_empty() {
                        state.accumulated_text = message;
                    }
                }
                SseEvent::Error { message } => {
                    return Err(StreamError {
                        code: "GATEWAY_STREAM_ERROR".into(),
                        message,
                    });
                }
                SseEvent::Unknown { .. } => {}
            }
        }
    }

    Ok(())
}

fn normalize_gateway_model(model: &str) -> &str {
    match model.trim() {
        "grok-3" | "deepseek-chat" | "" => "deepseek-flash",
        "deepseek-reasoner" => "deepseek-v4-pro",
        _ => model,
    }
}

async fn stream_turn_from_gateway(
    gateway_url: &str,
    input: &AgentTurnInput,
    model: &str,
    first_token_timeout: Duration,
    stream_timeout: Duration,
    delta_cb: Option<&TextDeltaCallback>,
    tx: Option<&mpsc::Sender<AgentTurnOutput>>,
) -> std::result::Result<String, StreamError> {
    let start = Instant::now();
    let client = reqwest::Client::builder()
        .connect_timeout(first_token_timeout)
        .user_agent("gpui_telegram/1.0.0")
        .build()
        .map_err(|e| StreamError {
            code: "GATEWAY_CLIENT_ERROR".into(),
            message: format!("Failed to build HTTP client for AI Gateway: {e}"),
        })?;

    let bearer_token = env_non_empty("AI_GATEWAY_BEARER_TOKEN")
        .unwrap_or_else(|| format!("gpui-turn-{}", uuid::Uuid::new_v4().simple()));

    let payload = serde_json::json!({
        "message": input.prompt,
        "model": normalize_gateway_model(model),
        "conversationId": input.session_id,
        "session_id": input.session_id,
        "client": {
            "app": "gpui_telegram",
            "version": "1.0.0",
            "project": {
                "path": "/gpui-telegram",
                "name": "gpui-telegram"
            }
        }
    });

    let send_future = client
        .post(gateway_url)
        .bearer_auth(&bearer_token)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&payload)
        .send();

    let mut resp = match tokio::time::timeout(first_token_timeout, send_future).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) if e.is_timeout() => {
            return Err(StreamError {
                code: "FIRST_TOKEN_TIMEOUT".into(),
                message: format!(
                    "Connection to AI Gateway timed out after {:?}: {}",
                    first_token_timeout, e
                ),
            });
        }
        Ok(Err(e)) if e.is_builder() => {
            return Err(StreamError {
                code: "INVALID_GATEWAY_URL".into(),
                message: format!("Failed to parse gateway URL {}: {}", gateway_url, e),
            });
        }
        Ok(Err(e)) => {
            return Err(StreamError {
                code: "GATEWAY_CONNECT_ERROR".into(),
                message: format!("Failed to connect to AI Gateway {}: {}", gateway_url, e),
            });
        }
        Err(_) => {
            return Err(StreamError {
                code: "FIRST_TOKEN_TIMEOUT".into(),
                message: format!(
                    "Connection to AI Gateway timed out after {:?}",
                    first_token_timeout
                ),
            });
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let body_preview = tokio::time::timeout(Duration::from_secs(2), resp.text())
            .await
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or_default();
        let trimmed = body_preview.trim();
        let detail = if trimmed.is_empty() {
            format!("AI Gateway returned error status: {}", status)
        } else {
            format!("AI Gateway returned error status {}: {}", status, trimmed)
        };
        return Err(StreamError {
            code: "GATEWAY_HTTP_ERROR".into(),
            message: detail,
        });
    }

    let stream_future = async {
        let mut raw_bytes = Vec::<u8>::new();
        let mut state = SseStreamState {
            accumulated_text: String::new(),
            current_event: "message".to_string(),
            emitted_thinking: false,
            pending_meta_thinking: None,
        };

        while let Some(chunk) = resp.chunk().await.map_err(|e| StreamError {
            code: "STREAM_READ_ERROR".into(),
            message: format!("Error reading from AI Gateway: {}", e),
        })? {
            raw_bytes.extend_from_slice(&chunk);
            while let Some(line_end) = raw_bytes.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&raw_bytes[..line_end])
                    .trim_end_matches('\r')
                    .to_string();
                raw_bytes.drain(..=line_end);
                process_sse_line(
                    &line,
                    &mut state,
                    input.thinking_budget_tokens,
                    start,
                    delta_cb,
                    tx,
                )
                .await?;
            }
        }

        if !raw_bytes.is_empty() {
            let line = String::from_utf8_lossy(&raw_bytes)
                .trim_end_matches('\r')
                .to_string();
            process_sse_line(
                &line,
                &mut state,
                input.thinking_budget_tokens,
                start,
                delta_cb,
                tx,
            )
            .await?;
        }

        if state.accumulated_text.is_empty() {
            return Err(StreamError {
                code: "EMPTY_RESPONSE".into(),
                message: "AI Gateway stream finished with empty text".into(),
            });
        }

        Ok(state.accumulated_text)
    };

    match tokio::time::timeout(stream_timeout, stream_future).await {
        Ok(res) => res,
        Err(_) => Err(StreamError {
            code: "STREAM_TIMEOUT".into(),
            message: format!("AI Gateway stream exceeded overall timeout {:?}", stream_timeout),
        }),
    }
}

#[derive(Clone)]
pub struct AcpRunner {
    pub default_model: String,
    pub gateway_url: String,
    pub first_token_timeout: Duration,
    pub stream_timeout: Duration,
    pub fallback_to_mock: bool,
    on_text_delta: Option<TextDeltaCallback>,
    composition: TurnAgentComposition,
    data_dir: Option<PathBuf>,
    provider: Option<RoutedProvider>,
    tool_bridge: Option<Arc<dyn RoutedToolBridge>>,
    mcp_registry: Option<Arc<RwLock<McpRegistry>>>,
    hermes_engine: Option<Arc<HermesEngine>>,
    lazy_hermes: Arc<Mutex<Option<Arc<HermesEngine>>>>,
    gateway_overridden: bool,
    routed_registry: Arc<RoutedProviderTaskRegistry>,
}

impl AcpRunner {
    pub fn new(default_model: impl Into<String>) -> Self {
        let gateway_url = resolve_gateway_url().unwrap_or_default();
        Self {
            default_model: default_model.into(),
            gateway_url,
            first_token_timeout: DEFAULT_FIRST_TOKEN_TIMEOUT,
            stream_timeout: DEFAULT_STREAM_TIMEOUT,
            fallback_to_mock: false,
            on_text_delta: None,
            composition: default_turn_composition(),
            data_dir: None,
            provider: None,
            tool_bridge: None,
            mcp_registry: None,
            hermes_engine: None,
            lazy_hermes: Arc::new(Mutex::new(None)),
            gateway_overridden: false,
            routed_registry: Arc::new(RoutedProviderTaskRegistry::default()),
        }
    }

    pub fn with_gateway_url(mut self, url: impl Into<String>) -> Self {
        self.gateway_url = url.into();
        self.gateway_overridden = true;
        self
    }

    pub fn with_hermes_engine(mut self, engine: Arc<HermesEngine>) -> Self {
        self.hermes_engine = Some(engine);
        self
    }

    pub fn hermes_engine(&self) -> Result<Arc<HermesEngine>> {
        if let Some(eng) = &self.hermes_engine {
            return Ok(Arc::clone(eng));
        }
        let mut guard = self.lazy_hermes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(eng) = guard.as_ref() {
            return Ok(Arc::clone(eng));
        }
        let cfg = HermesEngineConfig {
            data_dir: self.data_dir.clone(),
            default_model: self.default_model.clone(),
            ..Default::default()
        };
        let eng = Arc::new(HermesEngine::new(cfg)?);
        if let Some(ref mcp) = self.mcp_registry {
            if let (Ok(mut target), Ok(src)) = (eng.mcp_registry().write(), mcp.read()) {
                for tool in src.list_tools() {
                    target.register(tool);
                }
            }
        }
        *guard = Some(Arc::clone(&eng));
        Ok(eng)
    }

    pub fn with_timeouts(mut self, first_token: Duration, stream: Duration) -> Self {
        self.first_token_timeout = first_token;
        self.stream_timeout = stream;
        self
    }

    pub fn with_fallback_to_mock(mut self, _fallback: bool) -> Self {
        // Production paths do not synthesize fake completions on transport/gateway failures.
        self.fallback_to_mock = false;
        self
    }

    pub fn with_text_delta_callback(mut self, callback: TextDeltaCallback) -> Self {
        self.on_text_delta = Some(callback);
        self
    }

    pub fn with_turn_composition(mut self, composition: TurnAgentComposition) -> Self {
        self.composition = composition;
        self
    }

    pub fn with_data_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.data_dir = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn with_provider(mut self, provider: RoutedProvider) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn with_tool_bridge(mut self, bridge: Arc<dyn RoutedToolBridge>) -> Self {
        self.tool_bridge = Some(bridge);
        self
    }

    pub fn with_mcp_registry(mut self, registry: Arc<RwLock<McpRegistry>>) -> Self {
        self.mcp_registry = Some(registry);
        self
    }

    pub fn cancel_session(&self, session_id: &str, reason: impl Into<String>) -> bool {
        let routed = self.routed_registry.cancel_agent(session_id, reason) > 0;
        let hermes = if let Some(eng) = &self.hermes_engine {
            eng.cancel_session(session_id)
        } else {
            self.lazy_hermes
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|e| e.cancel_session(session_id)))
                .unwrap_or(false)
        };
        routed || hermes
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

    /// Execute turn streaming via configured RoutedProvider or AI Gateway SSE.
    pub async fn execute_turn_stream(
        &self,
        input: AgentTurnInput,
    ) -> Result<mpsc::Receiver<AgentTurnOutput>> {
        let resolved_provider = if self.gateway_overridden && self.provider.is_none() {
            None
        } else {
            resolve_routed_provider(self.provider, self.data_dir.as_deref())?
        };

        if let Some(provider) = resolved_provider {
            let data_dir = self
                .data_dir
                .clone()
                .unwrap_or_else(|| std::env::temp_dir().join("gpui_telegram_agent_data"));
            let outputs = execute_routed_turn(
                provider,
                data_dir,
                input,
                self.tool_bridge.clone(),
                self.mcp_registry.clone(),
                self.on_text_delta.clone(),
                Arc::clone(&self.routed_registry),
            )
            .await?;
            let (tx, rx) = mpsc::channel(outputs.len().max(16));
            for item in outputs {
                let _ = tx.send(item).await;
            }
            return Ok(rx);
        }

        // Only stream via external SSE gateway when explicitly configured by caller/env.
        if !self.gateway_overridden && self.gateway_url.trim().is_empty() {
            return self.execute_hermes_turn_stream(input).await;
        }

        let (tx, rx) = mpsc::channel(64);
        let model = input
            .model_id
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        let delta_cb = self.on_text_delta.clone();
        let static_config = self.build_static_config(&input);
        let composition = self.composition.clone();
        let gateway_url = self.gateway_url.clone();
        let first_token_timeout = self.first_token_timeout;
        let stream_timeout = self.stream_timeout;

        tokio::spawn(async move {
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

            let messages = vec![ProviderMessage {
                role: "user".into(),
                content: input.prompt.clone(),
            }];

            let stream_result = stream_turn_from_gateway(
                &gateway_url,
                &input,
                &model,
                first_token_timeout,
                stream_timeout,
                delta_cb.as_ref(),
                Some(&tx),
            )
            .await;

            match stream_result {
                Ok(full_response) => {
                    let _ = sand_runner.run_with(&messages, || Ok(full_response.clone()));
                    let _ = tx
                        .send(AgentTurnOutput::FinalResponse {
                            text: full_response,
                        })
                        .await;
                }
                Err(err) => {
                    let _ = tx
                        .send(AgentTurnOutput::Error {
                            code: err.code,
                            message: err.message,
                        })
                        .await;
                }
            }
        });

        Ok(rx)
    }

    async fn execute_hermes_turn_stream(
        &self,
        input: AgentTurnInput,
    ) -> Result<mpsc::Receiver<AgentTurnOutput>> {
        let engine = self.hermes_engine()?;
        let model = input
            .model_id
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| {
                (!self.default_model.trim().is_empty()).then(|| self.default_model.clone())
            });

        let hermes_req = HermesTurnRequest {
            session_id: (!input.session_id.trim().is_empty()).then(|| input.session_id.clone()),
            chat_id: None,
            bot_id: None,
            bot_name: None,
            user_id: None,
            source: "acp".to_string(),
            prompt: input.prompt.clone(),
            model,
            system_prompt: None,
            thinking_budget_tokens: input.thinking_budget_tokens,
            preset: None,
            max_iterations: None,
            max_tokens: None,
            temperature: None,
            skip_slash_commands: false,
            caller_depth: if input.parent_turn_id.is_some() { 1 } else { 0 },
            ..Default::default()
        };

        let mut hrx = engine.run_turn_stream(hermes_req).await?;
        let (tx, rx) = mpsc::channel(64);
        let delta_cb = self.on_text_delta.clone();
        let started = Instant::now();

        tokio::spawn(async move {
            while let Some(ev) = hrx.recv().await {
                match ev {
                    HermesEvent::Meta { .. } | HermesEvent::InlineButtons { .. } => {}
                    HermesEvent::ThinkingDelta { text } => {
                        let _ = tx
                            .send(AgentTurnOutput::Thinking {
                                token: text,
                                elapsed_ms: started.elapsed().as_millis() as u64,
                            })
                            .await;
                    }
                    HermesEvent::TextDelta { text, accumulated } => {
                        if let Some(cb) = &delta_cb {
                            cb(&text, &accumulated);
                        }
                    }
                    HermesEvent::ToolStart {
                        tool_call_id,
                        tool_name,
                        arguments,
                    } => {
                        let _ = tx
                            .send(AgentTurnOutput::ToolCall {
                                tool_call_id,
                                tool_name,
                                arguments: arguments.to_string(),
                            })
                            .await;
                    }
                    HermesEvent::ToolResult {
                        tool_call_id,
                        success,
                        output,
                        ..
                    } => {
                        let _ = tx
                            .send(AgentTurnOutput::ToolResult {
                                tool_call_id,
                                output,
                                is_error: !success,
                            })
                            .await;
                    }
                    HermesEvent::ApprovalRequired {
                        approval_id,
                        command,
                        reason,
                        ..
                    } => {
                        let _ = tx
                            .send(AgentTurnOutput::Thinking {
                                token: format!("[approval required {approval_id}] {command}: {reason}"),
                                elapsed_ms: started.elapsed().as_millis() as u64,
                            })
                            .await;
                    }
                    HermesEvent::CommandResult { .. } => {}
                    HermesEvent::GoalProgress { note, .. } => {
                        let _ = tx
                            .send(AgentTurnOutput::Thinking {
                                token: note,
                                elapsed_ms: started.elapsed().as_millis() as u64,
                            })
                            .await;
                    }
                    HermesEvent::Done { response_text, .. } => {
                        let _ = tx
                            .send(AgentTurnOutput::FinalResponse {
                                text: response_text,
                            })
                            .await;
                    }
                    HermesEvent::Error { code, message } => {
                        let _ = tx.send(AgentTurnOutput::Error { code, message }).await;
                    }
                }
            }
        });

        Ok(rx)
    }

    pub async fn run_turn(&self, input: AgentTurnInput) -> Result<Vec<AgentTurnOutput>> {
        let mut rx = self.execute_turn_stream(input).await?;
        let mut outputs = Vec::new();
        while let Some(out) = rx.recv().await {
            outputs.push(out);
        }
        Ok(outputs)
    }

    pub async fn execute_turn(input: AgentTurnInput) -> Result<Vec<AgentTurnOutput>> {
        let runner = Self::new("deepseek-flash");
        runner.run_turn(input).await
    }
}

#[cfg(test)]
mod runner_tests {
    use super::*;

    #[test]
    fn test_sse_event_parsing() {
        let meta_event = SseEvent::parse(
            "meta",
            r#"{"conversationId":"conv_123","provider":"deepseek","model":"deepseek-chat"}"#,
        );
        assert_eq!(
            meta_event,
            Some(SseEvent::Meta {
                conversation_id: "conv_123".into(),
                provider: "deepseek".into(),
                model: "deepseek-chat".into(),
            })
        );

        let delta_event = SseEvent::parse("delta", r#"{"text":"Hello world"}"#);
        assert_eq!(
            delta_event,
            Some(SseEvent::Delta {
                text: "Hello world".into(),
            })
        );

        let done_event = SseEvent::parse(
            "done",
            r#"{"message":"Finished response","conversationId":"conv_123"}"#,
        );
        assert_eq!(
            done_event,
            Some(SseEvent::Done {
                message: "Finished response".into(),
                conversation_id: "conv_123".into(),
            })
        );

        let error_event = SseEvent::parse("error", r#"{"message":"Quota exceeded"}"#);
        assert_eq!(
            error_event,
            Some(SseEvent::Error {
                message: "Quota exceeded".into(),
            })
        );
    }

    #[test]
    fn test_url_parser() {
        let (host, port, path) =
            parse_http_url("http://34.46.77.133:8788/api/ai/chat/stream").unwrap();
        assert_eq!(host, "34.46.77.133");
        assert_eq!(port, 8788);
        assert_eq!(path, "/api/ai/chat/stream");

        let (host2, port2, path2) = parse_http_url("http://localhost/stream").unwrap();
        assert_eq!(host2, "localhost");
        assert_eq!(port2, 80);
        assert_eq!(path2, "/stream");
    }

    #[test]
    fn test_runner_builder_methods() {
        let runner = AcpRunner::new("custom-model")
            .with_gateway_url("http://127.0.0.1:9999/chat")
            .with_timeouts(Duration::from_secs(5), Duration::from_secs(30))
            .with_fallback_to_mock(false);

        assert_eq!(runner.default_model, "custom-model");
        assert_eq!(runner.gateway_url, "http://127.0.0.1:9999/chat");
        assert_eq!(runner.first_token_timeout, Duration::from_secs(5));
        assert_eq!(runner.stream_timeout, Duration::from_secs(30));
        assert!(!runner.fallback_to_mock);
    }

    #[test]
    fn test_resolve_gateway_url_priority_and_normalization() {
        assert_eq!(resolve_gateway_url_from(None, None, None), None);
        assert_eq!(
            resolve_gateway_url_from(Some("  "), Some(""), Some("https://gw.example.com")),
            Some("https://gw.example.com/api/ai/chat/stream".into())
        );
        assert_eq!(
            resolve_gateway_url_from(None, None, Some("https://gw.example.com/")),
            Some("https://gw.example.com/api/ai/chat/stream".into())
        );
        assert_eq!(
            resolve_gateway_url_from(
                None,
                None,
                Some("https://gw.example.com/custom/api/ai/chat/stream")
            ),
            Some("https://gw.example.com/custom/api/ai/chat/stream".into())
        );
        assert_eq!(
            resolve_gateway_url_from(
                None,
                Some("https://gpui.example.com/stream"),
                Some("https://fabushi.example.com")
            ),
            Some("https://gpui.example.com/stream".into())
        );
        assert_eq!(
            resolve_gateway_url_from(
                Some("https://primary.example.com/api/ai/chat/stream"),
                Some("https://gpui.example.com/stream"),
                Some("https://fabushi.example.com")
            ),
            Some("https://primary.example.com/api/ai/chat/stream".into())
        );
    }

    #[tokio::test]
    async fn test_reqwest_mock_sse_meta_thinking_delta_done() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local mock SSE server");
        let addr = listener.local_addr().expect("local addr");
        let gateway_url = format!("http://{addr}/api/ai/chat/stream");

        let server_task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept client");
            let mut req_bytes = Vec::new();
            let mut buf = [0u8; 2048];
            let mut header_end = None;
            let mut content_len = 0usize;

            loop {
                let n = sock.read(&mut buf).await.expect("read request");
                if n == 0 {
                    break;
                }
                req_bytes.extend_from_slice(&buf[..n]);
                if header_end.is_none() {
                    if let Some(pos) = req_bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = Some(pos + 4);
                        let headers = String::from_utf8_lossy(&req_bytes[..pos]);
                        for line in headers.lines() {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.eq_ignore_ascii_case("content-length") {
                                    content_len = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                    }
                }
                if let Some(end) = header_end {
                    if req_bytes.len() >= end + content_len {
                        break;
                    }
                }
            }

            let req_text = String::from_utf8_lossy(&req_bytes).to_string();
            let req_lower = req_text.to_ascii_lowercase();
            assert!(req_text.starts_with("POST /api/ai/chat/stream HTTP/1.1"));
            assert!(req_lower.contains("accept: text/event-stream"));
            assert!(req_lower.contains("authorization: bearer "));
            assert!(req_text.contains("\"conversationId\":\"sess_meta\""));
            assert!(req_text.contains("\"model\":\"deepseek-flash\""));
            assert!(req_text.contains("\"/gpui-telegram\""));
            assert!(!req_text.contains("Do NOT overthink"));

            let chunks = [
                "event: meta\ndata: {\"conversationId\":\"conv_mock_99\",\"provider\":\"deepseek\",\"model\":\"deepseek-chat\"}\n\n",
                "event: delta\ndata: {\"text\":\"Reqwest \"}\n\n",
                "event: delta\ndata: {\"text\":\"SSE ok\"}\n\n",
                "event: done\ndata: {\"message\":\"Reqwest SSE ok\",\"conversationId\":\"conv_mock_99\"}",
            ];
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("write headers");
            for chunk in chunks {
                let frame = format!("{:x}\r\n{}\r\n", chunk.len(), chunk);
                sock.write_all(frame.as_bytes())
                    .await
                    .expect("write chunk");
            }
            sock.write_all(b"0\r\n\r\n").await.expect("write terminator");
            let _ = sock.shutdown().await;
        });

        let saw_delta = Arc::new(AtomicBool::new(false));
        let saw_delta_flag = Arc::clone(&saw_delta);
        let runner = AcpRunner::new("deepseek-chat")
            .with_gateway_url(gateway_url)
            .with_text_delta_callback(Arc::new(move |delta, _acc| {
                if !delta.is_empty() {
                    saw_delta_flag.store(true, Ordering::SeqCst);
                }
            }));

        let outputs = runner
            .run_turn(AgentTurnInput {
                session_id: "sess_meta".into(),
                prompt: "Verify meta thinking".into(),
                parent_turn_id: None,
                model_id: Some("deepseek-chat".into()),
                thinking_budget_tokens: Some(512),
            })
            .await
            .expect("turn should stream cleanly");

        server_task.await.expect("join mock SSE server");

        assert!(saw_delta.load(Ordering::SeqCst));
        assert_eq!(outputs.len(), 2, "expected Thinking + FinalResponse, got: {outputs:?}");
        assert!(matches!(
            &outputs[0],
            AgentTurnOutput::Thinking { token, .. }
                if token == "[deepseek:deepseek-chat] session=conv_mock_99"
        ));
        assert_eq!(
            outputs[1],
            AgentTurnOutput::FinalResponse {
                text: "Reqwest SSE ok".into()
            }
        );
    }

    #[test]
    fn test_normalize_gateway_model() {
        assert_eq!(normalize_gateway_model("grok-3"), "deepseek-flash");
        assert_eq!(normalize_gateway_model("deepseek-chat"), "deepseek-flash");
        assert_eq!(normalize_gateway_model(""), "deepseek-flash");
        assert_eq!(normalize_gateway_model("   "), "deepseek-flash");
        assert_eq!(normalize_gateway_model("deepseek-reasoner"), "deepseek-v4-pro");
        assert_eq!(normalize_gateway_model("deepseek-flash"), "deepseek-flash");
        assert_eq!(normalize_gateway_model("deepseek-v4-pro"), "deepseek-v4-pro");
        assert_eq!(normalize_gateway_model("custom-model-v1"), "custom-model-v1");
    }

    #[tokio::test]
    async fn test_default_runner_routes_through_pure_rust_hermes() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let saw_delta = Arc::new(AtomicBool::new(false));
        let saw_flag = Arc::clone(&saw_delta);
        let runner = AcpRunner::new("hermes-local-v1")
            .with_text_delta_callback(Arc::new(move |d, _| {
                if !d.is_empty() {
                    saw_flag.store(true, Ordering::SeqCst);
                }
            }));

        assert!(runner.gateway_url.is_empty() || !runner.gateway_url.contains("ombhrum.com"));
        let eng = runner.hermes_engine().expect("hermes engine initialized");
        let _ = eng
            .providers()
            .set_active("local_hermes", Some("hermes-local-v1"));

        let outputs = runner
            .run_turn(AgentTurnInput {
                session_id: "sess_hermes_default".into(),
                prompt: "Hello Hermes runtime".into(),
                parent_turn_id: None,
                model_id: None,
                thinking_budget_tokens: Some(128),
            })
            .await
            .expect("default turn should succeed via HermesEngine");

        assert!(saw_delta.load(Ordering::SeqCst));
        assert!(outputs
            .iter()
            .any(|o| matches!(o, AgentTurnOutput::FinalResponse { text } if !text.is_empty())));

        // Slash command executes directly without touching any remote gateway
        let cmd_out = runner
            .run_turn(AgentTurnInput {
                session_id: "sess_hermes_default".into(),
                prompt: "/status".into(),
                parent_turn_id: None,
                model_id: None,
                thinking_budget_tokens: None,
            })
            .await
            .expect("/status should succeed");
        assert!(cmd_out
            .iter()
            .any(|o| matches!(o, AgentTurnOutput::FinalResponse { text } if text.contains("sess_hermes_default"))));
    }
}
