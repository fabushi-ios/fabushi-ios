//! Product-owned `AgentBackend` implementation for Mahayana.
//!
//! This adapter preserves the existing MiniApp/host contract while replacing
//! the default Codex implementation with the sovereign native engine and MCP
//! transport. Codex can remain behind an explicit compatibility feature.

use async_trait::async_trait;
use mahayana_agent::{
    AgentActivity, AgentActivityStatus, AgentBackend, AgentError, AgentEvent, AgentMessageRequest,
    ApprovalResolution, McpAppSession, OpenMcpAppRequest, SharedAgentEventSink, StartThreadRequest,
};
use mahayana_core::{
    AgentThreadId, ApprovalDecision, ConversationId, Message, MessageId, MessageRole,
    ModelTokenUsage, ModelTokenUsageSnapshot, OperationId,
};
use mahayana_kernel::{
    ApprovalResolution as KernelApprovalResolution, Capability, CapabilitySet, EngineBackend,
    ExecutionPolicy, KernelError, KernelEvent, KernelEventSink, OpenSessionRequest,
    OperationId as KernelOperationId, RunRequest, RuntimeProfile, SessionId, SharedKernelEventSink,
};
use mahayana_mcp_runtime::{NativeMcpRegistry, ResolvedMcpPlugin};
use mahayana_native_engine::NativeEngine;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct NativeAgentConfig {
    pub profile: RuntimeProfile,
    pub workspace_root: Option<PathBuf>,
    pub mcp_registry: NativeMcpRegistry,
}

#[derive(Clone)]
struct NativeThread {
    session_id: SessionId,
    conversation_id: ConversationId,
}

#[derive(Clone)]
struct NativeMcpSession {
    plugin: ResolvedMcpPlugin,
    platform: mahayana_platform_core::HostPlatform,
    tools: Vec<Value>,
}

#[derive(Clone)]
struct NativeMcpServerState {
    plugin_id: String,
    status: String,
    status_detail: Option<String>,
    tools: Vec<Value>,
    generation: u64,
}

#[derive(Default)]
struct NativeMcpServerStateStore {
    by_server: BTreeMap<String, NativeMcpServerState>,
    next_generation: u64,
}

impl NativeMcpServerStateStore {
    fn begin(&mut self, server_identifier: &str, plugin_id: &str) -> Option<u64> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        self.by_server.insert(
            server_identifier.to_string(),
            NativeMcpServerState {
                plugin_id: plugin_id.to_string(),
                status: "loading".into(),
                status_detail: None,
                tools: Vec::new(),
                generation,
            },
        );
        Some(generation)
    }

    fn is_current(&self, server_identifier: &str, generation: u64) -> bool {
        self.by_server
            .get(server_identifier)
            .is_some_and(|state| state.generation == generation)
    }

    fn settle(
        &mut self,
        server_identifier: &str,
        generation: u64,
        status: &str,
        status_detail: Option<String>,
        tools: Vec<Value>,
    ) -> bool {
        let Some(state) = self.by_server.get_mut(server_identifier) else {
            return false;
        };
        if state.generation != generation {
            return false;
        }
        state.status = status.to_string();
        state.status_detail = status_detail.filter(|detail| !detail.trim().is_empty());
        state.tools = tools;
        true
    }

    fn remove(&mut self, server_identifier: &str) -> bool {
        self.by_server.remove(server_identifier).is_some()
    }

    fn projected(&self) -> Vec<Value> {
        self.by_server
            .iter()
            .map(|(server_identifier, state)| {
                project_mcp_server_state(
                    server_identifier,
                    &state.plugin_id,
                    &state.status,
                    state.status_detail.as_deref(),
                    &state.tools,
                )
            })
            .collect()
    }

    fn clear(&mut self) {
        self.by_server.clear();
    }
}

pub struct NativeAgentBackend {
    engine: Arc<NativeEngine>,
    config: NativeAgentConfig,
    threads: Mutex<HashMap<String, NativeThread>>,
    mcp_sessions: Mutex<HashMap<String, NativeMcpSession>>,
    mcp_server_state: Mutex<NativeMcpServerStateStore>,
    disabled_tools: Mutex<HashMap<String, HashSet<String>>>,
}

