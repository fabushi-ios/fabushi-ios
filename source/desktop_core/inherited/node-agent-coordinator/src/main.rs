use chrono::{SecondsFormat, Utc};
use mahayana_node_agent_coordinator::miniapp_management_relay::miniapp_carrier_allowed;
use mahayana_node_agent_coordinator::carrier::{
    parse_bootstrap_argument, CarrierChannel, CarrierEnvelope, CoordinatorBootstrap,
};
use mahayana_node_agent_coordinator::control_port_client::{
    ClientAction, ControlPortClient, ControlPortPhase,
};
use mahayana_node_agent_coordinator::client_side_tool_v2_relay::{
    ClientSideToolV2Relay, CLIENT_SIDE_TOOL_V2_FAMILY,
};
use mahayana_node_agent_coordinator::gateway::gateway_client::{
    CoordinatorGatewayClient, GatewayCommandExecution, GatewayCommandPolicy,
    GatewayCommandSpan, GatewayTransportStage, dispatch_gateway_command, stream_http_events,
};
use mahayana_node_agent_coordinator::gateway::gateway_event_families::event_family_for_channel;
use mahayana_node_agent_coordinator::gateway::gateway_reachability::ReachabilityOutcome;
use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::{
    GatewayDispatchError, dispatch_http_json, failure_for,
};
use mahayana_node_agent_coordinator::gateway::host_supervisor::{
    GatewayConnection, GatewayHostSupervisor, HEALTH_PROBE_TTL_MS, read_gateway_discovery,
};
use mahayana_node_agent_coordinator::oauth::mcp_oauth_callback_listener::{
    McpOAuthCallbackListener, OAuthCallback, start_mcp_oauth_callback_listener,
};
use mahayana_node_agent_coordinator::oauth::mcp_oauth_forwarder::{
    McpOAuthForwarderState, McpOAuthPendingPayload, OAuthForwarderAction,
};
use mahayana_node_agent_coordinator::oauth::mcp_oauth_loopback_registry::McpOAuthLoopbackRegistry;
use mahayana_node_agent_coordinator::inference_router::{
    deleted_agent_ids_for_host_success, host_transcript_method,
    ActiveInferenceStreamRegistry, CoordinatorInferenceRouter, InferenceProvider,
    InferenceStreamSupersede, InferenceTaskQueue, InferenceTranscriptFile,
    RunnerInferenceEvent, StoredEntry, StoredRole, is_direct_user_send,
    parse_host_routed_prompt_acceptance, parse_runner_inference_event,
    agent_inbound_failure_gateway_args, agent_inbound_wake_prefers_urgent_queue,
    prepare_agent_inbound_wake_routes, redrive_agent_inbound_after_priority_preemption,
    should_append_user_message, should_await_turn,
    parse_send_prompt_attachments,
    prepare_workflow_run_now_route, project_runner_turn_context,
    provider_messages_from_host_transcript, CoordinatorWorkflowRunNowRoute,
};
use mahayana_node_agent_coordinator::webauthn::{
    ApprovedWebAuthnConsent, WebAuthnCeremony,
};
use mahayana_node_agent_coordinator::webauthn::provider::{
    ProductionWebAuthnRuntime, ProductionWebAuthnRuntimeOptions,
};
use mahayana_node_agent_coordinator::webauthn::signer::{
    SpawnedWebAuthnSigner, resolve_web_authn_signer_path,
};
use mahayana_node_agent_coordinator::local_exec::supervisor::{
    ExpectedLocalExecProcessIdentity, LOCAL_EXEC_DAEMON_LIVENESS_INTERVAL_MS,
    LOCAL_EXEC_DAEMON_REFRESH_INTERVAL_MS, LocalExecControl, LocalExecDaemonRuntime,
    LocalExecProcessIdentity, LocalExecSupervisorError, SpawnLocalExecDaemonRequest,
};
use mahayana_node_agent_coordinator::protocol::{
    CoordinatorFrame, Failure, LifecyclePhase, ReplyOutcome, COORDINATOR_DISCONNECTED,
};
use mahayana_node_agent_coordinator::renderer_port_server::{
    RendererPortServer, ServerAction,
};
use mahayana_node_agent_coordinator::runner_tool_relay::{
    RUNNER_RESOLVE_ROUTED_TOOL_GATEWAY_METHOD, RUNNER_TOOL_REQUEST_EVENT_CHANNEL,
    execute_runner_tool_request, runner_tool_resolution_failure,
};
use mahayana_node_agent_coordinator::mcp_lifecycle_relay::{
    HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL, MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD,
    execute_host_mcp_lifecycle_request, host_mcp_lifecycle_resolution_failure,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::{env, fs};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, RecvTimeoutError, Sender},
};
use std::thread;
use std::time::{Duration, Instant};

const HOST_CRASH_CIRCUIT_LIMIT: u64 = 6;

#[derive(Debug)]
struct ActiveHostStdin {
    generation: u64,
    stdin: ChildStdin,
}

#[derive(Debug)]
struct PendingHostRequest {
    generation: u64,
    request_id: String,
    channel: CarrierChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalExecRuntimeCommand {
    Refresh,
    SetPaused(bool),
    Dispose,
}

struct CoordinatorState {
    bootstrap: CoordinatorBootstrap,
    host_bin: PathBuf,
    gateway_discovery_path: PathBuf,
    inference_router: CoordinatorInferenceRouter,
    inference_store: InferenceTranscriptFile,
    inference_store_lock: Mutex<()>,
    inference_queue: InferenceTaskQueue,
    active_inference_streams: ActiveInferenceStreamRegistry,
    inference_streams: Mutex<HashMap<String, Sender<Value>>>,
    host_stdin: Mutex<Option<ActiveHostStdin>>,
    gateway_client: Mutex<CoordinatorGatewayClient>,
    gateway_command_policy: GatewayCommandPolicy,
    host_supervisor: Mutex<GatewayHostSupervisor>,
    tool_relay: Mutex<ClientSideToolV2Relay>,
    tool_replay_done: AtomicBool,
    oauth_forwarder: Mutex<McpOAuthForwarderState>,
    oauth_loopback: Mutex<McpOAuthLoopbackRegistry>,
    oauth_listeners: Mutex<HashMap<String, McpOAuthCallbackListener>>,
    pending: Mutex<HashMap<String, PendingHostRequest>>,
    control_port: Mutex<ControlPortClient>,
    local_exec_commands: Mutex<Option<Sender<LocalExecRuntimeCommand>>>,
    webauthn_runtime: Mutex<Option<ProductionWebAuthnRuntime>>,
    renderer_port: Mutex<RendererPortServer>,
    main_data_port: Mutex<RendererPortServer>,
    pending_native_shell_event: Mutex<Option<Value>>,
    stdout_lock: Mutex<()>,
    spawn_lock: Mutex<()>,
    host_generation: AtomicU64,
    lifecycle_sequence: AtomicU64,
    closed: AtomicBool,
    consecutive_crashes: AtomicU64,
    gateway_events_live: AtomicBool,
    trace_window_refresh_inflight: AtomicBool,
    tdrp_state: Mutex<mahayana_node_agent_coordinator::tdrp::TdrpCoordinatorState>,
}

impl CoordinatorState {
    fn write_frame(&self, channel: CarrierChannel, frame: &CoordinatorFrame) -> io::Result<()> {
        let _guard = self
            .stdout_lock
            .lock()
            .map_err(|_| io::Error::other("stdout lock poisoned"))?;
        let envelope = CarrierEnvelope::new(
            channel,
            serde_json::to_value(frame)
                .map_err(|error| io::Error::other(format!("coordinator frame serialization failed: {error}")))?,
        );
        let mut stdout = io::stdout().lock();
        serde_json::to_writer(&mut stdout, &envelope)
            .map_err(|error| io::Error::other(format!("coordinator envelope serialization failed: {error}")))?;
        writeln!(stdout)?;
        stdout.flush()
    }

    fn post_event(&self, family: &str, payload: Value) {
        let action = match self.renderer_port.lock() {
            Ok(server) => server.post_event(family.to_string(), payload),
            Err(_) => {
                eprintln!(
                    "coordinator event delivery failed family={family} error=renderer_port_lock_poisoned"
                );
                return;
            }
        };
        if let Some(ServerAction::Post(frame)) = action {
            if let Err(error) = self.write_frame(CarrierChannel::Data, &frame) {
                eprintln!(
                    "coordinator event delivery failed family={family} error={error}"
                );
            }
        }
    }

    fn post_main_event(&self, family: &str, payload: Value) -> bool {
        let action = self
            .main_data_port
            .lock()
            .ok()
            .and_then(|server| server.post_event(family.to_string(), payload));
        match action {
            Some(ServerAction::Post(frame)) => {
                self.write_frame(CarrierChannel::MainData, &frame).is_ok()
            }
            _ => false,
        }
    }

    fn queue_native_shell_event(&self, payload: Value) {
        if self.post_main_event("native-shell", payload.clone()) {
            return;
        }
        if let Ok(mut pending) = self.pending_native_shell_event.lock() {
            *pending = Some(payload);
        }
    }

    fn flush_native_shell_event(&self) {
        let pending = self
            .pending_native_shell_event
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        let Some(payload) = pending else {
            return;
        };
        if !self.post_main_event("native-shell", payload.clone()) {
            if let Ok(mut pending) = self.pending_native_shell_event.lock() {
                *pending = Some(payload);
            }
        }
    }

    fn lifecycle(&self, lifecycle: &str, generation: u64, recoverable: bool, detail: Option<&str>) {
        let sequence = self.lifecycle_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let mut event = json!({
            "type": "host.lifecycle",
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "lifecycle": lifecycle,
            "state": if lifecycle == "running" { "running" } else { "stopped" },
            "generation": generation,
            "sequence": sequence,
            "recoverable": recoverable,
        });
        if let Some(detail) = detail {
            event["reason"] = Value::String(detail.to_string());
        }
        self.post_event("runtime", event);
    }


    fn gateway_lifecycle(&self, lifecycle: &str, generation: u64, detail: Option<&str>) {
        let sequence = self.lifecycle_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let mut event = json!({
            "type": "gateway.lifecycle",
            "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "lifecycle": lifecycle,
            "generation": generation,
            "sequence": sequence,
        });
        if let Some(detail) = detail {
            event["reason"] = Value::String(detail.to_string());
        }
        self.post_event("runtime", event);
    }

    fn complete_request(
        &self,
        channel: CarrierChannel,
        request_id: &str,
        outcome: ReplyOutcome,
    ) {
        let actions = match channel {
            CarrierChannel::Data => self
                .renderer_port
                .lock()
                .map(|mut server| server.complete_request(request_id, outcome))
                .unwrap_or_default(),
            CarrierChannel::MainData => self
                .main_data_port
                .lock()
                .map(|mut server| server.complete_request(request_id, outcome))
                .unwrap_or_default(),
            CarrierChannel::Control => Vec::new(),
        };
        for action in actions {
            if let ServerAction::Post(frame) = action {
                let _ = self.write_frame(channel, &frame);
            }
        }
    }

