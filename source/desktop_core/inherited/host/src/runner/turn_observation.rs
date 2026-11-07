use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedToolDefinition,
};
use crate::extensions::transcript::async_task_union::AsyncTask;
use crate::extensions::transcript::client_side_tool_v2_projection::{
    ProjectedClientSideToolV2, ToolProjectionPhase, project_routed_tool_call,
    project_routed_tool_partial_raw,
};

use super::clock_skew_guard::{
    SEND_DISPATCH_MAX_PLAUSIBLE_MS, TTFT_MAX_PLAUSIBLE_MS,
    SanitizedCrossClockDuration, bucket_clock_skew_delta_ms,
    sanitize_cross_clock_duration_ms,
};
use super::conversation_outline::MCP_TOOL_CALL_OUTLINE_NAME;
use super::routed_provider_runtime::RoutedToolBridge;

pub const MCP_EXEC_STALL_THRESHOLD_MS: u64 = 15 * 60 * 1_000;
pub const RECENT_ACTIVITY_CAP: usize = 24;

pub type TurnObservationEventSink =
    Arc<dyn Fn(Value) + Send + Sync + 'static>;
pub type AsyncTasksProvider =
    Arc<dyn Fn(&str) -> Vec<AsyncTask> + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallTelemetryEvent {
    Started {
        conversation_id: String,
        request_id: Option<String>,
        tool_name: String,
        tool_call_id: String,
        surface: String,
    },
    Error {
        conversation_id: String,
        request_id: Option<String>,
        tool_name: String,
        tool_call_id: String,
        error_class: String,
        duration_ms: u64,
        connector: String,
    },
    Stalled {
        conversation_id: String,
        request_id: Option<String>,
        tool_name: String,
        tool_call_id: String,
        connector: String,
        elapsed_ms: u64,
    },
}

pub type ToolCallTelemetrySink =
    Arc<dyn Fn(ToolCallTelemetryEvent) + Send + Sync + 'static>;
pub type ClientSideToolV2ProjectionSink =
    Arc<dyn Fn(ProjectedClientSideToolV2) + Send + Sync + 'static>;

pub struct McpExecObservationGuard {
    cancel_stall: mpsc::Sender<()>,
    started: Instant,
    emit: ToolCallTelemetrySink,
    conversation_id: String,
    request_id: Option<String>,
    tool_call_id: String,
    connector: String,
}

