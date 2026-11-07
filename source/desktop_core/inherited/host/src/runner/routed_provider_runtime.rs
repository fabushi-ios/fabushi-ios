use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::extensions::inference::cursor_inference_transport::CursorInferenceAuth;
use crate::extensions::session::typed_replay::TypedReplayEntry;
use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderPartialToolCall, ProviderSessionError, ProviderTokenUsage,
    ProviderToolStepReminderHook, RoutedMcpMetaToolDefinition, RoutedProvider,
    RoutedProviderCheckpoint, RoutedProviderOptions, RoutedToolDefinition,
    run_routed_provider_text_with_lifecycle_reporting_usage_with_max_steps,
};
use crate::host_request_context::HostRequestContext;
use crate::runner::production_turn_run_shell_adapter::{
    ProductionTurnRunShellAdapter, ProviderRetryEvent, ProviderRetryReport,
    RoutedProviderAttemptExecutor, RoutedProviderCheckpointStore,
};
use crate::runner::system_prompt_assembly::render_request_context_system_prompt_with_capabilities;
use crate::runner::StreamAttemptRuntime;
use crate::runner::tools::mcp_meta_tools::execute_routed_tool_with_timeout;

pub const ROUTED_MCP_PROTOCOL_VERSION: &str = "2025-03-26";
pub const ROUTED_MCP_MAX_BODY_BYTES: usize = 1_048_576;

#[derive(Clone, Default)]
pub struct RoutedProviderCancellation {
    cancelled: Arc<AtomicBool>,
    reason: Arc<Mutex<Option<String>>>,
}