    fn reject_generation(&self, generation: u64, message: &str) {
        let rejected = {
            let mut pending = match self.pending.lock() {
                Ok(pending) => pending,
                Err(_) => return,
            };
            let keys = pending
                .iter()
                .filter_map(|(key, request)| (request.generation == generation).then_some(key.clone()))
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| pending.remove(&key))
                .collect::<Vec<_>>()
        };
        for request in rejected {
            self.complete_request(
                request.channel,
                &request.request_id,
                ReplyOutcome::Failed {
                    failure: Failure::new(COORDINATOR_DISCONNECTED, message),
                },
            );
        }
    }

    fn abort_request(&self, channel: CarrierChannel, request_id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            let host_request_id = pending
                .iter()
                .find_map(|(host_request_id, request)| {
                    (request.channel == channel && request.request_id == request_id)
                        .then(|| host_request_id.clone())
                });
            if let Some(host_request_id) = host_request_id {
                pending.remove(&host_request_id);
            }
        }
    }
}


fn control_command(
    state: &Arc<CoordinatorState>,
    method: &str,
    args: Value,
) -> Result<Value, Failure> {
    let waiter = state
        .control_port
        .lock()
        .map_err(|_| Failure::new(COORDINATOR_DISCONNECTED, "control port lock poisoned"))?
        .call_waiting(method.to_string(), args)?;
    let frame = match waiter.action {
        ClientAction::Post(frame) => frame,
        _ => {
            return Err(Failure::new(
                "COORDINATOR_CONTROL_PROTOCOL_ERROR",
                "control command did not produce a request frame",
            ));
        }
    };
    state
        .write_frame(CarrierChannel::Control, &frame)
        .map_err(|error| {
            Failure::new(
                COORDINATOR_DISCONNECTED,
                format!("control request write failed: {error}"),
            )
        })?;
    waiter
        .reply
        .recv_timeout(Duration::from_secs(15))
        .map_err(|error| {
            Failure::new(
                COORDINATOR_DISCONNECTED,
                format!("control request {method} did not settle: {error}"),
            )
        })?
}

fn local_exec_error(error: Failure) -> LocalExecSupervisorError {
    LocalExecSupervisorError::new(format!("{}: {}", error.code, error.message))
}

fn parse_u64_field(value: &Value, field: &str) -> Result<u64, LocalExecSupervisorError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| LocalExecSupervisorError::new(format!("missing {field}")))
}

fn parse_string_field(value: &Value, field: &str) -> Result<String, LocalExecSupervisorError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| LocalExecSupervisorError::new(format!("missing {field}")))
}

fn parse_local_exec_identity(value: Value) -> Result<LocalExecProcessIdentity, LocalExecSupervisorError> {
    let pid = u32::try_from(parse_u64_field(&value, "pid")?)
        .map_err(|_| LocalExecSupervisorError::new("local-exec pid is out of range"))?;
    Ok(LocalExecProcessIdentity {
        pid,
        start_epoch_ms: parse_u64_field(&value, "startEpochMs")?,
        command: parse_string_field(&value, "command")?,
        entry_realpath: parse_string_field(&value, "entryRealpath")?,
        generation_token: parse_string_field(&value, "generationToken")?,
    })
}

fn local_exec_identity_json(identity: &LocalExecProcessIdentity) -> Value {
    json!({
        "pid": identity.pid,
        "startEpochMs": identity.start_epoch_ms,
        "command": identity.command,
        "entryRealpath": identity.entry_realpath,
        "generationToken": identity.generation_token,
    })
}

struct CoordinatorLocalExecControl {
    state: Arc<CoordinatorState>,
}

impl LocalExecControl for CoordinatorLocalExecControl {
    fn now_ms(&self) -> u64 {
        coordinator_now_ms()
    }

    fn sleep_ms(&mut self, duration_ms: u64) {
        thread::sleep(Duration::from_millis(duration_ms));
    }

    fn resolve_gateway_connection(&mut self) -> Result<Value, LocalExecSupervisorError> {
        control_command(&self.state, "resolveGatewayConnection", json!({}))
            .map_err(local_exec_error)
    }

    fn mint_local_exec_daemon_credential(
        &mut self,
    ) -> Result<Option<Value>, LocalExecSupervisorError> {
        let value = control_command(&self.state, "mintLocalExecDaemonCredential", json!({}))
            .map_err(local_exec_error)?;
        Ok((!value.is_null()).then_some(value))
    }

    fn spawn_local_exec_daemon(
        &mut self,
        request: SpawnLocalExecDaemonRequest,
    ) -> Result<LocalExecProcessIdentity, LocalExecSupervisorError> {
        let SpawnLocalExecDaemonRequest { mut env, log_path } = request;
        env.insert(
            "MAHAYANA_APP_HOST_BIN".into(),
            self.state.host_bin.to_string_lossy().into_owned(),
        );
        let value = control_command(
            &self.state,
            "spawnLocalExecDaemon",
            json!({
                "env": env,
                "logPath": log_path.to_string_lossy(),
            }),
        )
        .map_err(local_exec_error)?;
        parse_local_exec_identity(value)
    }

    fn is_process_alive(&mut self, pid: u32) -> Result<bool, LocalExecSupervisorError> {
        control_command(&self.state, "isProcessAlive", json!({ "pid": pid }))
            .map_err(local_exec_error)?
            .as_bool()
            .ok_or_else(|| LocalExecSupervisorError::new("isProcessAlive returned a non-boolean"))
    }

    fn get_process_identity(
        &mut self,
        expected: &ExpectedLocalExecProcessIdentity,
    ) -> Result<Option<LocalExecProcessIdentity>, LocalExecSupervisorError> {
        let value = control_command(
            &self.state,
            "getProcessIdentity",
            json!({
                "pid": expected.pid,
                "entryRealpath": expected.entry_realpath,
                "generationToken": expected.generation_token,
                "startEpochMs": expected.start_epoch_ms,
                "command": expected.command,
                "discoveryStartedAt": expected.discovery_started_at,
            }),
        )
        .map_err(local_exec_error)?;
        if value.is_null() {
            return Ok(None);
        }
        parse_local_exec_identity(value).map(Some)
    }

    fn terminate_process(
        &mut self,
        identity: &LocalExecProcessIdentity,
    ) -> Result<bool, LocalExecSupervisorError> {
        let value = control_command(
            &self.state,
            "terminateProcess",
            json!({ "identity": local_exec_identity_json(identity) }),
        )
        .map_err(local_exec_error)?;
        value
            .get("terminated")
            .and_then(Value::as_bool)
            .ok_or_else(|| LocalExecSupervisorError::new("terminateProcess returned no terminated flag"))
    }
}

fn signal_local_exec(state: &Arc<CoordinatorState>, command: LocalExecRuntimeCommand) {
    let sender = state
        .local_exec_commands
        .lock()
        .ok()
        .and_then(|sender| sender.clone());
    if let Some(sender) = sender {
        let _ = sender.send(command);
    }
}

fn ensure_local_exec_runtime_started(state: &Arc<CoordinatorState>) {
    let mut slot = match state.local_exec_commands.lock() {
        Ok(slot) => slot,
        Err(_) => return,
    };
    if slot.is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    *slot = Some(sender);
    drop(slot);

    let runtime_state = Arc::clone(state);
    let data_dir = runtime_state.bootstrap.process_config.data_dir.clone();
    let is_packaged = runtime_state.bootstrap.process_config.is_packaged;
    thread::spawn(move || {
        let control = CoordinatorLocalExecControl {
            state: Arc::clone(&runtime_state),
        };
        let mut runtime = LocalExecDaemonRuntime::new(data_dir, is_packaged, control);
        runtime.start();
        let mut last_refresh = Instant::now();
        loop {
            if runtime_state.closed.load(Ordering::SeqCst) {
                break;
            }
            match receiver.recv_timeout(Duration::from_millis(
                LOCAL_EXEC_DAEMON_LIVENESS_INTERVAL_MS,
            )) {
                Ok(LocalExecRuntimeCommand::Refresh) => runtime.refresh_tick(),
                Ok(LocalExecRuntimeCommand::SetPaused(paused)) => runtime.set_paused(paused),
                Ok(LocalExecRuntimeCommand::Dispose) => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            runtime.liveness_tick();
            if last_refresh.elapsed()
                >= Duration::from_millis(LOCAL_EXEC_DAEMON_REFRESH_INTERVAL_MS)
            {
                runtime.refresh_tick();
                last_refresh = Instant::now();
            }
        }
        runtime.dispose();
    });
}

fn coordinator_repo_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.file_name().is_some_and(|name| name == "desktop") {
        return cwd.parent().unwrap_or(Path::new(".")).to_path_buf();
    }
    cwd
}