impl NativeAgentBackend {
    pub fn new(engine: Arc<NativeEngine>, config: NativeAgentConfig) -> Self {
        Self {
            engine,
            config,
            threads: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(HashMap::new()),
            mcp_server_state: Mutex::new(NativeMcpServerStateStore::default()),
            disabled_tools: Mutex::new(HashMap::new()),
        }
    }

    async fn create_thread(
        &self,
        conversation_id: ConversationId,
    ) -> Result<(AgentThreadId, NativeThread), AgentError> {
        let session_id = self
            .engine
            .open_session(OpenSessionRequest {
                profile: self.config.profile,
                workspace_root: self
                    .config
                    .workspace_root
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                model: None,
                metadata: json!({"conversationId": conversation_id.as_str()}),
            })
            .await
            .map_err(kernel_error)?;
        let thread_id = AgentThreadId::generated("mahayana-native-thread");
        let thread = NativeThread {
            session_id,
            conversation_id,
        };
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .insert(thread_id.to_string(), thread.clone());
        Ok((thread_id, thread))
    }

    fn thread(&self, id: &AgentThreadId) -> Result<NativeThread, AgentError> {
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .get(&id.to_string())
            .cloned()
            .ok_or_else(|| AgentError::ThreadNotFound(id.clone()))
    }

    fn mcp_session(&self, id: &AgentThreadId) -> Result<NativeMcpSession, AgentError> {
        self.mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .get(&id.to_string())
            .cloned()
            .ok_or_else(|| AgentError::ThreadNotFound(id.clone()))
    }

    fn is_tool_disabled(&self, server: &str, tool: &str) -> Result<bool, AgentError> {
        Ok(self
            .disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?
            .get(server)
            .is_some_and(|tools| tools.contains(tool)))
    }

    fn begin_mcp_server_attempt(&self, plugin: &ResolvedMcpPlugin) -> Result<u64, AgentError> {
        self.mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .begin(&plugin.server_name, &plugin.plugin_id)
            .ok_or_else(|| AgentError::Backend("native MCP server generation exhausted".into()))
    }

    fn settle_mcp_server_attempt(
        &self,
        server_identifier: &str,
        generation: u64,
        status: &str,
        status_detail: Option<String>,
        tools: Vec<Value>,
    ) -> Result<bool, AgentError> {
        Ok(self
            .mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .settle(
                server_identifier,
                generation,
                status,
                status_detail,
                tools,
            ))
    }

    async fn mcp_call(
        &self,
        session: NativeMcpSession,
        tool: String,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        if self.is_tool_disabled(&session.plugin.server_name, &tool)? {
            return Err(AgentError::Unavailable(format!(
                "MCP tool `{tool}` is disabled by Mahayana policy"
            )));
        }
        tokio::task::spawn_blocking(move || session.plugin.client().call_tool(&tool, arguments))
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .map_err(|error| AgentError::Backend(error.to_string()))
    }
}

struct NativeEventBridge {
    operation_id: OperationId,
    conversation_id: ConversationId,
    sink: SharedAgentEventSink,
}