impl McpExecObservationGuard {
    pub fn settle(self, error_class: Option<&str>) {
        let _ = self.cancel_stall.send(());
        if let Some(error_class) = error_class {
            (self.emit)(ToolCallTelemetryEvent::Error {
                conversation_id: self.conversation_id,
                request_id: self.request_id,
                tool_name: MCP_TOOL_CALL_OUTLINE_NAME.into(),
                tool_call_id: self.tool_call_id,
                error_class: error_class.to_string(),
                duration_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                connector: self.connector,
            });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivity {
    pub status: String,
    pub name: String,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnObservationSnapshot {
    pub elapsed_ms: u64,
    pub last_tool: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingAwait {
    index: u64,
    block_until_ms: u64,
}

pub struct TurnObservation {
    conversation_id: String,
    event_sink: Option<TurnObservationEventSink>,
    turn_await_sink: Option<TurnObservationEventSink>,
    first_token_sink: Option<TurnObservationEventSink>,
    send_dispatch_sink: Option<TurnObservationEventSink>,
    async_tasks_sink: Option<TurnObservationEventSink>,
    async_tasks_provider: Option<AsyncTasksProvider>,
    tool_call_telemetry_sink: Option<ToolCallTelemetrySink>,
    client_side_tool_v2_sink: Option<ClientSideToolV2ProjectionSink>,
    request_id: Option<String>,
    turn_started_at_ms: u64,
    last_tool: Option<String>,
    recent_activity: Vec<String>,
    observed_tool_call_count: u64,
    await_observation_count: u64,
    pending_awaits: HashMap<String, PendingAwait>,
    mcp_observed_tool_call_ids: HashSet<String>,
    provider_streamed_tool_call_ids: HashSet<String>,
    first_token_observed: bool,
    first_output_dispatch_started: Option<Instant>,
    first_output_model_id: Option<String>,
    first_output_is_fork: bool,
}

pub type TurnObservationHandle = Arc<Mutex<TurnObservation>>;

pub fn async_tasks_changed_event(
    parent_agent_id: &str,
    tasks: &[AsyncTask],
) -> Result<Value, String> {
    if parent_agent_id.trim().is_empty() {
        return Err("async-task owner conversation id is unavailable".into());
    }
    let mut sorted = tasks.to_vec();
    sorted.sort_by(|left, right| {
        left.started_at_ms
            .partial_cmp(&right.started_at_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(json!({
        "parentAgentId": parent_agent_id,
        "tasks": sorted,
    }))
}

impl TurnObservation {
    pub fn new(
        conversation_id: impl Into<String>,
        event_sink: Option<TurnObservationEventSink>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            event_sink,
            turn_await_sink: None,
            first_token_sink: None,
            send_dispatch_sink: None,
            async_tasks_sink: None,
            async_tasks_provider: None,
            tool_call_telemetry_sink: None,
            client_side_tool_v2_sink: None,
            request_id: None,
            turn_started_at_ms: now_ms(),
            last_tool: None,
            recent_activity: Vec::new(),
            observed_tool_call_count: 0,
            await_observation_count: 0,
            pending_awaits: HashMap::new(),
            mcp_observed_tool_call_ids: HashSet::new(),
            provider_streamed_tool_call_ids: HashSet::new(),
            first_token_observed: false,
            first_output_dispatch_started: None,
            first_output_model_id: None,
            first_output_is_fork: false,
        }
    }

    pub fn shared(
        conversation_id: impl Into<String>,
        event_sink: Option<TurnObservationEventSink>,
    ) -> TurnObservationHandle {
        Arc::new(Mutex::new(Self::new(conversation_id, event_sink)))
    }

    pub fn set_tool_call_telemetry_handler(&mut self, sink: ToolCallTelemetrySink) {
        self.tool_call_telemetry_sink = Some(sink);
    }

    pub fn set_client_side_tool_v2_handler(&mut self, sink: ClientSideToolV2ProjectionSink) {
        self.client_side_tool_v2_sink = Some(sink);
    }

    fn emit_client_side_tool_v2(&self, projected: ProjectedClientSideToolV2) {
        if let Some(sink) = self.client_side_tool_v2_sink.as_ref() {
            sink(projected);
        }
    }

    pub fn observe_provider_partial_tool_call(&mut self, partial: &ProviderPartialToolCall) {
        if partial.tool_call_id.trim().is_empty() {
            return;
        }
        let first = self
            .provider_streamed_tool_call_ids
            .insert(partial.tool_call_id.clone());
        let phase = if first {
            ToolProjectionPhase::Started
        } else {
            ToolProjectionPhase::Partial
        };
        if let Some(projected) = project_routed_tool_partial_raw(
            phase,
            &partial.tool,
            &partial.raw_arguments,
            &partial.tool_call_id,
            partial.model_call_id.as_deref().unwrap_or_default(),
        ) {
            self.emit_client_side_tool_v2(projected);
        }
    }

    pub fn set_async_tasks_provider(&mut self, provider: AsyncTasksProvider) {
        self.async_tasks_provider = Some(provider);
    }

    pub fn set_async_tasks_event_handler(&mut self, handler: TurnObservationEventSink) {
        self.async_tasks_sink = Some(handler);
    }

    pub fn list_async_tasks(&self) -> Vec<AsyncTask> {
        let mut tasks = self
            .async_tasks_provider
            .as_ref()
            .map(|provider| provider(&self.conversation_id))
            .unwrap_or_default();
        tasks.sort_by(|left, right| {
            left.started_at_ms
                .partial_cmp(&right.started_at_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        tasks
    }

    pub fn emit_async_tasks_changed(&self) -> Result<Vec<AsyncTask>, String> {
        let tasks = self.list_async_tasks();
        let event = async_tasks_changed_event(&self.conversation_id, &tasks)?;
        if let Some(sink) = self.async_tasks_sink.as_ref() {
            sink(event);
        }
        Ok(tasks)
    }

    pub fn set_request_id(&mut self, request_id: Option<String>) {
        self.request_id = request_id;
    }

    fn emit_tool_call_telemetry(&self, event: ToolCallTelemetryEvent) {
        if let Some(sink) = self.tool_call_telemetry_sink.as_ref() {
            sink(event);
        }
    }

    fn mark_mcp_tool_call(&mut self, tool_call_id: &str) {
        if self.mcp_observed_tool_call_ids.len() >= 256 {
            self.mcp_observed_tool_call_ids.clear();
        }
        self.mcp_observed_tool_call_ids
            .insert(tool_call_id.to_string());
    }

    fn take_mcp_tool_call_marker(&mut self, tool_call_id: &str) -> bool {
        self.mcp_observed_tool_call_ids.remove(tool_call_id)
    }

    pub fn begin_mcp_exec_observation(
        &self,
        tool_call_id: &str,
        connector: &str,
    ) -> Option<McpExecObservationGuard> {
        self.begin_mcp_exec_observation_with_threshold(
            tool_call_id,
            connector,
            Duration::from_millis(MCP_EXEC_STALL_THRESHOLD_MS),
        )
    }

    pub fn begin_mcp_exec_observation_with_threshold(
        &self,
        tool_call_id: &str,
        connector: &str,
        threshold: Duration,
    ) -> Option<McpExecObservationGuard> {
        let emit = self.tool_call_telemetry_sink.as_ref()?.clone();
        let timer_emit = Arc::clone(&emit);
        let (cancel_stall, stall_cancelled) = mpsc::channel();
        let conversation_id = self.conversation_id.clone();
        let request_id = self.request_id.clone();
        let tool_call_id_owned = tool_call_id.to_string();
        let connector_owned = connector.to_string();
        std::thread::spawn({
            let conversation_id = conversation_id.clone();
            let request_id = request_id.clone();
            let tool_call_id = tool_call_id_owned.clone();
            let connector = connector_owned.clone();
            move || {
                if matches!(
                    stall_cancelled.recv_timeout(threshold),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    (timer_emit)(ToolCallTelemetryEvent::Stalled {
                        conversation_id,
                        request_id,
                        tool_name: MCP_TOOL_CALL_OUTLINE_NAME.into(),
                        tool_call_id,
                        connector,
                        elapsed_ms: threshold.as_millis().min(u64::MAX as u128) as u64,
                    });
                }
            }
        });
        Some(McpExecObservationGuard {
            cancel_stall,
            started: Instant::now(),
            emit,
            conversation_id,
            request_id,
            tool_call_id: tool_call_id_owned,
            connector: connector_owned,
        })
    }

    pub fn turn_started(&mut self, at_ms: u64) {
        self.turn_started_at_ms = at_ms;
        self.first_token_observed = false;
        self.first_output_dispatch_started = None;
        self.first_output_model_id = None;
        self.first_output_is_fork = false;
        self.emit(json!({
            "type": "turn-started",
            "agentId": self.conversation_id,
            "at": at_ms,
        }));
    }

    pub fn tool_started(&mut self, name: &str) {
        self.last_tool = Some(name.to_string());
        self.record_tool_activity(ToolActivity {
            status: "pending".into(),
            name: name.to_string(),
            summary: None,
        });
        self.emit(json!({
            "type": "tool-started",
            "agentId": self.conversation_id,
            "name": name,
            "at": now_ms(),
        }));
    }

    pub fn tool_completed(
        &mut self,
        name: &str,
        failed: bool,
        summary: Option<&str>,
    ) {
        if self.last_tool.as_deref() == Some(name) {
            self.last_tool = None;
        }
        self.record_tool_activity(ToolActivity {
            status: if failed { "failed" } else { "done" }.into(),
            name: name.to_string(),
            summary: summary
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        });
        self.emit(json!({
            "type": "tool-completed",
            "agentId": self.conversation_id,
            "name": name,
            "failed": failed,
            "at": now_ms(),
        }));
    }

    pub fn record_tool_activity(&mut self, update: ToolActivity) {
        if matches!(update.status.as_str(), "done" | "failed") {
            self.observed_tool_call_count =
                self.observed_tool_call_count.saturating_add(1);
        }
        let summary = update
            .summary
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| format!(": {value}"))
            .unwrap_or_default();
        let line = format!("[{}] {}{summary}", update.status, update.name);
        if self.recent_activity.last() == Some(&line) {
            return;
        }
        self.recent_activity.push(line);
        if self.recent_activity.len() > RECENT_ACTIVITY_CAP {
            let drain = self.recent_activity.len() - RECENT_ACTIVITY_CAP;
            self.recent_activity.drain(0..drain);
        }
    }

    pub fn observe_first_token(
        &mut self,
        chunk_type: &str,
        dispatch_perf_ms: Option<f64>,
        observed_perf_ms: Option<f64>,
        model_id: Option<&str>,
        is_fork: bool,
    ) -> bool {
        if self.first_token_observed {
            return false;
        }
        self.first_token_observed = true;
        let timing = match (dispatch_perf_ms, observed_perf_ms) {
            (Some(dispatch), Some(observed)) => {
                let raw = observed - dispatch;
                let sanitized = sanitize_cross_clock_duration_ms(
                    raw,
                    TTFT_MAX_PLAUSIBLE_MS as f64,
                );
                match sanitized {
                    SanitizedCrossClockDuration::Millis(ms) => json!({
                        "ttftMs": ms,
                        "skew": false,
                    }),
                    SanitizedCrossClockDuration::Skew(reason) => json!({
                        "skew": true,
                        "skewReason": format!("{reason:?}"),
                        "skewBucket": bucket_clock_skew_delta_ms(raw),
                    }),
                }
            }
            _ => json!({}),
        };
        let mut event = json!({
            "type": "first-token",
            "agentId": self.conversation_id,
            "chunkType": chunk_type,
            "isFork": is_fork,
        });
        if let Some(model_id) = model_id.filter(|value| !value.is_empty()) {
            event["modelId"] = Value::String(model_id.to_string());
        }
        if let (Some(target), Some(source)) =
            (event.as_object_mut(), timing.as_object())
        {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        if let Some(sink) = self.first_token_sink.as_ref() {
            sink(event.clone());
        }
        self.emit(event);
        true
    }

    pub fn observe_stream_output(&mut self, chunk_type: &str) -> bool {
        let elapsed_ms = self
            .first_output_dispatch_started
            .map(|started| started.elapsed().as_secs_f64() * 1_000.0);
        let model_id = self.first_output_model_id.clone();
        let is_fork = self.first_output_is_fork;
        self.observe_first_token(
            chunk_type,
            elapsed_ms.map(|_| 0.0),
            elapsed_ms,
            model_id.as_deref(),
            is_fork,
        )
    }

    pub fn observe_send_dispatch(
        &mut self,
        host_receipt_perf_ms: f64,
        dispatch_perf_ms: f64,
        enter_epoch_ms: Option<f64>,
        dispatch_epoch_ms: f64,
        is_fork: bool,
        model_id: &str,
    ) {
        self.first_output_dispatch_started = Some(Instant::now());
        self.first_output_model_id =
            (!model_id.trim().is_empty()).then(|| model_id.to_string());
        self.first_output_is_fork = is_fork;
        let host_dispatch_ms = (dispatch_perf_ms - host_receipt_perf_ms)
            .max(0.0)
            .round() as u64;
        let mut event = json!({
            "type": "send-dispatch",
            "agentId": self.conversation_id,
            "hostDispatchMs": host_dispatch_ms,
            "isFork": is_fork,
            "modelId": model_id,
        });
        if let Some(enter) = enter_epoch_ms.filter(|value| value.is_finite()) {
            let raw = dispatch_epoch_ms - enter;
            match sanitize_cross_clock_duration_ms(
                raw,
                SEND_DISPATCH_MAX_PLAUSIBLE_MS as f64,
            ) {
                SanitizedCrossClockDuration::Millis(ms) => {
                    event["dispatchMs"] = json!(ms);
                    event["skew"] = Value::Bool(false);
                }
                SanitizedCrossClockDuration::Skew(reason) => {
                    event["skew"] = Value::Bool(true);
                    event["skewReason"] =
                        Value::String(format!("{reason:?}"));
                    event["skewBucket"] =
                        Value::String(bucket_clock_skew_delta_ms(raw).into());
                }
            }
        }
        if let Some(sink) = self.send_dispatch_sink.as_ref() {
            sink(event.clone());
        }
        self.emit(event);
    }

    pub fn report_turn_retry(&self, event: Value) {
        self.emit(json!({
            "type": "turn-retry",
            "agentId": self.conversation_id,
            "event": event,
        }));
    }

    pub fn set_turn_await_handler(&mut self, handler: TurnObservationEventSink) {
        self.turn_await_sink = Some(handler);
    }

    pub fn set_first_token_handler(&mut self, handler: TurnObservationEventSink) {
        self.first_token_sink = Some(handler);
    }

    pub fn set_send_dispatch_handler(&mut self, handler: TurnObservationEventSink) {
        self.send_dispatch_sink = Some(handler);
    }

    pub fn observe_communicate_tool_call(
        &self,
        phase: &str,
        tool_name: &str,
        tool_call_id: &str,
        wire: &[u8],
    ) {
        self.emit(json!({
            "type": "agent-tool-call",
            "agentId": self.conversation_id,
            "phase": phase,
            "tool": tool_name,
            "toolCallId": tool_call_id,
            "encoding": "agent.v1.protobuf",
            "wire": wire,
        }));
    }

    pub fn observe_await_tool_call(
        &mut self,
        started: bool,
        call_id: &str,
        block_until_ms: u64,
        result: Option<&str>,
        cancelled: bool,
        interrupted: bool,
    ) {
        if started {
            self.await_observation_count =
                self.await_observation_count.saturating_add(1);
            self.pending_awaits.insert(
                call_id.to_string(),
                PendingAwait {
                    index: self.await_observation_count,
                    block_until_ms,
                },
            );
            return;
        }

        let pending = self.pending_awaits.remove(call_id);
        let index = pending
            .as_ref()
            .map(|value| value.index)
            .unwrap_or_else(|| {
                self.await_observation_count =
                    self.await_observation_count.saturating_add(1);
                self.await_observation_count
            });
        let block_until_ms = pending
            .map(|value| value.block_until_ms)
            .unwrap_or(block_until_ms);
        let outcome = if cancelled {
            if interrupted { "aborted" } else { "clean_stop" }
        } else if result == Some("timeout") {
            "timeout"
        } else {
            "completed"
        };
        let observation = json!({
            "awaitIndex": index,
            "blockUntilMs": block_until_ms,
            "outcome": outcome,
        });
        if let Some(sink) = self.turn_await_sink.as_ref() {
            sink(observation.clone());
        }
        self.emit(json!({
            "type": "turn-await",
            "agentId": self.conversation_id,
            "awaitIndex": index,
            "blockUntilMs": block_until_ms,
            "outcome": outcome,
        }));
    }

    pub fn flush_pending_awaits_on_unwind(
        &mut self,
        interrupted: bool,
    ) {
        let outcome = if interrupted { "aborted" } else { "clean_stop" };
        let pending = self.pending_awaits.drain().collect::<Vec<_>>();
        for (_, item) in pending {
            let observation = json!({
                "awaitIndex": item.index,
                "blockUntilMs": item.block_until_ms,
                "outcome": outcome,
            });
            if let Some(sink) = self.turn_await_sink.as_ref() {
                sink(observation.clone());
            }
            self.emit(json!({
                "type": "turn-await",
                "agentId": self.conversation_id,
                "awaitIndex": item.index,
                "blockUntilMs": item.block_until_ms,
                "outcome": outcome,
            }));
        }
        self.await_observation_count = 0;
    }

    pub fn snapshot(&self, at_ms: u64) -> TurnObservationSnapshot {
        TurnObservationSnapshot {
            elapsed_ms: at_ms.saturating_sub(self.turn_started_at_ms),
            last_tool: self.last_tool.clone(),
        }
    }

    pub fn recent_activity(&self) -> Vec<String> {
        self.recent_activity.clone()
    }

    pub fn observed_tool_call_count(&self) -> u64 {
        self.observed_tool_call_count
    }

    fn emit(&self, event: Value) {
        if let Some(sink) = self.event_sink.as_ref() {
            sink(event);
        }
    }
}

pub struct McpObservedRoutedToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    observation: TurnObservationHandle,
}

impl McpObservedRoutedToolBridge {
    pub fn new(delegate: Arc<dyn RoutedToolBridge>, observation: TurnObservationHandle) -> Self {
        Self { delegate, observation }
    }
}

impl RoutedToolBridge for McpObservedRoutedToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.delegate.list_tools()
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let guard = self
            .observation
            .lock()
            .ok()
            .and_then(|mut observation| {
                observation.mark_mcp_tool_call(tool_call_id);
                observation.begin_mcp_exec_observation(
                    tool_call_id,
                    &bounded_connector_tag(&tool.provider_identifier),
                )
            });
        let result = self.delegate.call_tool(tool, args, tool_call_id);
        if let Some(guard) = guard {
            guard.settle(if result.is_err() { Some("mcp_error_result") } else { None });
        }
        result
    }
}

pub struct ObservedRoutedToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    observation: TurnObservationHandle,
}

impl ObservedRoutedToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        observation: TurnObservationHandle,
    ) -> Self {
        Self {
            delegate,
            observation,
        }
    }
}

impl RoutedToolBridge for ObservedRoutedToolBridge {
    fn list_tools(
        &self,
    ) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.delegate.list_tools()
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        if let Ok(mut observation) = self.observation.lock() {
            observation.observe_stream_output("tool-call");
            observation.observe_provider_partial_tool_call(partial);
        }
        Ok(())
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let name = if tool.tool_name.trim().is_empty() {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        let started = Instant::now();
        if let Ok(mut observation) = self.observation.lock() {
            observation.observe_stream_output("tool-call");
            observation.tool_started(name);
            if let Some(surface) = dual_surface_tool_surface(name) {
                observation.emit_tool_call_telemetry(ToolCallTelemetryEvent::Started {
                    conversation_id: observation.conversation_id.clone(),
                    request_id: observation.request_id.clone(),
                    tool_name: name.to_string(),
                    tool_call_id: tool_call_id.to_string(),
                    surface: surface.to_string(),
                });
            }
            if matches!(name, "AwaitShell" | "ExternalAwaitShell" | "awaitToolCall") {
                let block_until_ms = args
                    .get("block_until_ms")
                    .or_else(|| args.get("blockUntilMs"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                observation.observe_await_tool_call(
                    true,
                    tool_call_id,
                    block_until_ms,
                    None,
                    false,
                    false,
                );
            }
        }

        let provider_streamed = self
            .observation
            .lock()
            .map(|observation| observation.provider_streamed_tool_call_ids.contains(tool_call_id))
            .unwrap_or(false);
        if !provider_streamed {
            if let Some(projected) = project_routed_tool_call(
                ToolProjectionPhase::Started,
                tool,
                &args,
                tool_call_id,
                "",
                None,
            ) {
                if let Ok(observation) = self.observation.lock() {
                    observation.emit_client_side_tool_v2(projected);
                }
            }
        }

        let result = self.delegate.call_tool(tool, args.clone(), tool_call_id);

        let projection_result = Some(match &result {
            Ok(value) => Ok(value.clone()),
            Err(error) => Err(error.to_string()),
        });
        if let Some(projected) = project_routed_tool_call(
            ToolProjectionPhase::Completed,
            tool,
            &args,
            tool_call_id,
            "",
            projection_result,
        ) {
            if let Ok(observation) = self.observation.lock() {
                observation.emit_client_side_tool_v2(projected);
            }
        }
        if let Ok(mut observation) = self.observation.lock() {
            observation.provider_streamed_tool_call_ids.remove(tool_call_id);
            let failed = result.is_err();
            let summary = result.as_ref().ok().map(Value::to_string);
            observation.tool_completed(name, failed, summary.as_deref());
            let mcp_observed = observation.take_mcp_tool_call_marker(tool_call_id);
            if failed && !mcp_observed {
                observation.emit_tool_call_telemetry(ToolCallTelemetryEvent::Error {
                    conversation_id: observation.conversation_id.clone(),
                    request_id: observation.request_id.clone(),
                    tool_name: name.to_string(),
                    tool_call_id: tool_call_id.to_string(),
                    error_class: "task_error_result".into(),
                    duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    connector: "unknown".into(),
                });
            }
            if matches!(name, "AwaitShell" | "ExternalAwaitShell" | "awaitToolCall") {
                let block_until_ms = args
                    .get("block_until_ms")
                    .or_else(|| args.get("blockUntilMs"))
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let outcome = result
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("status").and_then(Value::as_str));
                observation.observe_await_tool_call(
                    false,
                    tool_call_id,
                    block_until_ms,
                    outcome,
                    false,
                    false,
                );
            }
        }
        result
    }
}

fn bounded_connector_tag(provider_identifier: &str) -> String {
    const KNOWN: &[&str] = &[
        "asana", "atlassian", "buildkite", "confluence", "context7", "databricks",
        "datadog", "deepwiki", "dock", "figma", "filesystem", "github", "gmail",
        "google", "googlecalendar", "googledocs", "googledrive", "googlesheets",
        "googleworkspace", "huggingface", "jira", "linear", "memory", "notion",
        "playwright", "salesforce", "sentry", "sequentialthinking", "slack", "stripe",
        "telegram", "todoist", "zoominfo",
    ];
    let normalized = provider_identifier
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        .collect::<String>();
    if normalized.is_empty() {
        "unknown".into()
    } else if KNOWN.contains(&normalized.as_str()) {
        normalized
    } else {
        "other".into()
    }
}

fn dual_surface_tool_surface(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "Shell" | "Read" | "AwaitShell" => Some("box"),
        "ExternalShell" | "ExternalRead" | "AwaitExternalShell" => Some("external"),
        _ => None,
    }
}

pub fn mcp_tool_call_outline_name() -> &'static str {
    MCP_TOOL_CALL_OUTLINE_NAME
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}