fn coordinator_resources_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .and_then(|bin| bin.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

fn webauthn_relying_party_id(ceremony: &WebAuthnCeremony) -> String {
    let options = ceremony
        .payload
        .get("optionsJson")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    options
        .as_ref()
        .and_then(|value| value.get("rpId").and_then(Value::as_str))
        .or_else(|| {
            options
                .as_ref()
                .and_then(|value| value.get("rp"))
                .and_then(|rp| rp.get("id"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty())
        .unwrap_or(ceremony.origin.as_str())
        .to_string()
}

fn webauthn_control_state(
    state: &Weak<CoordinatorState>,
) -> Result<Arc<CoordinatorState>, String> {
    state
        .upgrade()
        .ok_or_else(|| "Coordinator closed during WebAuthn request".to_string())
}

fn ensure_webauthn_runtime_started(state: &Arc<CoordinatorState>) {
    let mut slot = match state.webauthn_runtime.lock() {
        Ok(slot) => slot,
        Err(_) => return,
    };
    if slot.is_some() {
        return;
    }

    let override_path = env::var_os("SAND_WEBAUTHN_SIGNER_PATH").map(PathBuf::from);
    let signer_path = resolve_web_authn_signer_path(
        state.bootstrap.process_config.is_packaged,
        &coordinator_resources_path(),
        &coordinator_repo_root(),
        override_path.as_deref(),
    );
    let Some(signer_path) = signer_path else {
        return;
    };

    let connection_state = Arc::downgrade(state);
    let consent_state = Arc::downgrade(state);
    let status_state = Arc::downgrade(state);
    let pin_state = Arc::downgrade(state);
    let finish_state = Arc::downgrade(state);

    let options = ProductionWebAuthnRuntimeOptions {
        signer: SpawnedWebAuthnSigner {
            binary_path: signer_path,
        },
        resolve_connection: Arc::new(move || {
            let state = webauthn_control_state(&connection_state)
                .map_err(GatewayDispatchError::Transport)?;
            let generation = state.host_generation.load(Ordering::SeqCst);
            wait_for_gateway_connection(&state, generation)
                .map_err(|error| GatewayDispatchError::Transport(error.to_string()))
        }),
        request_consent: Arc::new(move |ceremony| {
            let state = webauthn_control_state(&consent_state)?;
            let value = control_command(
                &state,
                "requestWebAuthnConsent",
                json!({
                    "origin": ceremony.origin,
                    "rpId": webauthn_relying_party_id(ceremony),
                }),
            )
            .map_err(|error| error.message)?;
            let approved = value
                .get("approved")
                .and_then(Value::as_bool)
                .ok_or_else(|| "requestWebAuthnConsent returned no approved flag".to_string())?;
            if !approved {
                return Ok(ApprovedWebAuthnConsent::declined());
            }
            Ok(ApprovedWebAuthnConsent {
                approved: true,
                prompt_id: value
                    .get("promptId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                window_handle: value
                    .get("windowHandle")
                    .and_then(Value::as_u64),
            })
        }),
        update_status: Arc::new(move |status| {
            let Ok(state) = webauthn_control_state(&status_state) else {
                return;
            };
            let _ = control_command(
                &state,
                "updateWebAuthnConsent",
                json!({ "status": status }),
            );
        }),
        request_pin: Arc::new(move |request, prompt_id| {
            let Ok(state) = webauthn_control_state(&pin_state) else {
                return None;
            };
            let mut args = json!({
                "promptId": prompt_id,
                "invalid": request.invalid,
            });
            if let Some(retries) = request.retries {
                args["retries"] = Value::from(retries);
            }
            control_command(&state, "requestWebAuthnPin", args)
                .ok()
                .and_then(|value| value.get("pin").and_then(Value::as_str).map(str::to_string))
                .filter(|pin| !pin.is_empty())
        }),
        finish_consent: Arc::new(move || {
            let Ok(state) = webauthn_control_state(&finish_state) else {
                return;
            };
            let _ = control_command(&state, "finishWebAuthnConsent", json!({}));
        }),
        computer_id: None,
        label: env::var("HOSTNAME")
            .ok()
            .or_else(|| env::var("COMPUTERNAME").ok()),
    };
    let mut runtime = ProductionWebAuthnRuntime::new(options);
    runtime.start();
    *slot = Some(runtime);
}

fn set_webauthn_paused(state: &Arc<CoordinatorState>, paused: bool) {
    if let Ok(mut slot) = state.webauthn_runtime.lock() {
        if let Some(runtime) = slot.as_mut() {
            if paused {
                runtime.stop();
            } else {
                runtime.start();
            }
        }
    }
}

fn dispose_webauthn_runtime(state: &Arc<CoordinatorState>) {
    let runtime = state
        .webauthn_runtime
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    if let Some(mut runtime) = runtime {
        runtime.dispose();
    }
}

fn coordinator_now_ms() -> u64 {
    u64::try_from(Utc::now().timestamp_millis()).unwrap_or_default()
}

fn close_oauth_listener(state: &Arc<CoordinatorState>, origin: &str) {
    let listener = state
        .oauth_listeners
        .lock()
        .ok()
        .and_then(|mut listeners| listeners.remove(origin));
    if let Some(listener) = listener {
        if let Ok(mut registry) = state.oauth_loopback.lock() {
            let _ = listener.close(&mut registry);
        }
    }
}

fn complete_oauth_callback(
    state: &Arc<CoordinatorState>,
    callback: OAuthCallback,
) -> Result<(), Failure> {
    let generation = state.host_generation.load(Ordering::SeqCst);
    let payload = json!({
        "code": callback.code,
        "state": callback.state,
    });
    let mut last_failure = None;
    for attempt in 0..2 {
        let connection = match wait_for_gateway_connection(state, generation) {
            Ok(connection) => connection,
            Err(error) => {
                let failure = Failure::new(
                    "MCP_OAUTH_COMPLETION_UNAVAILABLE",
                    format!("Host gateway unavailable during OAuth completion: {error}"),
                );
                if attempt == 0 {
                    last_failure = Some(failure);
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                return Err(failure);
            }
        };
        match dispatch_http_json(&connection, "completeMcpOAuth", payload.clone()) {
            Ok(_) => return Ok(()),
            Err(error) => {
                let retryable = matches!(
                    error,
                    GatewayDispatchError::Unreachable { .. }
                        | GatewayDispatchError::Transport(_)
                );
                let failure = failure_for(&error);
                if attempt == 0 && retryable {
                    last_failure = Some(failure);
                    thread::sleep(Duration::from_millis(25));
                    continue;
                }
                return Err(failure);
            }
        }
    }
    Err(last_failure.unwrap_or_else(|| {
        Failure::new(
            "MCP_OAUTH_COMPLETION_UNAVAILABLE",
            "OAuth completion exhausted its bounded Host gateway retry",
        )
    }))
}

fn apply_oauth_actions(state: &Arc<CoordinatorState>, actions: Vec<OAuthForwarderAction>) {
    for action in actions {
        match action {
            OAuthForwarderAction::CloseListener { origin } => {
                close_oauth_listener(state, &origin);
            }
            OAuthForwarderAction::Complete { callback } => {
                if let Err(error) = complete_oauth_callback(state, callback) {
                    eprintln!(
                        "node-agent-coordinator: mcp-oauth completion failed: {}",
                        error.message
                    );
                }
            }
            OAuthForwarderAction::StartListener {
                origin,
                redirect_url,
                state: pending_state,
            } => {
                let resolve_state = Arc::clone(state);
                let resolve_origin = origin.clone();
                let callback_state = Arc::clone(state);
                let settled_state = Arc::clone(state);
                let settled_origin = origin.clone();

                let listener = {
                    let mut registry = match state.oauth_loopback.lock() {
                        Ok(registry) => registry,
                        Err(_) => {
                            let actions = state
                                .oauth_forwarder
                                .lock()
                                .map(|mut forwarder| {
                                    forwarder.listener_failed(
                                        &origin,
                                        &pending_state,
                                        coordinator_now_ms(),
                                    )
                                })
                                .unwrap_or_default();
                            apply_oauth_actions(state, actions);
                            continue;
                        }
                    };
                    start_mcp_oauth_callback_listener(
                        &redirect_url,
                        &mut registry,
                        move |callback_state_value| {
                            resolve_state
                                .oauth_forwarder
                                .lock()
                                .ok()
                                .and_then(|forwarder| {
                                    forwarder
                                        .resolve_server(
                                            &resolve_origin,
                                            callback_state_value,
                                            coordinator_now_ms(),
                                        )
                                        .map(str::to_string)
                                })
                        },
                        move |callback| complete_oauth_callback(&callback_state, callback),
                        move |callback_state_value| {
                            let state_value = callback_state_value.to_string();
                            let settle_state = Arc::clone(&settled_state);
                            let settle_origin = settled_origin.clone();
                            thread::spawn(move || {
                                let actions = settle_state
                                    .oauth_forwarder
                                    .lock()
                                    .map(|mut forwarder| {
                                        forwarder.settle_callback(
                                            &settle_origin,
                                            &state_value,
                                            coordinator_now_ms(),
                                        )
                                    })
                                    .unwrap_or_default();
                                apply_oauth_actions(&settle_state, actions);
                            });
                        },
                    )
                };

                match listener {
                    Ok(listener) => {
                        if let Ok(mut listeners) = state.oauth_listeners.lock() {
                            if let Some(previous) = listeners.insert(origin.clone(), listener) {
                                if let Ok(mut registry) = state.oauth_loopback.lock() {
                                    let _ = previous.close(&mut registry);
                                }
                            }
                        }
                        let actions = state
                            .oauth_forwarder
                            .lock()
                            .map(|mut forwarder| {
                                forwarder.listener_started(&origin, coordinator_now_ms())
                            })
                            .unwrap_or_default();
                        apply_oauth_actions(state, actions);
                    }
                    Err(error) => {
                        eprintln!(
                            "node-agent-coordinator: mcp-oauth listener start failed for {}: {}",
                            origin,
                            error.message
                        );
                        let actions = state
                            .oauth_forwarder
                            .lock()
                            .map(|mut forwarder| {
                                forwarder.listener_failed(
                                    &origin,
                                    &pending_state,
                                    coordinator_now_ms(),
                                )
                            })
                            .unwrap_or_default();
                        apply_oauth_actions(state, actions);
                    }
                }
            }
        }
    }
}

fn handle_oauth_pending(state: &Arc<CoordinatorState>, payload: &Value) {
    let Some(redirect_url) = payload.get("redirectUrl").and_then(Value::as_str) else {
        eprintln!("node-agent-coordinator: mcp-oauth pending event missing redirectUrl");
        return;
    };
    let Some(oauth_state) = payload.get("state").and_then(Value::as_str) else {
        eprintln!("node-agent-coordinator: mcp-oauth pending event missing state");
        return;
    };
    let Some(server_name) = payload.get("serverName").and_then(Value::as_str) else {
        eprintln!("node-agent-coordinator: mcp-oauth pending event missing serverName");
        return;
    };
    let actions = state
        .oauth_forwarder
        .lock()
        .map_err(|_| ())
        .and_then(|mut forwarder| {
            forwarder
                .handle_pending(
                    McpOAuthPendingPayload {
                        redirect_url: redirect_url.to_string(),
                        state: oauth_state.to_string(),
                        server_name: server_name.to_string(),
                    },
                    coordinator_now_ms(),
                )
                .map_err(|error| {
                    eprintln!(
                        "node-agent-coordinator: invalid mcp-oauth pending event: {}",
                        error.message
                    );
                })
        })
        .unwrap_or_default();
    apply_oauth_actions(state, actions);
}

fn start_oauth_expiry_loop(state: Arc<CoordinatorState>) {
    thread::spawn(move || {
        while !state.closed.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(1));
            let actions = state
                .oauth_forwarder
                .lock()
                .map(|mut forwarder| forwarder.expire(coordinator_now_ms()))
                .unwrap_or_default();
            apply_oauth_actions(&state, actions);
        }
    });
}

fn post_tool_event(state: &Arc<CoordinatorState>, event: mahayana_node_agent_coordinator::client_side_tool_v2_relay::RendererToolEvent) {
    if let Ok(payload) = serde_json::to_value(event) {
        state.post_event(CLIENT_SIDE_TOOL_V2_FAMILY, payload);
    }
}

fn replay_tool_events(state: &Arc<CoordinatorState>) {
    let events = state
        .tool_relay
        .lock()
        .map(|relay| relay.replay())
        .unwrap_or_default();
    for event in events {
        post_tool_event(state, event);
    }
}

fn report_agent_inbound_failure_best_effort(
    state: &Arc<CoordinatorState>,
    agent_id: &str,
    error: &Failure,
) {
    if let Err(report_error) = dispatch_gateway_value(
        state,
        "reportAgentInboundFailure",
        agent_inbound_failure_gateway_args(agent_id, &error.code, &error.message),
    ) {
        eprintln!(
            "agent inbound failure could not report agent={}: {}: {}",
            agent_id, report_error.code, report_error.message
        );
    }
}

fn accept_agent_inbound_message_for_execution(
    state: &Arc<CoordinatorState>,
    agent_id: &str,
    args: &mut Value,
) -> Result<bool, Failure> {
    if args.get("requestSource").and_then(Value::as_str) != Some("agent-inbound")
        || args.get("groupContext").is_some_and(|value| !value.is_null())
    {
        return Ok(true);
    }
    let already_displayed = args
        .get("agentWake")
        .and_then(Value::as_object)
        .and_then(|wake| wake.get("isDisplayed"))
        .and_then(Value::as_bool)
        == Some(true);
    if already_displayed {
        return Ok(true);
    }
    let Some(inbound) = args
        .get("agentWake")
        .and_then(Value::as_object)
        .and_then(|wake| wake.get("inbound"))
        .cloned()
    else {
        return Ok(true);
    };
    let result = dispatch_gateway_value(
        state,
        "acceptAgentInboundMessage",
        json!({ "agentId": agent_id, "inbound": inbound }),
    )?;
    if result.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Ok(false);
    }
    if let Some(wake) = args
        .get_mut("agentWake")
        .and_then(Value::as_object_mut)
    {
        wake.insert("isDisplayed".into(), Value::Bool(true));
    }
    Ok(true)
}

fn enqueue_agent_inbound_wake(
    state: &Arc<CoordinatorState>,
    agent_id: String,
    args: Value,
) -> Result<(), Failure> {
    let provider = routed_inference_provider(state, &agent_id);
    let urgent = agent_inbound_wake_prefers_urgent_queue(&args);
    let worker_state = Arc::clone(state);
    let queue_key = agent_id.clone();
    let task = move || {
        let mut args = args;
        match accept_agent_inbound_message_for_execution(&worker_state, &agent_id, &mut args) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                report_agent_inbound_failure_best_effort(&worker_state, &agent_id, &error);
                return;
            }
        }
        if provider == InferenceProvider::Cursor {
            if let Err(error) = dispatch_gateway_value(&worker_state, "sendPrompt", args.clone()) {
                if let Some(redrive_args) =
                    redrive_agent_inbound_after_priority_preemption(&args, &error.message)
                {
                    if let Err(redrive_error) = enqueue_agent_inbound_wake(
                        &worker_state,
                        agent_id.clone(),
                        redrive_args,
                    ) {
                        eprintln!(
                            "priority Cursor peer wake redrive could not enqueue agent={}: {}: {}",
                            agent_id, redrive_error.code, redrive_error.message
                        );
                    }
                } else {
                    report_agent_inbound_failure_best_effort(&worker_state, &agent_id, &error);
                }
            }
            return;
        }
        if let Err(error) =
            execute_local_inference(Arc::clone(&worker_state), provider, args.clone())
        {
            if error.code == "INFERENCE_PROVIDER_CANCELLED" {
                if let Some(redrive_args) =
                    redrive_agent_inbound_after_priority_preemption(&args, &error.message)
                {
                    if let Err(redrive_error) = enqueue_agent_inbound_wake(
                        &worker_state,
                        agent_id.clone(),
                        redrive_args,
                    ) {
                        eprintln!(
                            "priority peer wake redrive could not enqueue agent={}: {}: {}",
                            agent_id, redrive_error.code, redrive_error.message
                        );
                    }
                }
            } else {
                report_agent_inbound_failure_best_effort(&worker_state, &agent_id, &error);
            }
        }
    };
    if urgent {
        state.inference_queue.enqueue_urgent(&queue_key, task)
    } else {
        state.inference_queue.enqueue(&queue_key, task)
    }
}

fn dispatch_gateway_event(state: &Arc<CoordinatorState>, value: Value) {
    let (channel, family, payload) = match (
        value.get("channel").and_then(Value::as_str),
        value.get("payload"),
    ) {
        (Some(channel), Some(payload)) => (
            channel.to_string(),
            event_family_for_channel(channel).to_string(),
            payload.clone(),
        ),
        _ => ("runtime".into(), "runtime".into(), value),
    };
    if channel == "native-shell" {
        state.queue_native_shell_event(payload);
        return;
    }

    let now_ms = coordinator_now_ms();
    if let Ok(mut gateway) = state.gateway_client.lock() {
        let _ = gateway.accept_event(now_ms, channel.clone(), payload.clone());
    }

    if channel == "agent-inbound-wake-request" {
        match prepare_agent_inbound_wake_routes(&payload) {
            Ok(routes) => {
                for route in routes {
                    if let Err(failure) = enqueue_agent_inbound_wake(
                        state,
                        route.agent_id.clone(),
                        route.send_args,
                    ) {
                        eprintln!(
                            "agent inbound wake could not enqueue agent={}: {}: {}",
                            route.agent_id, failure.code, failure.message
                        );
                    }
                }
            }
            Err(failure) => eprintln!(
                "agent inbound wake rejected: {}: {}",
                failure.code, failure.message
            ),
        }
        return;
    }

    if channel == HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL {
        let request_id = payload
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let relay_state = Arc::clone(state);
        let relay_payload = payload;
        let spawn = thread::Builder::new()
            .name("mahayana-coordinator-host-mcp-lifecycle-relay".into())
            .spawn(move || {
                let resolution = match execute_host_mcp_lifecycle_request(
                    &relay_payload,
                    |method, args| control_command(&relay_state, method, args),
                ) {
                    Ok(resolution) => resolution,
                    Err(failure) => {
                        let Some(request_id) = request_id.as_deref() else {
                            eprintln!(
                                "Host MCP lifecycle relay rejected malformed request: {}: {}",
                                failure.code, failure.message
                            );
                            return;
                        };
                        host_mcp_lifecycle_resolution_failure(
                            request_id,
                            format!("{}: {}", failure.code, failure.message),
                        )
                    }
                };
                if let Err(failure) = dispatch_gateway_value(
                    &relay_state,
                    MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD,
                    resolution,
                ) {
                    eprintln!(
                        "Host MCP lifecycle relay could not settle Host request: {}: {}",
                        failure.code, failure.message
                    );
                }
            });
        if let Err(error) = spawn {
            eprintln!("Host MCP lifecycle relay worker could not start: {error}");
        }
        return;
    }

    if channel == RUNNER_TOOL_REQUEST_EVENT_CHANNEL {
        let request_id = payload
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let relay_state = Arc::clone(state);
        let relay_payload = payload;
        let spawn = thread::Builder::new()
            .name("mahayana-coordinator-runner-tool-relay".into())
            .spawn(move || {
                let resolution = match execute_runner_tool_request(
                    &relay_payload,
                    |method, args| control_command(&relay_state, method, args),
                ) {
                    Ok(resolution) => resolution,
                    Err(failure) => {
                        let Some(request_id) = request_id.as_deref() else {
                            eprintln!(
                                "runner tool relay rejected malformed request: {}: {}",
                                failure.code, failure.message
                            );
                            return;
                        };
                        runner_tool_resolution_failure(
                            request_id,
                            format!("{}: {}", failure.code, failure.message),
                        )
                    }
                };
                if let Err(failure) = dispatch_gateway_value(
                    &relay_state,
                    RUNNER_RESOLVE_ROUTED_TOOL_GATEWAY_METHOD,
                    resolution,
                ) {
                    eprintln!(
                        "runner tool relay could not settle Host request: {}: {}",
                        failure.code, failure.message
                    );
                }
            });
        if let Err(error) = spawn {
            eprintln!("runner tool relay worker could not start: {error}");
        }
        return;
    }

    if channel == "runner-inference" {
        let stream_id = payload
            .get("streamId")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !stream_id.is_empty() {
            let sender = state
                .inference_streams
                .lock()
                .ok()
                .and_then(|streams| streams.get(stream_id).cloned());
            if let Some(sender) = sender {
                let _ = sender.send(payload);
            }
        }
        return;
    }

    if channel == "mcp-oauth-pending" {
        handle_oauth_pending(state, &payload);
        return;
    }

    if channel == CLIENT_SIDE_TOOL_V2_FAMILY {
        let projected = state
            .tool_relay
            .lock()
            .ok()
            .and_then(|mut relay| relay.accept_value(payload));
        if let Some(event) = projected {
            post_tool_event(state, event);
        }
        return;
    }

    state.post_event(&family, payload);
}

fn run_gateway_event_stream(
    state: Arc<CoordinatorState>,
    generation: u64,
    mut connection: GatewayConnection,
) {
    loop {
        if state.closed.load(Ordering::SeqCst)
            || state.host_generation.load(Ordering::SeqCst) != generation
        {
            state.gateway_events_live.store(false, Ordering::SeqCst);
            return;
        }
        let transport_suppressed = state
            .gateway_client
            .lock()
            .map(|gateway| gateway.is_transport_suppressed())
            .unwrap_or(true);
        if transport_suppressed {
            state.gateway_events_live.store(false, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        let on_connected_state = Arc::clone(&state);
        let on_event_state = Arc::clone(&state);
        let continue_state = Arc::clone(&state);
        let result = stream_http_events(
            &connection,
            move || {
                on_connected_state
                    .gateway_events_live
                    .store(true, Ordering::SeqCst);
                let now_ms =
                    u64::try_from(Utc::now().timestamp_millis()).unwrap_or_default();
                if let Ok(mut gateway) = on_connected_state.gateway_client.lock() {
                    let _ = gateway.start(now_ms);
                }
                if let Ok(mut supervisor) = on_connected_state.host_supervisor.lock() {
                    supervisor.mark_transport_live(true);
                    supervisor.record_health(now_ms, true);
                }
                signal_local_exec(&on_connected_state, LocalExecRuntimeCommand::Refresh);
                on_connected_state.gateway_lifecycle(
                    "transport-connected",
                    generation,
                    None,
                );
            },
            move |event| dispatch_gateway_event(&on_event_state, event),
            move || {
                !continue_state.closed.load(Ordering::SeqCst)
                    && continue_state.host_generation.load(Ordering::SeqCst) == generation
                    && continue_state
                        .gateway_client
                        .lock()
                        .map(|gateway| !gateway.is_transport_suppressed())
                        .unwrap_or(false)
            },
        );

        state.gateway_events_live.store(false, Ordering::SeqCst);
        if state.closed.load(Ordering::SeqCst)
            || state.host_generation.load(Ordering::SeqCst) != generation
        {
            return;
        }

        let outcome = match &result {
            Err(GatewayDispatchError::Unreachable { outcome, .. }) => *outcome,
            Err(GatewayDispatchError::Command(_)) => ReachabilityOutcome::AccessDenied,
            Err(GatewayDispatchError::Transport(_)) | Ok(()) => ReachabilityOutcome::Network,
        };
        let detail = result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "gateway event stream ended".into());
        let delay = state
            .gateway_client
            .lock()
            .ok()
            .and_then(|mut gateway| gateway.transport_down(outcome));
        if let Ok(mut supervisor) = state.host_supervisor.lock() {
            supervisor.mark_transport_live(false);
            supervisor.invalidate();
        }
        state.gateway_lifecycle("transport-down", generation, Some(&detail));
        let Some(delay) = delay else {
            return;
        };
        thread::sleep(delay);

        if state.closed.load(Ordering::SeqCst)
            || state.host_generation.load(Ordering::SeqCst) != generation
        {
            return;
        }
        match read_gateway_discovery(&state.gateway_discovery_path) {
            Ok(next_connection) => {
                if let Ok(mut supervisor) = state.host_supervisor.lock() {
                    let attempt = supervisor.begin_connection_attempt();
                    let _ = supervisor.settle_connection_attempt(attempt, next_connection.clone());
                }
                if let Ok(mut gateway) = state.gateway_client.lock() {
                    let _ = gateway.install_connection(next_connection.clone());
                }
                connection = next_connection;
            }
            Err(error) => {
                state.gateway_lifecycle(
                    "rediscovery-failed",
                    generation,
                    Some(&error.to_string()),
                );
            }
        }
    }
}

fn spawn_host(state: Arc<CoordinatorState>) -> io::Result<u64> {
    let spawn_guard = state
        .spawn_lock
        .lock()
        .map_err(|_| io::Error::other("spawn lock poisoned"))?;
    if state.closed.load(Ordering::SeqCst) {
        return Err(io::Error::other("coordinator is closed"));
    }
    if let Ok(active) = state.host_stdin.lock() {
        if active.is_some() {
            return Ok(state.host_generation.load(Ordering::SeqCst));
        }
    }

    let generation = state.host_generation.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut supervisor) = state.host_supervisor.lock() {
        supervisor.invalidate();
    }
    let _ = fs::remove_file(&state.gateway_discovery_path);
    let mut command = Command::new(&state.host_bin);
    command.env(
        "SAND_DATA_ROOT",
        state.bootstrap.process_config.data_dir.trim(),
    );
    if let Some(local_human_id) = state.bootstrap.process_config.local_human_id.as_deref() {
        command.env("FABUSHI_LOCAL_HUMAN_ID", local_human_id.trim());
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            io::Error::other(format!(
                "failed to spawn Mahayana Host {}: {error}",
                state.host_bin.display()
            ))
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Host stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Host stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Host stderr unavailable"))?;
    *state
        .host_stdin
        .lock()
        .map_err(|_| io::Error::other("host stdin lock poisoned"))? =
        Some(ActiveHostStdin { generation, stdin });
    state.lifecycle("running", generation, true, None);
    drop(spawn_guard);

    {
        let discovery_state = Arc::clone(&state);
        thread::spawn(move || {
            for _ in 0..500 {
                if discovery_state.closed.load(Ordering::SeqCst)
                    || discovery_state.host_generation.load(Ordering::SeqCst) != generation
                {
                    return;
                }
                if let Ok(connection) =
                    read_gateway_discovery(&discovery_state.gateway_discovery_path)
                {
                    let supervised = discovery_state
                        .host_supervisor
                        .lock()
                        .ok()
                        .is_some_and(|mut supervisor| {
                            let attempt = supervisor.begin_connection_attempt();
                            supervisor
                                .settle_connection_attempt(attempt, connection.clone())
                                .is_ok()
                        });
                    let installed = supervised
                        && discovery_state
                            .gateway_client
                            .lock()
                            .ok()
                            .is_some_and(|mut gateway| {
                                gateway.install_connection(connection.clone()).is_ok()
                            });
                    if installed {
                        discovery_state.gateway_lifecycle(
                            "discovered",
                            generation,
                            None,
                        );
                        run_gateway_event_stream(
                            Arc::clone(&discovery_state),
                            generation,
                            connection,
                        );
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
            discovery_state.gateway_lifecycle(
                "discovery-timeout",
                generation,
                Some("Host gateway discovery did not appear"),
            );
        });
    }

    {
        let output_state = Arc::clone(&state);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if output_state.host_generation.load(Ordering::SeqCst) != generation {
                    continue;
                }
                let value = match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        output_state.consecutive_crashes.store(0, Ordering::SeqCst);
                        value
                    }
                    Err(error) => {
                        output_state.lifecycle(
                            "protocol-error",
                            generation,
                            true,
                            Some(&format!("invalid Host JSON: {error}")),
                        );
                        continue;
                    }
                };

                if let Some(event) = value.get("event").filter(|event| event.is_object()) {
                    let native_shell = event
                        .get("channel")
                        .and_then(Value::as_str)
                        .is_some_and(|channel| channel == "native-shell");
                    if native_shell || !output_state.gateway_events_live.load(Ordering::SeqCst) {
                        // Host stdout is the fallback for gateway-projected events.
                        // Native-shell adoption is Host-local and never duplicated by SSE,
                        // so it must always cross this lane even after SSE is live.
                        dispatch_gateway_event(&output_state, event.clone());
                    }
                    continue;
                }

                output_state.lifecycle(
                    "protocol-error",
                    generation,
                    true,
                    Some(
                        "unexpected non-event Host stdout frame; Coordinator business requests must use the Host gateway",
                    ),
                );
            }
        });
    }

    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            eprintln!("[mahayana-host:generation-{generation}] {line}");
        }
    });

    thread::spawn(move || {
        let status = child.wait();
        if let Ok(mut active) = state.host_stdin.lock() {
            if active
                .as_ref()
                .is_some_and(|entry| entry.generation == generation)
            {
                *active = None;
            }
        }
        let detail = match status {
            Ok(status) => format!("Host exited with {status}"),
            Err(error) => format!("Host wait failed: {error}"),
        };
        if let Ok(mut gateway) = state.gateway_client.lock() {
            let _ = gateway.transport_down(ReachabilityOutcome::Network);
        }
        if let Ok(mut supervisor) = state.host_supervisor.lock() {
            supervisor.mark_transport_live(false);
            supervisor.invalidate();
        }
        state.reject_generation(generation, &detail);
        state.lifecycle("stopped", generation, true, Some(&detail));

        if state.closed.load(Ordering::SeqCst) {
            return;
        }
        let crashes = state.consecutive_crashes.fetch_add(1, Ordering::SeqCst) + 1;
        if crashes >= HOST_CRASH_CIRCUIT_LIMIT {
            state.lifecycle(
                "circuit-open",
                generation,
                true,
                Some(&format!(
                    "Host crash circuit opened after {crashes} consecutive startup failures"
                )),
            );
            return;
        }
        let delay_ms =
            (250_u64.saturating_mul(1_u64 << crashes.saturating_sub(1).min(4))).min(4_000);
        thread::sleep(Duration::from_millis(delay_ms));
        if !state.closed.load(Ordering::SeqCst) {
            if let Err(error) = spawn_host(Arc::clone(&state)) {
                state.lifecycle(
                    "spawn-failed",
                    state.host_generation.load(Ordering::SeqCst),
                    true,
                    Some(&error.to_string()),
                );
            }
        }
    });

    Ok(generation)
}