impl KernelEventSink for NativeEventBridge {
    fn emit(&self, event: KernelEvent) -> Result<(), KernelError> {
        let mapped = match event {
            KernelEvent::MessageDelta { delta, .. } => AgentEvent::MessageDelta { delta },
            KernelEvent::MessageCompleted { text, .. } => AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("mahayana-native-message"),
                    conversation_id: self.conversation_id.clone(),
                    role: MessageRole::Assistant,
                    text,
                    created_at_ms: now_ms(),
                    metadata: json!({"engine":"mahayana-native"}),
                },
            },
            KernelEvent::UsageUpdated {
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                reasoning_output_tokens,
                ..
            } => AgentEvent::TokenUsageUpdated {
                usage: ModelTokenUsageSnapshot {
                    total: None,
                    last: ModelTokenUsage {
                        total_tokens: to_i64(total_tokens),
                        input_tokens: to_i64(input_tokens),
                        cached_input_tokens: to_i64(cached_input_tokens),
                        output_tokens: to_i64(output_tokens),
                        reasoning_output_tokens: to_i64(reasoning_output_tokens),
                    },
                    model_context_window: None,
                },
            },
            KernelEvent::ApprovalRequested {
                approval_id,
                title,
                risk,
                details,
                ..
            } => AgentEvent::ApprovalRequested {
                approval_id: mahayana_core::ApprovalId::new(approval_id)
                    .map_err(|error| KernelError::Backend(error.to_string()))?,
                title,
                details: json!({"risk":risk,"details":details}),
            },
            KernelEvent::Activity {
                kind,
                title,
                detail,
                metadata,
                ..
            } => {
                let step_id = metadata
                    .get("stepId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("mahayana-native:{kind}:{title}"));
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id,
                        kind,
                        title,
                        detail,
                        status: activity_status(&metadata),
                        metadata: Some(metadata),
                    },
                }
            }
            KernelEvent::ToolStarted { tool, .. } if tool == "send_message" => return Ok(()),
            KernelEvent::ToolStarted {
                tool, arguments, ..
            } => {
                let (title, detail) = tool_activity_copy(&tool, false, true);
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id: format!("tool:{tool}"),
                        kind: "tool".into(),
                        title,
                        detail,
                        status: AgentActivityStatus::Running,
                        metadata: Some(json!({"tool":tool,"arguments":arguments})),
                    },
                }
            }
            KernelEvent::ToolCompleted {
                tool,
                output,
                success,
                ..
            } if tool == "send_message" => {
                if !success {
                    return Ok(());
                }
                let Some(event) =
                    native_send_message_event(&self.conversation_id, &output)
                else {
                    return Ok(());
                };
                event
            }
            KernelEvent::ToolCompleted {
                tool,
                output,
                success,
                ..
            } => {
                let (title, detail) = tool_activity_copy(&tool, true, success);
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id: format!("tool:{tool}"),
                        kind: "tool".into(),
                        title,
                        detail,
                        status: if success {
                            AgentActivityStatus::Completed
                        } else {
                            AgentActivityStatus::Failed
                        },
                        metadata: Some(json!({"tool":tool,"output":output,"success":success})),
                    },
                }
            }
            KernelEvent::CheckpointCreated {
                checkpoint_id,
                label,
                ..
            } => AgentEvent::Activity {
                activity: AgentActivity {
                    step_id: format!("checkpoint:{checkpoint_id}"),
                    kind: "checkpoint".into(),
                    title: label.unwrap_or_else(|| "Workspace checkpoint".into()),
                    detail: Some(checkpoint_id),
                    status: AgentActivityStatus::Completed,
                    metadata: None,
                },
            },
            KernelEvent::OperationFailed {
                message, retryable, ..
            } => AgentEvent::Activity {
                activity: AgentActivity {
                    step_id: format!("operation:{}", self.operation_id),
                    kind: "operation".into(),
                    title: "Operation failed".into(),
                    detail: Some(message),
                    status: AgentActivityStatus::Failed,
                    metadata: Some(json!({"retryable":retryable})),
                },
            },
            KernelEvent::OperationCompleted { .. } => return Ok(()),
        };
        self.sink
            .emit(mapped)
            .map_err(|error| KernelError::Backend(error.to_string()))
    }
}

#[async_trait]
impl AgentBackend for NativeAgentBackend {
    async fn start_thread(&self, request: StartThreadRequest) -> Result<AgentThreadId, AgentError> {
        self.create_thread(request.conversation_id)
            .await
            .map(|(thread_id, _)| thread_id)
    }

