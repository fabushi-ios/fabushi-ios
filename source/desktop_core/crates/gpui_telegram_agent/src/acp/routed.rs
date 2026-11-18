use crate::acp::runner::{AgentTurnInput, AgentTurnOutput, TextDeltaCallback};
use crate::mcp::McpRegistry;
use gpui_telegram_core::error::{Result, TelegramError};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, RoutedProvider, RoutedProviderCheckpoint,
    RoutedToolDefinition,
};
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    ProductionRoutedProviderCheckpointStore, RoutedProviderCancellation, RoutedProviderRun,
    RoutedProviderTaskRegistry, RoutedToolBridge, RunnerRequestContextSnapshot,
    run_routed_provider_in_runner,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

pub(crate) struct SessionCheckpointStore {
    primary: ProductionRoutedProviderCheckpointStore,
    session_dir: PathBuf,
}

impl SessionCheckpointStore {
    pub(crate) fn new(data_dir: &Path, session_id: &str, stream_id: &str) -> Self {
        let safe_session: String = session_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        Self {
            primary: ProductionRoutedProviderCheckpointStore::new(data_dir, session_id, stream_id),
            session_dir: data_dir.join("runner-provider-checkpoints").join(safe_session),
        }
    }
}

impl RoutedProviderCheckpointStore for SessionCheckpointStore {
    fn persist(&self, checkpoint: &RoutedProviderCheckpoint) -> std::result::Result<String, ProviderSessionError> {
        let id = self.primary.persist(checkpoint)?;
        fs::create_dir_all(&self.session_dir).map_err(|e| {
            ProviderSessionError::Transport(format!("checkpoint dir create failed: {e}"))
        })?;
        let payload = serde_json::to_vec_pretty(checkpoint)
            .map_err(|e| ProviderSessionError::Protocol(format!("checkpoint encode failed: {e}")))?;
        let tmp = self.session_dir.join(format!("{id}.tmp"));
        let current = self.session_dir.join("current.json");
        fs::write(&tmp, &payload)
            .and_then(|()| fs::rename(&tmp, &current))
            .map_err(|e| ProviderSessionError::Transport(format!("checkpoint write failed: {e}")))?;
        Ok(id)
    }
}

pub(crate) struct EventCapturingToolBridge {
    inner: Option<Arc<dyn RoutedToolBridge>>,
    mcp_registry: Option<Arc<RwLock<McpRegistry>>>,
    events: Arc<Mutex<Vec<AgentTurnOutput>>>,
}

impl EventCapturingToolBridge {
    pub(crate) fn new(
        inner: Option<Arc<dyn RoutedToolBridge>>,
        mcp_registry: Option<Arc<RwLock<McpRegistry>>>,
        events: Arc<Mutex<Vec<AgentTurnOutput>>>,
    ) -> Self {
        Self { inner, mcp_registry, events }
    }
}

impl RoutedToolBridge for EventCapturingToolBridge {
    fn list_tools(&self) -> std::result::Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = match &self.inner {
            Some(inner) => inner.list_tools()?,
            None => Vec::new(),
        };
        if let Some(reg_lock) = &self.mcp_registry {
            if let Ok(reg) = reg_lock.read() {
                for def in reg.list_tools() {
                    if tools.iter().any(|t| t.name == def.name) { continue; }
                    let schema = serde_json::from_str(&def.input_schema_json)
                        .unwrap_or_else(|_| json!({ "type": "object" }));
                    tools.push(RoutedToolDefinition {
                        name: def.name.clone(),
                        provider_identifier: def.server_id.unwrap_or_else(|| "mcp".into()),
                        tool_name: def.name,
                        description: Some(def.description),
                        input_schema: schema,
                    });
                }
            }
        }
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> std::result::Result<Value, ProviderSessionError> {
        if let Ok(mut ev) = self.events.lock() {
            ev.push(AgentTurnOutput::ToolCall {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool.name.clone(),
                arguments: args.to_string(),
            });
        }
        let outcome = if let Some(inner) = &self.inner {
            inner.call_tool(tool, args, tool_call_id)
        } else if let Some(reg_lock) = &self.mcp_registry {
            let reg = reg_lock.read().map_err(|_| ProviderSessionError::Tool("MCP lock poisoned".into()))?;
            reg.execute_tool(&tool.name, args).map_err(ProviderSessionError::Tool)
        } else {
            Err(ProviderSessionError::Tool("no active tool execution bridge configured".into()))
        };
        if let Ok(mut ev) = self.events.lock() {
            match &outcome {
                Ok(val) => ev.push(AgentTurnOutput::ToolResult {
                    tool_call_id: tool_call_id.to_string(),
                    output: val.to_string(),
                    is_error: false,
                }),
                Err(err) => ev.push(AgentTurnOutput::ToolResult {
                    tool_call_id: tool_call_id.to_string(),
                    output: err.to_string(),
                    is_error: true,
                }),
            }
        }
        outcome
    }
}