fn wait_for_gateway_connection(
    state: &Arc<CoordinatorState>,
    generation: u64,
) -> io::Result<GatewayConnection> {
    for _ in 0..500 {
        if state.closed.load(Ordering::SeqCst) {
            return Err(io::Error::other("coordinator closed while waiting for Host gateway"));
        }
        if state.host_generation.load(Ordering::SeqCst) != generation {
            return Err(io::Error::other("Host generation changed while waiting for gateway"));
        }
        if let Some(connection) = state
            .host_supervisor
            .lock()
            .ok()
            .and_then(|supervisor| supervisor.connection().cloned())
        {
            return Ok(connection);
        }
        if let Some(connection) = state
            .gateway_client
            .lock()
            .ok()
            .and_then(|gateway| gateway.state.connection.clone())
        {
            return Ok(connection);
        }
        // The discovery worker normally installs this asynchronously. Reading
        // it here as well removes the historical stdin business-request
        // fallback without introducing a startup race for the first command.
        if let Ok(connection) = read_gateway_discovery(&state.gateway_discovery_path) {
            if let Ok(mut supervisor) = state.host_supervisor.lock() {
                let attempt = supervisor.begin_connection_attempt();
                let _ = supervisor.settle_connection_attempt(attempt, connection.clone());
            }
            if let Ok(mut gateway) = state.gateway_client.lock() {
                let _ = gateway.install_connection(connection.clone());
            }
            return Ok(connection);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Host gateway discovery did not become ready before command dispatch",
    ))
}