    async fn send_message(
        &self,
        request: AgentMessageRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        let thread = self.thread(&request.thread_id)?;
        if let Ok(mcp) = self.mcp_session(&request.thread_id)
            && mcp
                .tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some("chat"))
        {
            let result = self
                .mcp_call(
                    mcp,
                    "chat".into(),
                    json!({"message":request.text,"surface":"agent"}),
                )
                .await?;
            return events.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("mcp-chat"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: mcp_result_text(&result),
                    created_at_ms: now_ms(),
                    metadata: json!({"mcpResult":result}),
                },
            });
        }

        let sink: SharedKernelEventSink = Arc::new(NativeEventBridge {
            operation_id: request.operation_id.clone(),
            conversation_id: thread.conversation_id,
            sink: events,
        });
        self.engine
            .run(
                RunRequest {
                    session_id: thread.session_id,
                    operation_id: KernelOperationId::from_string(request.operation_id.as_str()),
                    input: request.text,
                    policy: policy_for_profile(self.config.profile),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"clientMessageId":request.client_message_id}),
                },
                sink,
            )
            .await
            .map_err(kernel_error)
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), AgentError> {
        self.engine
            .interrupt(&KernelOperationId::from_string(operation_id.as_str()))
            .await
            .map_err(kernel_error)
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), AgentError> {
        self.engine
            .resolve_approval(KernelApprovalResolution {
                approval_id: resolution.approval_id.to_string(),
                approved: matches!(
                    resolution.decision,
                    ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
                ),
                metadata: resolution.payload,
            })
            .await
            .map_err(kernel_error)
    }

    fn reset_session(&self) -> Result<(), AgentError> {
        NativeEngine::reset_session(&self.engine).map_err(kernel_error)?;
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .clear();
        self.mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .clear();
        self.mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .clear();
        self.disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?
            .clear();
        Ok(())
    }

    async fn list_mcp_servers(&self) -> Result<Vec<Value>, AgentError> {
        Ok(self
            .mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .projected())
    }

    async fn list_connector_apps(&self) -> Result<Vec<Value>, AgentError> {
        Ok(Vec::new())
    }

    async fn remove_mcp_server(&self, server: &str) -> Result<bool, AgentError> {
        let mut removed = false;
        {
            let mut sessions = self
                .mcp_sessions
                .lock()
                .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
            let before = sessions.len();
            sessions.retain(|_, session| session.plugin.server_name != server);
            removed |= sessions.len() != before;
        }
        {
            let mut state = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
            removed |= state.remove(server);
        }
        {
            let mut policies = self
                .disabled_tools
                .lock()
                .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?;
            removed |= policies.remove(server).is_some();
        }
        Ok(removed)
    }

    async fn mcp_custom_instructions(&self) -> Result<HashMap<String, String>, AgentError> {
        Ok(HashMap::new())
    }

    async fn set_mcp_tool_disabled(
        &self,
        server: &str,
        tool: &str,
        disabled: bool,
    ) -> Result<Vec<String>, AgentError> {
        let mut policies = self
            .disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?;
        let tools = policies.entry(server.to_string()).or_default();
        if disabled {
            tools.insert(tool.to_string());
        } else {
            tools.remove(tool);
        }
        let mut disabled = tools.iter().cloned().collect::<Vec<_>>();
        disabled.sort();
        Ok(disabled)
    }

    async fn refresh_mcp_servers(&self) -> Result<(), AgentError> {
        let live = self
            .mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .iter()
            .map(|(thread_id, session)| {
                (
                    thread_id.clone(),
                    session.plugin.plugin_id.clone(),
                    session.plugin.server_name.clone(),
                    session.platform,
                )
            })
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        for (_thread_id, plugin_id, server, platform) in live {
            if !seen.insert(server.clone()) {
                continue;
            }
            let generation = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
                .begin(&server, &plugin_id)
                .ok_or_else(|| AgentError::Backend("native MCP server generation exhausted".into()))?;
            let registry = self.config.mcp_registry.clone();
            let plugin_id_for_resolve = plugin_id.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                registry.resolve_plugin(&plugin_id_for_resolve, platform)
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;
            let resolved = match resolved {
                Ok(resolved) if resolved.server_name == server => resolved,
                Ok(resolved) => {
                    let detail = format!(
                        "MCP refresh resolved {} instead of {}",
                        resolved.server_name,
                        server
                    );
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
                Err(error) => {
                    let detail = error.to_string();
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
            };
            let client = resolved.client();
            let tools = tokio::task::spawn_blocking(move || client.list_tools())
                .await
                .map_err(|error| AgentError::Backend(error.to_string()))?;
            let tools = match tools {
                Ok(tools) => tools,
                Err(error) => {
                    let detail = error.to_string();
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
            };
            {
                let mut sessions = self
                    .mcp_sessions
                    .lock()
                    .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
                let state = self
                    .mcp_server_state
                    .lock()
                    .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
                if !state.is_current(&server, generation) {
                    continue;
                }
                for session in sessions.values_mut() {
                    if session.plugin.server_name == server {
                        session.plugin = resolved.clone();
                        session.platform = platform;
                        session.tools = tools.clone();
                    }
                }
            }
            let _ = self.settle_mcp_server_attempt(
                &server,
                generation,
                "connected",
                None,
                tools,
            )?;
        }
        Ok(())
    }

    async fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        let session = self
            .mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .values()
            .find(|session| session.plugin.server_name == server)
            .cloned()
            .ok_or_else(|| AgentError::Unavailable(format!("MCP server not open: {server}")))?;
        self.mcp_call(session, tool.to_string(), arguments).await
    }

    async fn open_mcp_app(&self, request: OpenMcpAppRequest) -> Result<McpAppSession, AgentError> {
        let platform = request.platform;
        let registry = self.config.mcp_registry.clone();
        let plugin_id = request.plugin_id.clone();
        let resolved =
            tokio::task::spawn_blocking(move || registry.resolve_plugin(&plugin_id, platform))
                .await
                .map_err(|error| AgentError::Backend(error.to_string()))?
                .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        let server_identifier = resolved.server_name.clone();
        let generation = self.begin_mcp_server_attempt(&resolved)?;
        let client = resolved.client();
        let tools = match tokio::task::spawn_blocking({
            let client = client.clone();
            move || client.list_tools()
        })
        .await
        {
            Ok(Ok(tools)) => {
                let settled = self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "connected",
                    None,
                    tools.clone(),
                )?;
                if !settled {
                    return Err(AgentError::Unavailable(format!(
                        "MCP server open superseded by a newer attempt: {server_identifier}"
                    )));
                }
                tools
            }
            Ok(Err(error)) => {
                let detail = error.to_string();
                self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "error",
                    Some(detail.clone()),
                    Vec::new(),
                )?;
                return Err(AgentError::Backend(detail));
            }
            Err(error) => {
                let detail = error.to_string();
                self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "error",
                    Some(detail.clone()),
                    Vec::new(),
                )?;
                return Err(AgentError::Backend(detail));
            }
        };
        let resources = tokio::task::spawn_blocking({
            let client = client.clone();
            move || client.list_resources().unwrap_or_default()
        })
        .await
        .map_err(|error| AgentError::Backend(error.to_string()))?;
        let home_result = if tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("home"))
        {
            tokio::task::spawn_blocking({
                let client = client.clone();
                move || client.call_tool("home", json!({}))
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let (thread_id, _) = self.create_thread(request.conversation_id).await?;
        {
            // Use the same lock order as reset_session (runnable sessions first,
            // canonical server state second) so a reset cannot clear state and
            // then be followed by an older in-flight open resurrecting a
            // runnable session.
            let mut sessions = self
                .mcp_sessions
                .lock()
                .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
            let state = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
            if !state.is_current(&server_identifier, generation) {
                return Err(AgentError::Unavailable(format!(
                    "MCP server open superseded or reset before session commit: {server_identifier}"
                )));
            }
            sessions.insert(
                thread_id.to_string(),
                NativeMcpSession {
                    plugin: resolved.clone(),
                    platform,
                    tools: tools.clone(),
                },
            );
        }
        Ok(McpAppSession {
            thread_id,
            plugin_id: request.plugin_id,
            server: resolved.server_name,
            command_tools: command_tools(&tools),
            tool_gates: tool_gates(&tools),
            tools,
            home_result,
            ui_resources: resources,
        })
    }

    async fn list_mcp_app_tools(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
    ) -> Result<Vec<Value>, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        Ok(session.tools)
    }

    async fn call_mcp_app_tool(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        self.mcp_call(session, tool.to_string(), arguments).await
    }

    async fn read_mcp_app_resource(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
        uri: &str,
    ) -> Result<Vec<Value>, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        let client = session.plugin.client();
        let uri = uri.to_string();
        tokio::task::spawn_blocking(move || client.read_resource(&uri))
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    fn name(&self) -> &'static str {
        "mahayana-native"
    }
}