pub(crate) fn resolve_routed_provider(
    explicit: Option<RoutedProvider>,
    data_dir: Option<&Path>,
) -> Result<Option<RoutedProvider>> {
    if let Some(provider) = explicit {
        return Ok(Some(provider));
    }
    for env_key in ["FABUSHI_INFERENCE_PROVIDER", "SAND_INFERENCE_PROVIDER"] {
        if let Ok(raw) = std::env::var(env_key) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                let parsed = RoutedProvider::parse(trimmed).ok_or_else(|| {
                    TelegramError::Validation(format!(
                        "CONFIGURATION: Unsupported inference provider '{trimmed}' from {env_key}"
                    ))
                })?;
                return Ok(Some(parsed));
            }
        }
    }
    if let Some(dir) = data_dir {
        let settings_path = dir.join("settings.json");
        if let Ok(contents) = fs::read_to_string(&settings_path) {
            let val: Value = serde_json::from_str(&contents).map_err(|e| {
                TelegramError::Validation(format!("CONFIGURATION: Invalid settings.json: {e}"))
            })?;
            let raw_opt = val
                .get("inferenceProvider")
                .or_else(|| val.get("inference_provider"))
                .or_else(|| val.get("router").and_then(|r| r.get("provider")))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if let Some(raw) = raw_opt {
                let parsed = RoutedProvider::parse(raw).ok_or_else(|| {
                    TelegramError::Validation(format!(
                        "CONFIGURATION: Unsupported inference provider '{raw}'"
                    ))
                })?;
                return Ok(Some(parsed));
            }
        }
    }
    Ok(None)
}

pub(crate) fn map_provider_error(
    err: ProviderSessionError,
    cancellation: &RoutedProviderCancellation,
) -> TelegramError {
    if cancellation.is_cancelled() {
        let reason = cancellation.reason().unwrap_or_else(|| err.to_string());
        return TelegramError::Internal(format!("CANCELLED: {reason}"));
    }
    match err {
        ProviderSessionError::Configuration(m) => TelegramError::Validation(format!("CONFIGURATION: {m}")),
        ProviderSessionError::Authentication(m) => TelegramError::AuthFailed(format!("AUTHENTICATION: {m}")),
        ProviderSessionError::ProviderRefusal(r) => {
            TelegramError::PermissionDenied(r.explanation.unwrap_or_else(|| r.category))
        }
        ProviderSessionError::Protocol(m) => TelegramError::Internal(format!("PROTOCOL: {m}")),
        ProviderSessionError::Cancelled(m) => TelegramError::Internal(format!("CANCELLED: {m}")),
        ProviderSessionError::Tool(m) => TelegramError::Internal(format!("TOOL: {m}")),
        ProviderSessionError::Transport(m) => TelegramError::Network(format!("TRANSPORT: {m}")),
    }
}

pub(crate) async fn execute_routed_turn(
    provider: RoutedProvider,
    data_dir: PathBuf,
    input: AgentTurnInput,
    bridge: Option<Arc<dyn RoutedToolBridge>>,
    mcp_registry: Option<Arc<RwLock<McpRegistry>>>,
    delta_cb: Option<TextDeltaCallback>,
    registry: Arc<RoutedProviderTaskRegistry>,
) -> Result<Vec<AgentTurnOutput>> {
    let stream_id = format!("stream_{}", uuid::Uuid::new_v4().simple());
    let cancellation = registry
        .register_for_agent(&input.session_id, &stream_id)
        .map_err(|e| map_provider_error(e, &RoutedProviderCancellation::default()))?;
    struct FinishGuard { reg: Arc<RoutedProviderTaskRegistry>, id: String }
    impl Drop for FinishGuard {
        fn drop(&mut self) { self.reg.finish(&self.id); }
    }
    let _guard = FinishGuard { reg: Arc::clone(&registry), id: stream_id.clone() };
    let events: Arc<Mutex<Vec<AgentTurnOutput>>> = Arc::new(Mutex::new(Vec::new()));
    let capture_bridge = Arc::new(EventCapturingToolBridge::new(bridge, mcp_registry, Arc::clone(&events)));
    let store = Arc::new(SessionCheckpointStore::new(&data_dir, &input.session_id, &stream_id));
    let cancel_clone = cancellation.clone();
    let messages = vec![ProviderMessage { role: "user".into(), content: input.prompt.clone() }];
    let join_res = tokio::task::spawn_blocking(move || {
        let mut on_delta = |delta: &str, accumulated: &str| {
            if let Some(cb) = &delta_cb { cb(delta, accumulated); }
        };
        let run = RoutedProviderRun {
            provider,
            data_dir: &data_dir,
            messages: &messages,
            typed_replay: None,
            bridge: capture_bridge,
            cursor_auth: None,
            request_context: RunnerRequestContextSnapshot {
                context: HostRequestContext {
                    os_version: std::env::consts::OS.into(),
                    shell: None,
                    time_zone: Some("UTC".into()),
                    transcripts_folder: data_dir.join("transcripts").to_string_lossy().into_owned(),
                    user_full_name: None,
                },
                rules: None,
            },
            cancellation: cancel_clone,
            checkpoint_store: store,
            retry_sink: None,
            retry_report_sink: None,
            stream_attempt_runtime: None,
            usage_sink: None,
            cloud_agents_enabled: false,
            multitask_enabled: false,
            is_computer_use_subagent: false,
            max_steps: 12,
            tool_step_reminder: None,
        };
        run_routed_provider_in_runner(run, &mut on_delta)
    })
    .await
    .map_err(|e| TelegramError::Internal(format!("routed runner join error: {e}")))?;
    let final_text = join_res.map_err(|e| map_provider_error(e, &cancellation))?;
    let mut outputs = events.lock().map(|g| g.clone()).unwrap_or_default();
    outputs.push(AgentTurnOutput::FinalResponse { text: final_text });
    Ok(outputs)
}