fn refresh_gateway_trace_window_async(state: &Arc<CoordinatorState>) {
    let now_ms = coordinator_now_ms();
    if !state
        .gateway_command_policy
        .trace_window_root_needs_refresh(now_ms)
        || state
            .trace_window_refresh_inflight
            .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let refresh_state = Arc::clone(state);
    thread::spawn(move || {
        let root = control_command(
            &refresh_state,
            "getRpcTraceWindowTraceparent",
            json!({}),
        )
        .ok()
        .and_then(|value| value.as_str().map(str::to_string));
        refresh_state
            .gateway_command_policy
            .cache_trace_window_root(root, coordinator_now_ms());
        refresh_state
            .trace_window_refresh_inflight
            .store(false, Ordering::SeqCst);
    });
}

fn report_gateway_execution_async(
    state: &Arc<CoordinatorState>,
    spans: Vec<GatewayCommandSpan>,
    stages: Vec<GatewayTransportStage>,
) {
    if spans.is_empty() && stages.is_empty() {
        return;
    }
    let report_state = Arc::clone(state);
    thread::spawn(move || {
        for stage in stages {
            let _ = control_command(
                &report_state,
                "reportTransportStage",
                json!({
                    "accountSlot": stage.account_slot,
                    "clientNonce": stage.client_nonce,
                    "stage": stage.stage,
                    "attempt": stage.attempt,
                    "traceparent": stage.traceparent,
                    "startEpochMs": stage.start_epoch_ms,
                    "durationMs": stage.duration_ms,
                    "isError": stage.is_error,
                }),
            );
        }
        for span in spans {
            let _ = control_command(
                &report_state,
                "reportGatewayCommandSpan",
                json!({
                    "method": span.method,
                    "rootTraceparent": span.root_traceparent,
                    "spanId": span.span_id,
                    "startEpochMs": span.start_epoch_ms,
                    "durationMs": span.duration_ms,
                    "isError": span.is_error,
                }),
            );
        }
    });
}


fn dispatch_gateway_value(
    state: &Arc<CoordinatorState>,
    method: &str,
    args: Value,
) -> Result<Value, Failure> {
    let host_running = state
        .host_stdin
        .lock()
        .map_err(|_| Failure::new("COORDINATOR_HOST_LOCK_FAILED", "Host stdin lock poisoned"))?
        .is_some();
    if !host_running {
        spawn_host(Arc::clone(state)).map_err(|error| {
            Failure::new(
                "COORDINATOR_HOST_SPAWN_FAILED",
                format!("could not start Mahayana Host: {error}"),
            )
        })?;
    }

    let generation = state.host_generation.load(Ordering::SeqCst);
    refresh_gateway_trace_window_async(state);
    let result = dispatch_gateway_command(
        &state.gateway_command_policy,
        method,
        args,
        coordinator_now_ms(),
        |required_base_url| {
            let connection = wait_for_gateway_connection(state, generation)
                .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
            if let Some(required_base_url) = required_base_url {
                if connection.base_url != required_base_url {
                    return Err(GatewayDispatchError::Transport(format!(
                        "gateway endpoint changed from {required_base_url} to {}",
                        connection.base_url
                    )));
                }
            }
            Ok(connection)
        },
    );

    match result {
        Ok(GatewayCommandExecution {
            value,
            command_spans,
            transport_stages,
        }) => {
            report_gateway_execution_async(state, command_spans, transport_stages);
            Ok(value)
        }
        Err(error) => {
            if let GatewayDispatchError::Unreachable { outcome, .. } = &error {
                if let Ok(mut gateway) = state.gateway_client.lock() {
                    let _ = gateway.transport_down(*outcome);
                }
            }
            Err(failure_for(&error))
        }
    }
}

fn routed_inference_provider(
    state: &CoordinatorState,
    agent_id: &str,
) -> InferenceProvider {
    let route = state.inference_router.resolve(agent_id);
    InferenceProvider::parse(&route.provider).unwrap_or(InferenceProvider::Cursor)
}

fn configured_inference_provider(state: &CoordinatorState) -> InferenceProvider {
    routed_inference_provider(state, "")
}