fn native_send_message_event(
    conversation_id: &ConversationId,
    output: &Value,
) -> Option<AgentEvent> {
    let text = output
        .get("generatedMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let attachment = output
        .get("generatedAttachment")
        .cloned()
        .filter(|value| !value.is_null());
    let transcript_card = output
        .get("generatedTranscriptCard")
        .cloned()
        .filter(|value| !value.is_null());
    if text.is_empty() && attachment.is_none() && transcript_card.is_none() {
        return None;
    }
    let tool_call_id = output
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("send-message");
    let reply_to_message_id = output
        .get("replyToMessageId")
        .cloned()
        .filter(|value| !value.is_null());
    Some(AgentEvent::MessageCompleted {
        message: Message {
            id: MessageId::generated(&format!("mahayana-native-send:{tool_call_id}")),
            conversation_id: conversation_id.clone(),
            role: MessageRole::Assistant,
            text: text.to_string(),
            created_at_ms: now_ms(),
            metadata: json!({
                "engine": "mahayana-native",
                "deliveryTool": "send_message",
                "toolCallId": tool_call_id,
                "generatedAttachment": attachment,
                "transcriptCard": transcript_card,
                "replyToMessageId": reply_to_message_id,
            }),
        },
    })
}

fn project_mcp_server_state(
    server_identifier: &str,
    plugin_id: &str,
    status: &str,
    status_detail: Option<&str>,
    tools: &[Value],
) -> Value {
    let tool_definitions = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let input_schema = tool
                .get("inputSchema")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or(Value::Null);
            Some(json!({
                "name": name,
                "providerIdentifier": server_identifier,
                "toolName": name,
                "description": description,
                "inputSchema": input_schema,
            }))
        })
        .collect::<Vec<_>>();

    json!({
        "name": server_identifier,
        "serverIdentifier": server_identifier,
        "pluginId": plugin_id,
        "status": status,
        "statusDetail": status_detail.filter(|value| !value.trim().is_empty()),
        "runtime": "mahayana-native",
        "tools": tool_definitions,
    })
}