impl RoutedProviderCancellation {
    pub fn cancel(&self, reason: impl Into<String>) -> bool {
        let first = !self.cancelled.swap(true, Ordering::AcqRel);
        if first {
            if let Ok(mut slot) = self.reason.lock() {
                *slot = Some(reason.into());
            }
        }
        first
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|reason| reason.clone())
    }

    pub fn check(&self) -> Result<(), ProviderSessionError> {
        if self.is_cancelled() {
            Err(ProviderSessionError::Transport(
                self.reason().unwrap_or_else(|| "Runner provider request cancelled".into()),
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone)]
struct ActiveRoutedProviderTask {
    agent_id: Option<String>,
    cancellation: RoutedProviderCancellation,
}

#[derive(Default)]
pub struct RoutedProviderTaskRegistry {
    active: Mutex<HashMap<String, ActiveRoutedProviderTask>>,
}

impl RoutedProviderTaskRegistry {
    pub fn register(
        &self,
        stream_id: &str,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        self.register_internal(None, stream_id)
    }

    pub fn register_for_agent(
        &self,
        agent_id: &str,
        stream_id: &str,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(ProviderSessionError::Protocol(
                "Runner provider agent id is empty".into(),
            ));
        }
        self.register_internal(Some(agent_id.to_string()), stream_id)
    }

    fn register_internal(
        &self,
        agent_id: Option<String>,
        stream_id: &str,
    ) -> Result<RoutedProviderCancellation, ProviderSessionError> {
        let stream_id = stream_id.trim();
        if stream_id.is_empty() {
            return Err(ProviderSessionError::Protocol(
                "Runner provider stream id is empty".into(),
            ));
        }
        let mut active = self
            .active
            .lock()
            .map_err(|_| ProviderSessionError::Protocol(
                "Runner provider registry lock poisoned".into(),
            ))?;
        if active.contains_key(stream_id) {
            return Err(ProviderSessionError::Protocol(format!(
                "Runner provider stream already exists: {stream_id}"
            )));
        }
        let cancellation = RoutedProviderCancellation::default();
        active.insert(
            stream_id.to_string(),
            ActiveRoutedProviderTask {
                agent_id,
                cancellation: cancellation.clone(),
            },
        );
        Ok(cancellation)
    }

    pub fn cancel(&self, stream_id: &str, reason: impl Into<String>) -> bool {
        let reason = reason.into();
        self.active
            .lock()
            .ok()
            .and_then(|active| active.get(stream_id).cloned())
            .is_some_and(|task| task.cancellation.cancel(reason))
    }

    pub fn agent_id_for_stream(&self, stream_id: &str) -> Option<String> {
        self.active
            .lock()
            .ok()
            .and_then(|active| active.get(stream_id).and_then(|task| task.agent_id.clone()))
    }

    pub fn cancel_agent(&self, agent_id: &str, reason: impl Into<String>) -> usize {
        let reason = reason.into();
        let cancellations = self
            .active
            .lock()
            .map(|active| {
                active
                    .values()
                    .filter(|task| task.agent_id.as_deref() == Some(agent_id))
                    .map(|task| task.cancellation.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        cancellations
            .into_iter()
            .filter(|cancellation| cancellation.cancel(reason.clone()))
            .count()
    }

    pub fn active_stream_ids_for_agent(&self, agent_id: &str) -> Vec<String> {
        let mut streams = self
            .active
            .lock()
            .map(|active| {
                active
                    .iter()
                    .filter_map(|(stream_id, task)| {
                        (task.agent_id.as_deref() == Some(agent_id))
                            .then(|| stream_id.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        streams.sort();
        streams
    }

    pub fn finish(&self, stream_id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(stream_id);
        }
    }

    pub fn cancel_all(&self, reason: &str) {
        let cancellations = self
            .active
            .lock()
            .map(|active| {
                active
                    .values()
                    .map(|task| task.cancellation.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for cancellation in cancellations {
            cancellation.cancel(reason.to_string());
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().map(|active| active.len()).unwrap_or_default()
    }
}

pub trait RoutedToolBridge: Send + Sync {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError>;

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        self.list_tools()
            .map(|tools| tools.into_iter().map(RoutedMcpMetaToolDefinition::from).collect())
    }
    fn observe_partial_tool_call(
        &self,
        _partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        Ok(())
    }
    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunnerRequestContextSnapshot {
    pub context: HostRequestContext,
    pub rules: Option<Vec<Value>>,
}

pub trait RunnerRequestContextSource: Send + Sync {
    fn resolve(&self) -> RunnerRequestContextSnapshot;
}

#[derive(Debug, Clone)]
pub struct ProductionRoutedProviderCheckpointStore {
    directory: PathBuf,
}

impl ProductionRoutedProviderCheckpointStore {
    pub fn new(data_dir: &Path, agent_id: &str, stream_id: &str) -> Self {
        let agent_key = format!("{:x}", Sha256::digest(agent_id.as_bytes()));
        let stream_key = format!("{:x}", Sha256::digest(stream_id.as_bytes()));
        Self {
            directory: data_dir
                .join("runner-provider-checkpoints")
                .join(agent_key)
                .join(stream_key),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl RoutedProviderCheckpointStore for ProductionRoutedProviderCheckpointStore {
    fn persist(
        &self,
        checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError> {
        fs::create_dir_all(&self.directory).map_err(|error| {
            ProviderSessionError::Transport(format!(
                "could not create Runner checkpoint directory: {error}"
            ))
        })?;
        let checkpoint_id = Uuid::new_v4().to_string();
        let temporary_path = self.directory.join(format!("{checkpoint_id}.tmp"));
        let final_path = self.directory.join(format!("{checkpoint_id}.json"));
        let payload = serde_json::to_vec(checkpoint).map_err(|error| {
            ProviderSessionError::Protocol(format!(
                "could not encode Runner provider checkpoint: {error}"
            ))
        })?;
        let write_result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)?;
            file.write_all(&payload)?;
            file.sync_all()
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(ProviderSessionError::Transport(format!(
                "could not persist Runner provider checkpoint: {error}"
            )));
        }
        if let Err(error) = fs::rename(&temporary_path, &final_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(ProviderSessionError::Transport(format!(
                "could not commit Runner provider checkpoint: {error}"
            )));
        }
        Ok(final_path.to_string_lossy().into_owned())
    }
}

struct ProductionRoutedProviderAttemptExecutor<'a> {
    provider: RoutedProvider,
    data_dir: &'a Path,
    messages: &'a [ProviderMessage],
    typed_replay: Option<&'a [TypedReplayEntry]>,
    tools: &'a [RoutedToolDefinition],
    mcp_server_url: Option<&'a str>,
    cursor_auth: Option<Arc<dyn CursorInferenceAuth>>,
    execute_tool: &'a mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_partial_tool_call: &'a mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
    usage_sink: Option<Arc<dyn Fn(ProviderTokenUsage) + Send + Sync>>,
    max_steps: usize,
    tool_step_reminder: Option<Arc<ProviderToolStepReminderHook>>,
}

impl RoutedProviderAttemptExecutor for ProductionRoutedProviderAttemptExecutor<'_> {
    fn run_attempt(
        &mut self,
        resume_from: Option<&RoutedProviderCheckpoint>,
        on_text_delta: &mut dyn FnMut(&str, &str),
        on_checkpoint: &mut dyn FnMut(
            &RoutedProviderCheckpoint,
        ) -> Result<(), ProviderSessionError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<String, ProviderSessionError> {
        let mut options = RoutedProviderOptions {
            data_dir: self.data_dir,
            typed_replay: self.typed_replay,
            cursor_auth: self.cursor_auth.clone(),
            tools: self.tools,
            mcp_server_url: self.mcp_server_url,
            execute_tool: &mut *self.execute_tool,
            on_text_delta,
            on_partial_tool_call: Some(&mut *self.on_partial_tool_call),
            should_cancel,
        };
        let usage_sink = self.usage_sink.clone();
        let mut on_usage = move |usage: ProviderTokenUsage| {
            if let Some(sink) = usage_sink.as_ref() {
                sink(usage);
            }
        };
        run_routed_provider_text_with_lifecycle_reporting_usage_with_max_steps(
            self.provider,
            self.messages,
            &mut options,
            resume_from,
            on_checkpoint,
            &mut on_usage,
            self.max_steps,
            self.tool_step_reminder.as_deref(),
        )
    }
}

pub struct RoutedProviderRun<'a> {
    pub provider: RoutedProvider,
    pub data_dir: &'a Path,
    pub messages: &'a [ProviderMessage],
    pub typed_replay: Option<&'a [TypedReplayEntry]>,
    pub bridge: Arc<dyn RoutedToolBridge>,
    pub cursor_auth: Option<Arc<dyn CursorInferenceAuth>>,
    pub request_context: RunnerRequestContextSnapshot,
    pub cancellation: RoutedProviderCancellation,
    pub checkpoint_store: Arc<dyn RoutedProviderCheckpointStore>,
    pub retry_sink: Option<Arc<dyn Fn(&ProviderRetryEvent) + Send + Sync>>,
    pub retry_report_sink: Option<Arc<dyn Fn(&ProviderRetryReport) + Send + Sync>>,
    pub stream_attempt_runtime: Option<Arc<StreamAttemptRuntime>>,
    pub usage_sink: Option<Arc<dyn Fn(ProviderTokenUsage) + Send + Sync>>,
    pub cloud_agents_enabled: bool,
    pub multitask_enabled: bool,
    pub is_computer_use_subagent: bool,
    pub max_steps: usize,
    pub tool_step_reminder: Option<Arc<ProviderToolStepReminderHook>>,
}

pub fn run_routed_provider_in_runner(
    run: RoutedProviderRun<'_>,
    on_text_delta: &mut dyn FnMut(&str, &str),
) -> Result<String, ProviderSessionError> {
    run.cancellation.check()?;

    let system_prompt = render_request_context_system_prompt_with_capabilities(
        &run.request_context.context,
        run.request_context.rules.as_deref(),
        run.cloud_agents_enabled,
        run.multitask_enabled,
    );
    if run.typed_replay.is_some()
        && !matches!(
            run.provider,
            RoutedProvider::Cursor
                | RoutedProvider::Codex
                | RoutedProvider::Fabushi
                | RoutedProvider::OpenRouter
                | RoutedProvider::ClaudeCode
        )
    {
        return Err(ProviderSessionError::Protocol(
            "typed compacted replay reached the provider runtime before provider-specific message merge was enabled".into(),
        ));
    }
    let mut provider_messages = Vec::with_capacity(run.messages.len() + usize::from(!system_prompt.is_empty()));
    if !system_prompt.is_empty() {
        provider_messages.push(ProviderMessage {
            role: "system".into(),
            content: system_prompt,
        });
    }
    provider_messages.extend_from_slice(run.messages);

    let direct_tools = if run.provider == RoutedProvider::ClaudeCode {
        Vec::new()
    } else {
        run.cancellation.check()?;
        run.bridge.list_tools()?
    };
    let mut mcp_server = if run.provider == RoutedProvider::ClaudeCode {
        Some(start_routed_mcp_server_for_run(
            Arc::clone(&run.bridge),
            run.cancellation.clone(),
            run.is_computer_use_subagent,
        )?)
    } else {
        None
    };
    let mcp_url = mcp_server.as_ref().map(|server| server.url().to_string());
    let bridge = Arc::clone(&run.bridge);
    let tool_cancellation = run.cancellation.clone();
    let mut execute_tool = move |
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    | {
        tool_cancellation.check()?;
        let operation_bridge = Arc::clone(&bridge);
        let operation_cancellation = tool_cancellation.clone();
        let owned_tool = tool.clone();
        let guard_tool = owned_tool.clone();
        let guard_args = args.clone();
        let owned_tool_call_id = tool_call_id.to_string();
        execute_routed_tool_with_timeout(
            &guard_tool,
            &guard_args,
            run.is_computer_use_subagent,
            move || {
                operation_cancellation.check()?;
                operation_bridge.call_tool(
                    &owned_tool,
                    args,
                    &owned_tool_call_id,
                )
            },
        )
    };
    let partial_bridge = Arc::clone(&run.bridge);
    let partial_cancellation = run.cancellation.clone();
    let mut on_partial_tool_call = move |partial: ProviderPartialToolCall| {
        partial_cancellation.check()?;
        partial_bridge.observe_partial_tool_call(&partial)
    };
    let delta_cancellation = run.cancellation.clone();
    let mut guarded_delta = |delta: &str, accumulated: &str| {
        if !delta_cancellation.is_cancelled() {
            on_text_delta(delta, accumulated);
        }
    };

    let mut executor = ProductionRoutedProviderAttemptExecutor {
        provider: run.provider,
        data_dir: run.data_dir,
        messages: &provider_messages,
        typed_replay: run.typed_replay,
        tools: &direct_tools,
        mcp_server_url: mcp_url.as_deref(),
        cursor_auth: run.cursor_auth.clone(),
        execute_tool: &mut execute_tool,
        on_partial_tool_call: &mut on_partial_tool_call,
        usage_sink: run.usage_sink.clone(),
        max_steps: run.max_steps.max(1),
        tool_step_reminder: run.tool_step_reminder.clone(),
    };
    let retry_sink = run.retry_sink.clone();
    let mut on_retry = move |event: &ProviderRetryEvent| {
        if let Some(sink) = retry_sink.as_ref() {
            sink(event);
        }
    };
    let retry_report_sink = run.retry_report_sink.clone();
    let mut on_retry_report = move |report: &ProviderRetryReport| {
        if let Some(sink) = retry_report_sink.as_ref() {
            sink(report);
        }
    };
    let adapter = ProductionTurnRunShellAdapter::default();
    let result = match run.stream_attempt_runtime.clone() {
        Some(runtime) => adapter.run_with_attempt_runtime_reporting(
            runtime,
            &run.cancellation,
            run.checkpoint_store.as_ref(),
            &mut executor,
            &mut guarded_delta,
            &mut on_retry,
            &mut on_retry_report,
        ),
        None => adapter.run_with_retry_reporting(
            &run.cancellation,
            run.checkpoint_store.as_ref(),
            &mut executor,
            &mut guarded_delta,
            &mut on_retry,
            &mut on_retry_report,
        ),
    };

    if let Some(server) = mcp_server.as_mut() {
        server.close();
    }
    run.cancellation.check()?;
    result
}

pub struct RoutedMcpServer {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RoutedMcpServer {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = TcpStream::connect(
                self.url
                    .strip_prefix("http://")
                    .and_then(|value| value.split('/').next())
                    .unwrap_or("127.0.0.1:0"),
            );
            let _ = worker.join();
        }
    }
}

impl Drop for RoutedMcpServer {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn start_routed_mcp_server(
    bridge: Arc<dyn RoutedToolBridge>,
) -> Result<RoutedMcpServer, ProviderSessionError> {
    start_routed_mcp_server_with_cancellation(
        bridge,
        RoutedProviderCancellation::default(),
    )
}

pub fn start_routed_mcp_server_with_cancellation(
    bridge: Arc<dyn RoutedToolBridge>,
    cancellation: RoutedProviderCancellation,
) -> Result<RoutedMcpServer, ProviderSessionError> {
    start_routed_mcp_server_for_run(bridge, cancellation, false)
}

fn start_routed_mcp_server_for_run(
    bridge: Arc<dyn RoutedToolBridge>,
    cancellation: RoutedProviderCancellation,
    is_computer_use_subagent: bool,
) -> Result<RoutedMcpServer, ProviderSessionError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        ProviderSessionError::Transport(format!("could not bind routed MCP server: {error}"))
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        ProviderSessionError::Transport(format!("could not configure routed MCP server: {error}"))
    })?;
    let address = listener
        .local_addr()
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
    let secret = Uuid::new_v4().to_string();
    let path = format!("/mcp/{secret}");
    let url = format!("http://{}{}", address, path);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_cancellation = cancellation.clone();
    let worker = thread::Builder::new()
        .name("mahayana-runner-routed-mcp".into())
        .spawn(move || {
            let mut tools = HashMap::<String, RoutedToolDefinition>::new();
            while !worker_stop.load(Ordering::Acquire) && !worker_cancellation.is_cancelled() {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // A nonblocking listener can yield accepted sockets that are
                        // still nonblocking on some platforms. The routed MCP HTTP
                        // parser is deliberately synchronous, so normalize each
                        // accepted stream before the first header read instead of
                        // treating an early WouldBlock as a closed request.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        let _ = serve_request(
                            stream,
                            &path,
                            &mut tools,
                            &bridge,
                            &worker_cancellation,
                            is_computer_use_subagent,
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        })
        .map_err(|error| ProviderSessionError::Transport(format!(
            "could not start routed MCP server: {error}"
        )))?;

    Ok(RoutedMcpServer {
        url,
        stop,
        worker: Some(worker),
    })
}

fn serve_request(
    mut stream: TcpStream,
    expected_path: &str,
    tools: &mut HashMap<String, RoutedToolDefinition>,
    bridge: &Arc<dyn RoutedToolBridge>,
    cancellation: &RoutedProviderCancellation,
    is_computer_use_subagent: bool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if request.len() > 64 * 1024 {
            return write_response(&mut stream, 431, None);
        }
    };
    let headers = std::str::from_utf8(&request[..header_end])
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "headers are not UTF-8"))?;
    let mut lines = headers.lines();
    let mut request_line = lines.next().unwrap_or("").split_whitespace();
    let method = request_line.next().unwrap_or("");
    let path = request_line.next().unwrap_or("");
    if method != "POST" || path != expected_path {
        return write_response(&mut stream, 404, None);
    }
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if content_length > ROUTED_MCP_MAX_BODY_BYTES {
        return write_response(&mut stream, 413, None);
    }
    let expected = header_end.saturating_add(content_length);
    while request.len() < expected {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
    }
    if request.len() < expected {
        return write_response(&mut stream, 400, None);
    }
    let message = match serde_json::from_slice::<Value>(&request[header_end..expected]) {
        Ok(value) if value.is_object() => value,
        _ => return write_response(&mut stream, 400, None),
    };

    let rpc_method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if cancellation.is_cancelled() {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return write_json(
            &mut stream,
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "result":tool_failure(
                    cancellation.reason().unwrap_or_else(|| "Runner request cancelled".into())
                )
            }),
        );
    }
    if rpc_method == "notifications/initialized" {
        return write_response(&mut stream, 202, None);
    }
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let result = match rpc_method {
        "initialize" => json!({
            "protocolVersion": ROUTED_MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "fabushi-runner-plugins", "version": "1" }
        }),
        "tools/list" => match bridge.list_tools() {
            Ok(discovered) => {
                *tools = discovered.into_iter().map(|tool| (tool.name.clone(), tool)).collect();
                json!({
                    "tools": tools.values().map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description.clone().unwrap_or_else(|| format!(
                            "{} via {}", tool.tool_name, tool.provider_identifier
                        )),
                        "inputSchema": tool.input_schema
                    })).collect::<Vec<_>>()
                })
            }
            Err(error) => tool_failure(error.to_string()),
        },
        "tools/call" => {
            let params = message.get("params").and_then(Value::as_object);
            let name = params.and_then(|value| value.get("name")).and_then(Value::as_str).unwrap_or("");
            let args = params.and_then(|value| value.get("arguments")).cloned().unwrap_or_else(|| json!({}));
            match tools.get(name) {
                Some(tool) => {
                    let operation_bridge = Arc::clone(bridge);
                    let owned_tool = tool.clone();
                    let guard_tool = owned_tool.clone();
                    let guard_args = args.clone();
                    let tool_call_id = Uuid::new_v4().to_string();
                    match execute_routed_tool_with_timeout(
                        &guard_tool,
                        &guard_args,
                        is_computer_use_subagent,
                        move || operation_bridge.call_tool(
                            &owned_tool,
                            args,
                            &tool_call_id,
                        ),
                    ) {
                        Ok(value) => normalize_tool_result(value),
                        Err(error) => tool_failure(error.to_string()),
                    }
                },
                None => tool_failure(format!("Unknown Fabushi plugin tool: {name}")),
            }
        }
        _ => json!({}),
    };
    write_json(&mut stream, json!({"jsonrpc":"2.0","id":id,"result":result}))
}

fn normalize_tool_result(value: Value) -> Value {
    if let Some(root) = value.as_object() {
        if root.contains_key("content") || root.contains_key("structuredContent") {
            let mut normalized = root.clone();
            normalized.entry("isError").or_insert(Value::Bool(false));
            return Value::Object(normalized);
        }
    }
    json!({
        "isError": false,
        "content": [{"type":"text","text":value.to_string()}]
    })
}

fn tool_failure(message: impl Into<String>) -> Value {
    json!({
        "isError": true,
        "content": [{"type":"text","text":message.into()}]
    })
}

fn write_json(stream: &mut TcpStream, value: Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(&value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    write_response(stream, 200, Some(body))
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: Option<Vec<u8>>,
) -> std::io::Result<()> {
    let body = body.unwrap_or_default();
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    if !body.is_empty() {
        write!(stream, "Content-Type: application/json\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(&body)?;
    stream.flush()
}