fn project_inference_activity(
    agents: &[Value],
    agent_id: &str,
    running: bool,
) -> Vec<Value> {
    agents
        .iter()
        .map(|raw| {
            let Some(root) = raw.as_object() else {
                return raw.clone();
            };
            if root.get("id").and_then(Value::as_str) != Some(agent_id) {
                return raw.clone();
            }
            let mut projected = root.clone();
            projected.insert("isRunning".into(), Value::Bool(running));
            projected.insert("isRunningTurn".into(), Value::Bool(running));
            projected.insert("isComposingMessage".into(), Value::Bool(running));
            projected.insert("isRetrying".into(), Value::Bool(false));
            if running {
                projected.insert("currentActivity".into(), json!({ "kind": "thinking" }));
            } else {
                projected.remove("currentActivity");
            }
            Value::Object(projected)
        })
        .collect()
}

struct ActiveInferenceStreamGuard {
    state: Arc<CoordinatorState>,
    agent_id: String,
    stream_id: String,
}

impl Drop for ActiveInferenceStreamGuard {
    fn drop(&mut self) {
        self.state
            .active_inference_streams
            .finish(&self.agent_id, &self.stream_id);
    }
}

struct InferenceActivityGuard {
    state: Arc<CoordinatorState>,
    agent_id: String,
    idle_agents: Vec<Value>,
    stop: Option<Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for InferenceActivityGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.state.post_event(
            "agents",
            json!({
                "activeAgentId": self.agent_id,
                "agents": self.idle_agents,
            }),
        );
    }
}

fn begin_inference_activity(
    state: &Arc<CoordinatorState>,
    agent_id: &str,
) -> Option<InferenceActivityGuard> {
    let remote = dispatch_gateway_value(state, "listAgents", json!({})).ok()?;
    let agents = remote.as_array()?.clone();
    let running_agents = project_inference_activity(&agents, agent_id, true);
    let idle_agents = project_inference_activity(&agents, agent_id, false);
    state.post_event(
        "agents",
        json!({
            "activeAgentId": agent_id,
            "agents": running_agents,
        }),
    );

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let pulse_state = Arc::clone(state);
    let pulse_agent_id = agent_id.to_string();
    let pulse_agents = running_agents.clone();
    let worker = thread::spawn(move || {
        loop {
            match stop_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if pulse_state.closed.load(Ordering::SeqCst) {
                        break;
                    }
                    pulse_state.post_event(
                        "agents",
                        json!({
                            "activeAgentId": pulse_agent_id,
                            "agents": pulse_agents,
                        }),
                    );
                }
            }
        }
    });

    Some(InferenceActivityGuard {
        state: Arc::clone(state),
        agent_id: agent_id.to_string(),
        idle_agents,
        stop: Some(stop_tx),
        worker: Some(worker),
    })
}

fn remote_transcript_ids(value: &Value) -> Vec<String> {
    value
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn record_inference_error(
    state: &Arc<CoordinatorState>,
    provider: InferenceProvider,
    agent_id: &str,
    error: &Failure,
) {
    if agent_id.trim().is_empty() {
        return;
    }
    let timestamp_ms = coordinator_now_ms();
    let entry = StoredEntry {
        provider: provider.as_str().to_string(),
        role: StoredRole::Assistant,
        content: format!("Router error: {}", error.message),
        rich_text: None,
        id: format!("t{timestamp_ms}s0"),
        client_nonce: None,
        attachments: Vec::new(),
        reactions: Vec::new(),
        timestamp_ms,
    };
    let _ = state
        .inference_store_lock
        .lock()
        .map_err(|_| ())
        .and_then(|_guard| {
            state
                .inference_store
                .append(agent_id, [entry])
                .map(|_| ())
                .map_err(|_| ())
        });
}

fn wait_for_runner_event_stream(state: &Arc<CoordinatorState>) -> Result<(), Failure> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if state.gateway_events_live.load(Ordering::SeqCst) {
            return Ok(());
        }
        if state.closed.load(Ordering::SeqCst) {
            return Err(Failure::new(
                "INFERENCE_RUNNER_STREAM_CLOSED",
                "Coordinator closed before the Host Runner event stream became live",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(Failure::new(
        "INFERENCE_RUNNER_STREAM_UNAVAILABLE",
        "Host Runner event stream did not become live before inference dispatch",
    ))
}

fn cancel_runner_stream_best_effort(
    state: &Arc<CoordinatorState>,
    stream_id: &str,
    reason: &str,
) {
    let _ = dispatch_gateway_value(
        state,
        "runner.cancelRoutedProvider",
        json!({
            "streamId": stream_id,
            "reason": reason,
        }),
    );
}

fn execute_local_inference(
    state: Arc<CoordinatorState>,
    provider: InferenceProvider,
    args: Value,
) -> Result<(), Failure> {
    let root = args.as_object().ok_or_else(|| {
        Failure::new(
            "INFERENCE_ROUTER_INVALID_REQUEST",
            "local inference routing requires an object request",
        )
    })?;
    let agent_id = root
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let prompt = root
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let runner_prompt = root
        .get("_runnerPrompt")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| prompt.clone());
    let rich_text = root
        .get("richText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let client_nonce = root
        .get("clientNonce")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let attachments = parse_send_prompt_attachments(&args)?;
    let append_user_message = should_append_user_message(&args);

    if agent_id.is_empty() || (prompt.is_empty() && attachments.is_empty()) {
        return Err(Failure::new(
            "INFERENCE_ROUTER_INVALID_PROMPT",
            "local inference routing requires an agentId and prompt or attachment",
        ));
    }

    // Register the turn before any Host transcript read/admission delay. A
    // newer direct user send can therefore supersede this turn even while the
    // Host has not yet accepted its provider stream.
    let stream_id = uuid::Uuid::new_v4().to_string();
    state
        .active_inference_streams
        .begin(&agent_id, &stream_id)?;
    let _active_stream = ActiveInferenceStreamGuard {
        state: Arc::clone(&state),
        agent_id: agent_id.clone(),
        stream_id: stream_id.clone(),
    };

    let remote = dispatch_gateway_value(
        &state,
        "getAgentTranscriptTail",
        json!({ "id": agent_id }),
    )?;
    let timestamp_ms = coordinator_now_ms();

    let mut acceptance_args = args.clone();
    if let Some(object) = acceptance_args.as_object_mut() {
        object.remove("_runnerPrompt");
        object.insert("clientNonce".into(), Value::String(client_nonce.clone()));
        object.insert("streamId".into(), Value::String(stream_id.clone()));
    }
    let host_acceptance = parse_host_routed_prompt_acceptance(
        &dispatch_gateway_value(
            &state,
            "runner.acceptRoutedPrompt",
            acceptance_args,
        )?,
    )?;
    if host_acceptance.duplicate {
        return Ok(());
    }

    let (turn, messages, recent_user_messages, current_message_id) = {
        let _guard = state.inference_store_lock.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_STORE_LOCK_FAILED",
                "inference transcript lock poisoned",
            )
        })?;
        let mut store = state.inference_store.load();
        let turn = store.next_turn_number(&agent_id, remote_transcript_ids(&remote));
        let host_user_message_id = host_acceptance.user_message_id.clone();
        let current_message_id = host_user_message_id
            .clone()
            .or_else(|| host_acceptance.echo_entry_id.clone())
            .unwrap_or_else(|| format!("t{turn}u"));

        if append_user_message {
            let user_entry = StoredEntry {
                provider: provider.as_str().to_string(),
                role: StoredRole::User,
                content: prompt,
                rich_text,
                id: current_message_id.clone(),
                client_nonce: Some(client_nonce),
                attachments,
                reactions: Vec::new(),
                timestamp_ms,
            };
            store.append(&agent_id, [user_entry]);
            state.inference_store.persist(&store)?;
        }

        // Provider history must come from the Host-owned Session/Transcript,
        // because user-visible Agent replies are durably written there as
        // `send-message` entries by the SendMessage tool. The Coordinator's
        // inference store is only a local projection and can lag those replies.
        // `remote` was read before Host admission of this turn, so append the
        // current runner prompt exactly once after projecting prior history.
        let mut messages = provider_messages_from_host_transcript(&remote);
        messages.push(json!({
            "role": "user",
            "content": runner_prompt,
        }));
        (
            turn,
            messages,
            host_acceptance.recent_user_messages.clone(),
            current_message_id,
        )
    };

    let activity = begin_inference_activity(&state, &agent_id);
    // The live Runner event stream is the production readiness gate. An older
    // local-inference implementation inserted a fixed 1.2s presentation delay
    // here; carrying that delay into routed Host execution needlessly stalls
    // every first-party Fabushi turn after durable Host admission.
    wait_for_runner_event_stream(&state)?;

    let assistant_timestamp_ms = coordinator_now_ms();
    let assistant_id = format!("t{turn}s0");
    let (stream_tx, stream_rx) = mpsc::channel::<Value>();
    {
        let mut streams = state.inference_streams.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_RUNNER_STREAM_LOCK_FAILED",
                "Runner inference stream registry lock poisoned",
            )
        })?;
        streams.insert(stream_id.clone(), stream_tx);
    }

    let result = (|| -> Result<String, Failure> {
        let mut runner_args = json!({
            "provider": provider.as_str(),
            "agentId": agent_id,
            "streamId": stream_id,
            "messages": messages,
        });
        let turn_context =
            project_runner_turn_context(&args, &current_message_id, recent_user_messages);
        if let (Some(target), Some(context)) =
            (runner_args.as_object_mut(), turn_context.as_object())
        {
            target.extend(context.clone());
        }
        let accepted = dispatch_gateway_value(
            &state,
            "runner.startRoutedProvider",
            runner_args,
        )?;
        if accepted.get("accepted").and_then(Value::as_bool) != Some(true) {
            return Err(Failure::new(
                "INFERENCE_RUNNER_REJECTED",
                "Host Runner did not accept the routed provider request",
            ));
        }
        if state
            .active_inference_streams
            .mark_accepted(&agent_id, &stream_id)?
        {
            cancel_runner_stream_best_effort(
                &state,
                &stream_id,
                "Superseded by a newer user message before Runner acceptance settled",
            );
        }

        let started = Instant::now();
        loop {
            match stream_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(value) => {
                    let (event_stream_id, event) = parse_runner_inference_event(&value)?;
                    if event_stream_id != stream_id {
                        continue;
                    }
                    match event {
                        // Provider deltas are runner-internal progress. The user-visible
                        // transcript is owned exclusively by the Host Session store and
                        // its canonical send-message events, so never project these
                        // summaries onto the renderer transcript channel.
                        RunnerInferenceEvent::Delta { .. } => {}
                        RunnerInferenceEvent::Completed { content } => return Ok(content),
                        RunnerInferenceEvent::Failed { message } => {
                            return Err(Failure::new("INFERENCE_PROVIDER_FAILED", message));
                        }
                        RunnerInferenceEvent::Cancelled { message } => {
                            return Err(Failure::new(
                                "INFERENCE_PROVIDER_CANCELLED",
                                message,
                            ));
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if state.closed.load(Ordering::SeqCst) {
                        cancel_runner_stream_best_effort(
                            &state,
                            &stream_id,
                            "Coordinator closed while waiting for the Host Runner",
                        );
                        return Err(Failure::new(
                            "INFERENCE_RUNNER_STREAM_CLOSED",
                            "Coordinator closed while waiting for the Host Runner",
                        ));
                    }
                    if started.elapsed() >= Duration::from_secs(30 * 60) {
                        cancel_runner_stream_best_effort(
                            &state,
                            &stream_id,
                            "Coordinator inference safety deadline exceeded",
                        );
                        return Err(Failure::new(
                            "INFERENCE_RUNNER_TIMEOUT",
                            "Host Runner inference exceeded the 30 minute safety deadline",
                        ));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Failure::new(
                        "INFERENCE_RUNNER_STREAM_DISCONNECTED",
                        "Host Runner inference event stream disconnected",
                    ));
                }
            }
        }
    })();

    if let Ok(mut streams) = state.inference_streams.lock() {
        streams.remove(&stream_id);
    }
    drop(activity);
    let content = result?;

    let assistant_entry = StoredEntry {
        provider: provider.as_str().to_string(),
        role: StoredRole::Assistant,
        content,
        rich_text: None,
        id: assistant_id,
        client_nonce: None,
        attachments: Vec::new(),
        reactions: Vec::new(),
        timestamp_ms: assistant_timestamp_ms,
    };
    {
        let _guard = state.inference_store_lock.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_STORE_LOCK_FAILED",
                "inference transcript lock poisoned",
            )
        })?;
        state
            .inference_store
            .append(&agent_id, [assistant_entry.clone()])?;
    }
    // Keep the provider-local completion for diagnostics/recovery only. The Host
    // has already persisted the canonical user-visible send-message entry.
    Ok(())
}