fn command_tools(tools: &[Value]) -> HashMap<String, String> {
    let mut commands = HashMap::new();
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let command = tool
            .pointer("/annotations/command")
            .or_else(|| tool.pointer("/_meta/mahayana/command"))
            .and_then(Value::as_str);
        if let Some(command) = command {
            commands.insert(
                command.trim_start_matches('/').to_string(),
                name.to_string(),
            );
        }
    }
    commands
}

fn tool_gates(tools: &[Value]) -> HashMap<String, String> {
    tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?;
            let capability = tool
                .pointer("/annotations/requiresCapability")
                .or_else(|| tool.pointer("/_meta/mahayana/entitlement"))?
                .as_str()?;
            Some((name.to_string(), capability.to_string()))
        })
        .collect()
}

fn mcp_result_text(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let text = content
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return text;
        }
    }
    result
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            serde_json::to_string_pretty(result).unwrap_or_else(|_| "MCP result".into())
        })
}

fn policy_for_profile(profile: RuntimeProfile) -> ExecutionPolicy {
    match profile {
        RuntimeProfile::DesktopFull | RuntimeProfile::Headless => {
            ExecutionPolicy::interactive_default()
        }
        RuntimeProfile::MobileEmbedded | RuntimeProfile::WebWasm => {
            ExecutionPolicy::mobile_default()
        }
    }
}

fn tool_activity_copy(tool: &str, completed: bool, success: bool) -> (String, Option<String>) {
    let (running, done, detail) = match tool {
        "subagent_run" => (
            "请独立子智能体复核第一版",
            "独立复核已完成",
            "检查完整性、可运行性和明显遗漏",
        ),
        "workspace_read" => ("读取工作区文件", "工作区文件已读取", "基于真实文件继续处理"),
        "workspace_write" => ("写入实现文件", "实现文件已写入", "把变更落到实际工作区"),
        "workspace_search" => ("搜索相关代码", "相关代码搜索完成", "定位需要处理的实现位置"),
        "workspace_checkpoint" => (
            "创建安全检查点",
            "安全检查点已创建",
            "为后续修改保留可恢复状态",
        ),
        "process_exec" => ("运行验证命令", "验证命令已完成", "使用真实执行结果检查实现"),
        "git_status" => ("检查改动状态", "改动状态已检查", "确认当前工作区变更"),
        "git_diff" => ("检查代码差异", "代码差异已检查", "核对实际修改内容"),
        "web_search" => ("搜索外部资料", "外部资料搜索完成", "获取任务需要的最新信息"),
        "web_fetch" => ("读取外部资料", "外部资料读取完成", "核对来源内容"),
        "workflow_create" => ("建立执行步骤", "执行步骤已建立", "按依赖关系组织后续工作"),
        "workflow_status" => ("检查任务进度", "任务进度已更新", "确认各步骤当前状态"),
        _ => ("执行工具步骤", "工具步骤已完成", "继续推进实际任务"),
    };
    if completed {
        if success {
            (done.to_string(), Some(detail.to_string()))
        } else {
            (
                format!("{done}（失败）"),
                Some("这一步没有成功，Agent 会根据错误继续处理".into()),
            )
        }
    } else {
        (running.to_string(), Some(detail.to_string()))
    }
}