fn authoritative_host_transcript(
    state: &Arc<CoordinatorState>,
    method: &str,
    args: &Value,
) -> Result<Value, Failure> {
    dispatch_gateway_value(state, host_transcript_method(method), args.clone())
}

fn enqueue_inference_request(
    state: &Arc<CoordinatorState>,
    channel: CarrierChannel,
    request_id: &str,
    provider: InferenceProvider,
    args: Value,
) {
    let agent_id = args
        .get("agentId")
        .or_else(|| args.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if is_direct_user_send(&args) {
        if let InferenceStreamSupersede::CancelNow { stream_id } =
            state.active_inference_streams.request_supersede(&agent_id)
        {
            cancel_runner_stream_best_effort(
                state,
                &stream_id,
                "Superseded by a newer user message",
            );
        }
    }
    let queue_key = if agent_id.trim().is_empty() {
        format!("invalid-{}", uuid::Uuid::new_v4())
    } else {
        agent_id.clone()
    };
    let client_nonce = args
        .get("clientNonce")
        .and_then(Value::as_str)
        .map(str::to_string);
    let await_turn = should_await_turn(&args);
    let direct_user_send = is_direct_user_send(&args);
    let mut accepted_value = json!({
        "accepted": true,
        "provider": provider.as_str(),
    });
    if let Some(client_nonce) = client_nonce.as_ref() {
        accepted_value["clientNonce"] = Value::String(client_nonce.clone());
    }
    let terminal_accepted_value = accepted_value.clone();
    let terminal_request_id = request_id.to_string();
    let worker_state = Arc::clone(state);
    let worker_args = args;
    let task = move || {
        let result = execute_local_inference(Arc::clone(&worker_state), provider, worker_args);
        match result {
            Ok(()) => {
                if await_turn {
                    worker_state.complete_request(
                        channel,
                        &terminal_request_id,
                        ReplyOutcome::Ok {
                            value: terminal_accepted_value,
                        },
                    );
                }
            }
            Err(error) => {
                if error.code != "INFERENCE_PROVIDER_CANCELLED" {
                    record_inference_error(&worker_state, provider, &agent_id, &error);
                }
                if await_turn {
                    worker_state.complete_request(
                        channel,
                        &terminal_request_id,
                        ReplyOutcome::Failed { failure: error },
                    );
                }
            }
        }
    };
    let enqueue = if direct_user_send {
        state.inference_queue.enqueue_urgent(&queue_key, task)
    } else {
        state.inference_queue.enqueue(&queue_key, task)
    };
    match enqueue {
        Ok(()) if !await_turn => state.complete_request(
            channel,
            request_id,
            ReplyOutcome::Ok {
                value: accepted_value,
            },
        ),
        Ok(()) => {}
        Err(failure) => state.complete_request(
            channel,
            request_id,
            ReplyOutcome::Failed { failure },
        ),
    }
}

fn dispatch_inference_if_handled(
    state: &Arc<CoordinatorState>,
    channel: CarrierChannel,
    request_id: &str,
    method: &str,
    args: &Value,
) -> bool {
    let inference_agent_id = args
        .get("agentId")
        .or_else(|| args.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let provider = routed_inference_provider(state, inference_agent_id);

    if matches!(
        method,
        "getAgentTranscriptTail" | "openAgentTail" | "getAgentTranscriptWindow"
    ) {
        let worker_state = Arc::clone(state);
        let request_id = request_id.to_string();
        let method = method.to_string();
        let args = args.clone();
        thread::spawn(move || {
            match authoritative_host_transcript(&worker_state, &method, &args) {
                Ok(value) => worker_state.complete_request(
                    channel,
                    &request_id,
                    ReplyOutcome::Ok { value },
                ),
                Err(failure) => worker_state.complete_request(
                    channel,
                    &request_id,
                    ReplyOutcome::Failed { failure },
                ),
            }
        });
        return true;
    }

    if method == "runAgentWorkflowNow" {
        let worker_state = Arc::clone(state);
        let request_id = request_id.to_string();
        let args = args.clone();
        thread::spawn(move || {
            let agent_id = args
                .get("agentId")
                .or_else(|| args.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("")
                .to_string();
            let workflow_id = args
                .get("workflowId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("")
                .to_string();
            if agent_id.is_empty() || workflow_id.is_empty() {
                worker_state.complete_request(
                    channel,
                    &request_id,
                    ReplyOutcome::Failed {
                        failure: Failure::new(
                            "INFERENCE_WORKFLOW_INVALID",
                            "runAgentWorkflowNow requires agent id and workflowId",
                        ),
                    },
                );
                return;
            }
            let workflow = match dispatch_gateway_value(
                &worker_state,
                "getAgentWorkflow",
                json!({"id":agent_id,"workflowId":workflow_id}),
            ) {
                Ok(value) => value,
                Err(failure) => {
                    worker_state.complete_request(
                        channel,
                        &request_id,
                        ReplyOutcome::Failed { failure },
                    );
                    return;
                }
            };
            match prepare_workflow_run_now_route(&agent_id, &workflow) {
                Ok(CoordinatorWorkflowRunNowRoute::Missing) => worker_state.complete_request(
                    channel,
                    &request_id,
                    ReplyOutcome::Ok { value: Value::Null },
                ),
                Ok(CoordinatorWorkflowRunNowRoute::Automation) => {
                    match dispatch_gateway_value(&worker_state, "runAgentWorkflowNow", args) {
                        Ok(value) => worker_state.complete_request(
                            channel,
                            &request_id,
                            ReplyOutcome::Ok { value },
                        ),
                        Err(failure) => worker_state.complete_request(
                            channel,
                            &request_id,
                            ReplyOutcome::Failed { failure },
                        ),
                    }
                }
                Ok(CoordinatorWorkflowRunNowRoute::Reference { send_args }) => {
                    enqueue_inference_request(
                        &worker_state,
                        channel,
                        &request_id,
                        provider,
                        send_args,
                    );
                }
                Err(failure) => worker_state.complete_request(
                    channel,
                    &request_id,
                    ReplyOutcome::Failed { failure },
                ),
            }
        });
        return true;
    }

    if method != "sendPrompt" {
        return false;
    }

    enqueue_inference_request(state, channel, request_id, provider, args.clone());
    true
}

fn dispatch_to_host(
    state: &Arc<CoordinatorState>,
    channel: CarrierChannel,
    request_id: String,
    method: String,
    args: Value,
) -> io::Result<()> {
    if method == "coordinator.health" {
        let generation = state.host_generation.load(Ordering::SeqCst);
        let pending = state.pending.lock().map(|pending| pending.len()).unwrap_or_default();
        let (gateway_connected, gateway_generation, gateway_base_url) = state
            .gateway_client
            .lock()
            .map(|gateway| {
                (
                    gateway.state.connected,
                    gateway.state.endpoint_generation,
                    gateway
                        .state
                        .connection
                        .as_ref()
                        .map(|connection| connection.base_url.clone()),
                )
            })
            .unwrap_or((false, 0, None));
        state.complete_request(
            channel,
            &request_id,
            ReplyOutcome::Ok {
                value: json!({
                    "protocolVersion": 1,
                    "hostGeneration": generation,
                    "pending": pending,
                    "hostRunning": state.host_stdin.lock().map(|host| host.is_some()).unwrap_or(false),
                    "gateway": {
                        "connected": gateway_connected,
                        "endpointGeneration": gateway_generation,
                        "baseUrl": gateway_base_url
                    },
                    "processConfig": {
                        "appVersion": state.bootstrap.process_config.app_version.clone(),
                        "isPackaged": state.bootstrap.process_config.is_packaged,
                        "dataDir": state.bootstrap.process_config.data_dir.clone()
                    },
                    "inference": {
                        "provider": configured_inference_provider(state).as_str(),
                        "queueWorkers": state.inference_queue.worker_count()
                    }
                }),
            },
        );
        return Ok(());
    }

    let host_running = state
        .host_stdin
        .lock()
        .map_err(|_| io::Error::other("host stdin lock poisoned"))?
        .is_some();
    if !host_running {
        spawn_host(Arc::clone(state))?;
    }

    let generation = state.host_generation.load(Ordering::SeqCst);
    // Mirror Grok's trace-window behavior: refresh asynchronously so the
    // current command never waits on the Electron control port.
    refresh_gateway_trace_window_async(state);
    let host_request_id = format!("{}:{request_id}", channel.wire_name());
    state
        .pending
        .lock()
        .map_err(|_| io::Error::other("pending lock poisoned"))?
        .insert(
            host_request_id.clone(),
            PendingHostRequest {
                generation,
                request_id: request_id.clone(),
                channel,
            },
        );

    let deleted_agent_ids = deleted_agent_ids_for_host_success(&method, &args);
    let dispatch_state = Arc::clone(state);
    thread::spawn(move || {
        let command_now_ms = coordinator_now_ms();
        let result = dispatch_gateway_command(
            &dispatch_state.gateway_command_policy,
            &method,
            args,
            command_now_ms,
            |required_base_url| {
                let connection = wait_for_gateway_connection(&dispatch_state, generation)
                    .map_err(|error| GatewayDispatchError::Transport(error.to_string()))?;
                if let Some(required_base_url) = required_base_url {
                    if connection.base_url != required_base_url {
                        return Err(GatewayDispatchError::Transport(format!(
                            "gateway endpoint changed from {required_base_url} to {}",
                            connection.base_url
                        )));
                    }
                }
                Ok(connection)
            },
        );
        if matches!(result, Err(GatewayDispatchError::Unreachable { .. })) {
            // A Host crash closes the in-flight HTTP socket before the process
            // waiter can reject its generation. Give that owner a short window
            // to settle the request as COORDINATOR_DISCONNECTED. If the Host is
            // still alive, this is a genuine gateway/network failure and the
            // dispatch worker below owns the reply.
            for _ in 0..20 {
                let host_alive = dispatch_state
                    .host_stdin
                    .lock()
                    .ok()
                    .and_then(|active| active.as_ref().map(|entry| entry.generation))
                    == Some(generation);
                if !host_alive {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        let pending_request = dispatch_state
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&host_request_id));
        let Some(pending_request) = pending_request else {
            return;
        };
        match result {
            Ok(GatewayCommandExecution {
                value,
                command_spans,
                transport_stages,
            }) => {
                for agent_id in &deleted_agent_ids {
                    dispatch_state.inference_queue.clear_agent(agent_id);
                }
                dispatch_state.complete_request(
                    pending_request.channel,
                    &pending_request.request_id,
                    ReplyOutcome::Ok { value },
                );
                report_gateway_execution_async(
                    &dispatch_state,
                    command_spans,
                    transport_stages,
                );
            }
            Err(error) => {
                if let GatewayDispatchError::Unreachable { outcome, .. } = &error {
                    if let Ok(mut gateway) = dispatch_state.gateway_client.lock() {
                        let _ = gateway.transport_down(*outcome);
                    }
                }
                dispatch_state.complete_request(
                    pending_request.channel,
                    &pending_request.request_id,
                    ReplyOutcome::Failed {
                        failure: failure_for(&error),
                    },
                );
            }
        }
    });
    Ok(())
}

fn execute_actions(
    state: &Arc<CoordinatorState>,
    channel: CarrierChannel,
    actions: Vec<ServerAction>,
) -> bool {
    let mut close = false;
    for action in actions {
        match action {
            ServerAction::Post(frame) => {
                let renderer_serving_ready = channel == CarrierChannel::Data
                    && matches!(
                        &frame,
                        CoordinatorFrame::Lifecycle {
                            phase: LifecyclePhase::Ready,
                            ..
                        }
                    );
                let main_serving_ready = channel == CarrierChannel::MainData
                    && matches!(
                        &frame,
                        CoordinatorFrame::Lifecycle {
                            phase: LifecyclePhase::Ready,
                            ..
                        }
                    );
                let posted = state.write_frame(channel, &frame).is_ok();
                if renderer_serving_ready {
                    ensure_webauthn_runtime_started(state);
                    if !state.tool_replay_done.swap(true, Ordering::SeqCst) {
                        replay_tool_events(state);
                    }
                }
                if posted && main_serving_ready {
                    state.flush_native_shell_event();
                }
            }
            ServerAction::Dispatch {
                request_id,
                method,
                args,
            } => {
                if !miniapp_carrier_allowed(&method, channel) {
                    state.complete_request(
                        channel,
                        &request_id,
                        ReplyOutcome::Failed {
                            failure: Failure::new("MINIAPP_NATIVE_ONLY", "mini app intent requires native main-data carrier"),
                        },
                    );
                    continue;
                }
                if channel == CarrierChannel::MainData && method == "setGatewayPaused" {
                    let paused = args
                        .get("paused")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    signal_local_exec(state, LocalExecRuntimeCommand::SetPaused(paused));
                    set_webauthn_paused(state, paused);
                    let paused = state
                        .gateway_client
                        .lock()
                        .map(|mut gateway| gateway.set_client_paused(paused))
                        .unwrap_or(paused);
                    state.complete_request(
                        channel,
                        &request_id,
                        ReplyOutcome::Ok {
                            value: json!({ "paused": paused }),
                        },
                    );
                    continue;
                }
                if channel == CarrierChannel::MainData && method == "setDevGatewayOffline" {
                    let induced = args
                        .get("induced")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let induced = state
                        .gateway_client
                        .lock()
                        .map(|mut gateway| gateway.set_dev_induced_offline(induced))
                        .unwrap_or(induced);
                    state.complete_request(
                        channel,
                        &request_id,
                        ReplyOutcome::Ok {
                            value: json!({ "induced": induced }),
                        },
                    );
                    continue;
                }
                if let Ok(mut tdrp) = state.tdrp_state.lock() {
                    if let Some(outcome) = tdrp.dispatch(&method, &args) {
                        match outcome {
                            Ok(val) => {
                                state.complete_request(
                                    channel,
                                    &request_id,
                                    ReplyOutcome::Ok { value: val },
                                );
                            }
                            Err(failure) => {
                                state.complete_request(
                                    channel,
                                    &request_id,
                                    ReplyOutcome::Failed { failure },
                                );
                            }
                        }
                        continue;
                    }
                }
                if dispatch_inference_if_handled(
                    state,
                    channel,
                    &request_id,
                    &method,
                    &args,
                ) {
                    continue;
                }
                if let Err(error) =
                    dispatch_to_host(state, channel, request_id.clone(), method, args)
                {
                    state.complete_request(
                        channel,
                        &request_id,
                        ReplyOutcome::Failed {
                            failure: Failure::new(
                                "COORDINATOR_HOST_DISPATCH_FAILED",
                                error.to_string(),
                            ),
                        },
                    );
                }
            }
            ServerAction::Abort { request_id } => {
                state.abort_request(channel, &request_id);
            }
            ServerAction::Close => close = true,
        }
    }
    close
}

fn execute_control_actions(state: &Arc<CoordinatorState>, actions: Vec<ClientAction>) -> bool {
    let mut close = false;
    for action in actions {
        match action {
            ClientAction::Post(frame) => {
                let _ = state.write_frame(CarrierChannel::Control, &frame);
            }
            ClientAction::Close => close = true,
            ClientAction::Resolve { .. }
            | ClientAction::Reject { .. }
            | ClientAction::Event { .. } => {}
        }
    }
    close
}

fn main() {
    let arguments = env::args().collect::<Vec<_>>();
    let bootstrap = match parse_bootstrap_argument(arguments.iter().map(String::as_str)) {
        Ok(bootstrap) => bootstrap,
        Err(detail) => {
            eprintln!("node-agent-coordinator: {detail}");
            std::process::exit(2);
        }
    };

    let host_bin = match env::var_os("MAHAYANA_APP_HOST_BIN").filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => {
            eprintln!("MAHAYANA_APP_HOST_BIN is required");
            std::process::exit(2);
        }
    };

    let inference_data_dir = PathBuf::from(bootstrap.process_config.data_dir.trim());
    let gateway_discovery_path = inference_data_dir.join("gateway.json");
    let inference_settings_path = inference_data_dir.join("settings.json");
    let inference_fallback_provider =
        if env::var("FABUSHI_PRODUCT_MODE").ok().as_deref() == Some("1") {
            InferenceProvider::Fabushi
        } else {
            InferenceProvider::Cursor
        };
    let inference_router = CoordinatorInferenceRouter::new_with_fallback(
        inference_settings_path,
        inference_fallback_provider,
    );
    let inference_store =
        InferenceTranscriptFile::new(inference_data_dir.join("inference-router-transcript.json"));
    let gateway_client = CoordinatorGatewayClient::default();

    let state = Arc::new(CoordinatorState {
        bootstrap,
        host_bin,
        gateway_discovery_path,
        inference_router,
        inference_store,
        inference_store_lock: Mutex::new(()),
        inference_queue: InferenceTaskQueue::default(),
        active_inference_streams: ActiveInferenceStreamRegistry::default(),
        inference_streams: Mutex::new(HashMap::new()),
        host_stdin: Mutex::new(None),
        gateway_client: Mutex::new(gateway_client),
        gateway_command_policy: GatewayCommandPolicy::default(),
        host_supervisor: Mutex::new(GatewayHostSupervisor::new(HEALTH_PROBE_TTL_MS)),
        tool_relay: Mutex::new(ClientSideToolV2Relay::default()),
        tool_replay_done: AtomicBool::new(false),
        oauth_forwarder: Mutex::new(McpOAuthForwarderState::default()),
        oauth_loopback: Mutex::new(McpOAuthLoopbackRegistry::default()),
        oauth_listeners: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        control_port: Mutex::new(ControlPortClient::default()),
        local_exec_commands: Mutex::new(None),
        webauthn_runtime: Mutex::new(None),
        renderer_port: Mutex::new(RendererPortServer::default()),
        main_data_port: Mutex::new(RendererPortServer::default()),
        pending_native_shell_event: Mutex::new(None),
        stdout_lock: Mutex::new(()),
        spawn_lock: Mutex::new(()),
        host_generation: AtomicU64::new(0),
        lifecycle_sequence: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        consecutive_crashes: AtomicU64::new(0),
        gateway_events_live: AtomicBool::new(false),
        trace_window_refresh_inflight: AtomicBool::new(false),
        tdrp_state: Mutex::new(mahayana_node_agent_coordinator::tdrp::TdrpCoordinatorState::default()),
    });

    start_oauth_expiry_loop(Arc::clone(&state));

    if let Ok(control) = state.control_port.lock() {
        if let ClientAction::Post(frame) = control.start() {
            if state.write_frame(CarrierChannel::Control, &frame).is_err() {
                std::process::exit(1);
            }
        }
    }

    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("coordinator stdin read failed: {error}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let envelope = match serde_json::from_str::<CarrierEnvelope>(&line) {
            Ok(envelope) => envelope,
            Err(error) => {
                eprintln!("node-agent-coordinator: invalid carrier envelope: {error}");
                break;
            }
        };
        let Some(channel) = envelope.classify() else {
            eprintln!("node-agent-coordinator: unknown carrier channel {}", envelope.channel);
            break;
        };

        let close = match channel {
            CarrierChannel::Control => {
                let (actions, serving) = state
                    .control_port
                    .lock()
                    .map(|mut client| {
                        let actions = client.handle_value(envelope.frame);
                        let serving = client.phase() == ControlPortPhase::Serving;
                        (actions, serving)
                    })
                    .unwrap_or_default();
                let close = execute_control_actions(&state, actions);
                if serving {
                    ensure_local_exec_runtime_started(&state);
                }
                close
            }
            CarrierChannel::Data => {
                let actions = state
                    .renderer_port
                    .lock()
                    .map(|mut server| server.handle_value(envelope.frame))
                    .unwrap_or_default();
                execute_actions(&state, CarrierChannel::Data, actions)
            }
            CarrierChannel::MainData => {
                let actions = state
                    .main_data_port
                    .lock()
                    .map(|mut server| server.handle_value(envelope.frame))
                    .unwrap_or_default();
                execute_actions(&state, CarrierChannel::MainData, actions)
            }
        };
        if close {
            break;
        }
    }

    state.closed.store(true, Ordering::SeqCst);
    state.inference_queue.dispose();
    signal_local_exec(&state, LocalExecRuntimeCommand::Dispose);
    dispose_webauthn_runtime(&state);
    if let Ok(mut control) = state.control_port.lock() {
        let _ = control.handle_port_closed();
    }
    if let Ok(mut server) = state.renderer_port.lock() {
        let _ = server.handle_port_closed();
    }
    if let Ok(mut server) = state.main_data_port.lock() {
        let _ = server.handle_port_closed();
    }
    if let Ok(mut active) = state.host_stdin.lock() {
        active.take();
    }
    if let Ok(mut relay) = state.tool_relay.lock() {
        relay.clear();
    }
    let oauth_actions = state
        .oauth_forwarder
        .lock()
        .map(|mut forwarder| forwarder.dispose())
        .unwrap_or_default();
    apply_oauth_actions(&state, oauth_actions);
    if let Ok(mut registry) = state.oauth_loopback.lock() {
        registry.dispose();
    }
    let generation = state.host_generation.load(Ordering::SeqCst);
    state.reject_generation(generation, "Coordinator input closed");
}