fn activity_status(metadata: &Value) -> AgentActivityStatus {
    match metadata.get("status").and_then(Value::as_str) {
        Some("completed") => AgentActivityStatus::Completed,
        Some("failed") => AgentActivityStatus::Failed,
        _ => AgentActivityStatus::Running,
    }
}

fn kernel_error(error: KernelError) -> AgentError {
    match error {
        KernelError::SessionNotFound(id) => AgentError::Backend(format!("session not found: {id}")),
        KernelError::OperationNotFound(id) => AgentError::OperationNotFound(
            OperationId::new(id).unwrap_or_else(|_| OperationId::generated("operation")),
        ),
        KernelError::ApprovalNotFound(id) => AgentError::ApprovalNotFound(
            mahayana_core::ApprovalId::new(id)
                .unwrap_or_else(|_| mahayana_core::ApprovalId::generated("approval")),
        ),
        KernelError::BackendUnavailable(message) | KernelError::CapabilityUnavailable(message) => {
            AgentError::Unavailable(message)
        }
        KernelError::EventConsumerClosed => AgentError::EventConsumerClosed,
        other => AgentError::Backend(other.to_string()),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_send_message_completion_becomes_the_canonical_visible_agent_message() {
        let conversation_id = ConversationId::new("mahayana-ai:agent:test")
            .expect("valid conversation id");
        let event = native_send_message_event(
            &conversation_id,
            &json!({
                "generatedMessage": "Final answer",
                "generatedAttachment": {"name":"report.pdf","path":"files/report.pdf"},
                "replyToMessageId": "user:42",
                "toolCallId": "call-send-42"
            }),
        )
        .expect("visible send_message event");
        let AgentEvent::MessageCompleted { message } = event else {
            panic!("send_message must become MessageCompleted");
        };
        assert_eq!(message.conversation_id, conversation_id);
        assert_eq!(message.text, "Final answer");
        assert_eq!(message.metadata["deliveryTool"], "send_message");
        assert_eq!(message.metadata["toolCallId"], "call-send-42");
        assert_eq!(message.metadata["generatedAttachment"]["name"], "report.pdf");
        assert_eq!(message.metadata["replyToMessageId"], "user:42");
    }

    #[test]
    fn native_send_message_projection_accepts_card_only_awaiting_user_delivery() {
        let event = native_send_message_event(
            &ConversationId::new("mahayana-ai:agent:test").expect("valid conversation id"),
            &json!({
                "generatedTranscriptCard": {
                    "kind": "secretRequest",
                    "requestId": "secret:call-1",
                    "label": "API key",
                    "provided": false
                },
                "toolCallId": "call-1"
            }),
        )
        .expect("card-only delivery");
        let AgentEvent::MessageCompleted { message } = event else {
            panic!("card-only send_message must become MessageCompleted");
        };
        assert_eq!(message.text, "");
        assert_eq!(message.metadata["transcriptCard"]["kind"], "secretRequest");
        assert_eq!(
            message.metadata["transcriptCard"]["requestId"],
            "secret:call-1"
        );
    }

    #[test]
    fn native_send_message_projection_rejects_empty_success_payloads() {
        assert!(
            native_send_message_event(
                &ConversationId::new("mahayana-ai:agent:test").expect("valid conversation id"),
                &json!({"generatedMessage":"   ","generatedAttachment":null})
            )
            .is_none()
        );
    }

    #[test]
    fn command_and_entitlement_metadata_are_product_owned() {
        let tools = vec![json!({
            "name":"publish",
            "annotations":{"command":"/ship","requiresCapability":"publish.pro"}
        })];
        assert_eq!(
            command_tools(&tools).get("ship").map(String::as_str),
            Some("publish")
        );
        assert_eq!(
            tool_gates(&tools).get("publish").map(String::as_str),
            Some("publish.pro")
        );
    }
}

#[cfg(test)]
mod mcp_state_projection_tests {
    use super::*;

    #[test]
    fn production_mcp_state_store_preserves_failure_detail_and_rejects_stale_settlement() {
        let mut store = NativeMcpServerStateStore::default();
        let first = store.begin("calendar", "calendar-plugin").expect("first generation");
        let second = store.begin("calendar", "calendar-plugin").expect("second generation");
        assert!(second > first);

        assert!(!store.settle(
            "calendar",
            first,
            "connected",
            None,
            vec![json!({"name": "stale"})],
        ));
        assert!(store.settle(
            "calendar",
            second,
            "error",
            Some("transport handshake failed".into()),
            Vec::new(),
        ));

        let projected = store.projected();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0]["status"], "error");
        assert_eq!(projected[0]["statusDetail"], "transport handshake failed");
        assert_eq!(projected[0]["tools"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn production_mcp_state_store_remove_invalidates_older_settlement() {
        let mut store = NativeMcpServerStateStore::default();
        let generation = store
            .begin("calendar", "calendar-plugin")
            .expect("generation");
        assert!(store.remove("calendar"));
        assert!(!store.remove("calendar"));
        assert!(!store.settle(
            "calendar",
            generation,
            "connected",
            None,
            vec![json!({"name": "stale-after-remove"})],
        ));
        assert!(store.projected().is_empty());
        let replacement = store
            .begin("calendar", "calendar-plugin")
            .expect("replacement generation");
        assert!(replacement > generation);
    }

    #[test]
    fn production_mcp_state_store_keeps_generation_monotonic_across_reset() {
        let mut store = NativeMcpServerStateStore::default();
        let before_reset = store
            .begin("calendar", "calendar-plugin")
            .expect("pre-reset generation");
        store.clear();
        let after_reset = store
            .begin("calendar", "calendar-plugin")
            .expect("post-reset generation");

        assert!(after_reset > before_reset);
        assert!(!store.settle(
            "calendar",
            before_reset,
            "connected",
            None,
            vec![json!({"name": "stale-after-reset"})],
        ));
        assert_eq!(store.projected()[0]["status"], "loading");
    }

    #[test]
    fn production_mcp_state_store_settles_tools_on_current_generation() {
        let mut store = NativeMcpServerStateStore::default();
        let generation = store.begin("calendar", "calendar-plugin").expect("generation");
        assert!(store.settle(
            "calendar",
            generation,
            "connected",
            None,
            vec![json!({
                "name": "search",
                "description": "Search calendar entries",
                "inputSchema": {"type": "object"}
            })],
        ));

        let projected = store.projected();
        assert_eq!(projected[0]["status"], "connected");
        assert!(projected[0]["statusDetail"].is_null());
        assert_eq!(projected[0]["tools"][0]["toolName"], "search");
        assert_eq!(projected[0]["tools"][0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn canonical_mcp_state_projection_preserves_tool_schema_and_status_detail() {
        let state = project_mcp_server_state(
            "calendar",
            "calendar-plugin",
            "connected",
            Some("healthy detail"),
            &[
                json!({
                    "name": "search",
                    "description": "Search calendar entries",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}}
                    }
                }),
                json!({"name": "create", "inputSchema": {"type": "object"}}),
            ],
        );

        assert_eq!(state["serverIdentifier"], "calendar");
        assert_eq!(state["pluginId"], "calendar-plugin");
        assert_eq!(state["status"], "connected");
        assert_eq!(state["statusDetail"], "healthy detail");
        assert_eq!(state["tools"].as_array().map(Vec::len), Some(2));
        assert_eq!(state["tools"][0]["providerIdentifier"], "calendar");
        assert_eq!(state["tools"][0]["toolName"], "search");
        assert_eq!(state["tools"][0]["description"], "Search calendar entries");
        assert_eq!(
            state["tools"][0]["inputSchema"]["properties"]["query"]["type"],
            "string"
        );
    }

    #[test]
    fn canonical_mcp_state_projection_filters_empty_detail_and_invalid_tools() {
        let state = project_mcp_server_state(
            "files",
            "files-plugin",
            "connected",
            Some("   "),
            &[json!({"description": "missing name"}), json!({"name": "   "})],
        );

        assert!(state["statusDetail"].is_null());
        assert_eq!(state["tools"].as_array().map(Vec::len), Some(0));
    }
}
