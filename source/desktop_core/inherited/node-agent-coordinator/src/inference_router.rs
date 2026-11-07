use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::time::Duration;
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::protocol::Failure;

pub const INFERENCE_TRANSCRIPT_SCHEMA_VERSION: u32 = 2;
pub const INFERENCE_TRANSCRIPT_LIMIT: usize = 200;

pub fn deleted_agent_ids_for_host_success(method: &str, args: &Value) -> Vec<String> {
    match method {
        "deleteAgent" => args
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| vec![id.to_string()])
            .unwrap_or_default(),
        "deleteAgents" => args
            .get("ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Normalize renderer-facing transcript aliases before crossing the Host boundary.
/// The Rust Session gateway owns `getAgentTranscriptTail`; `openAgentTail`
/// remains a renderer/Coordinator convenience alias and must never leak to Host.
pub fn host_transcript_method(method: &str) -> &str {
    match method {
        "openAgentTail" => "getAgentTranscriptTail",
        other => other,
    }
}

/// Frozen send-turn semantics distinguish durable acceptance from terminal turn
/// settlement. Only an explicit boolean true asks the Coordinator to hold the
/// renderer request until the Host Runner publishes its terminal event.
pub fn should_await_turn(args: &Value) -> bool {
    args.get("awaitTurn").and_then(Value::as_bool) == Some(true)
}

/// Project the sendPrompt fields that materially define one user turn across
/// Coordinator -> Host -> Runner without linking the Coordinator to Host types.
/// The current message id is returned by authoritative Host/Session routed
/// prompt admission before the Runner starts.
pub fn project_runner_turn_context(
    send_args: &Value,
    message_id: &str,
    recent_user_messages: Vec<Value>,
) -> Value {
    let mut projected = serde_json::Map::new();
    if !message_id.trim().is_empty() {
        projected.insert("messageId".into(), Value::String(message_id.to_string()));
    }
    projected.insert(
        "recentUserMessages".into(),
        Value::Array(recent_user_messages),
    );
    for field in [
        "attachmentPaths",
        "selectedImages",
        "selectedVideos",
        "replyContext",
        "isFork",
        "richText",
        "composedAtMs",
        "enterEpochMs",
        "requestSource",
        "modelId",
        "isSummarizationSession",
        "isComputerUseSubagent",
        "isBrowserUseSubagent",
        "skipLabeling",
        "ackRedrive",
        "ackRedriveTrigger",
        "redriveAttempts",
    ] {
        if let Some(value) = send_args.get(field) {
            projected.insert(field.to_string(), value.clone());
        }
    }
    Value::Object(projected)
}


pub const WORKFLOW_REFERENCE_NODE_TYPE: &str = "workflowReference";
pub const WORKFLOW_INJECTED_BODY_LIMIT: usize = 8_000;

#[derive(Debug, Clone, PartialEq)]
pub enum CoordinatorWorkflowRunNowRoute {
    Missing,
    Automation,
    Reference { send_args: Value },
}

pub fn prepare_workflow_run_now_route(
    agent_id: &str,
    workflow: &Value,
) -> Result<CoordinatorWorkflowRunNowRoute, Failure> {
    if workflow.is_null() {
        return Ok(CoordinatorWorkflowRunNowRoute::Missing);
    }
    let workflow = workflow.as_object().ok_or_else(|| {
        Failure::new(
            "INFERENCE_WORKFLOW_INVALID",
            "getAgentWorkflow returned a non-object workflow",
        )
    })?;
    let string_field = |field: &str| {
        workflow
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let id = string_field("id").ok_or_else(|| {
        Failure::new("INFERENCE_WORKFLOW_INVALID", "workflow is missing id")
    })?;
    let name = string_field("name").ok_or_else(|| {
        Failure::new("INFERENCE_WORKFLOW_INVALID", "workflow is missing name")
    })?;
    let source = string_field("source").unwrap_or_else(|| "workflow".into());
    if source == "automation" {
        return Ok(CoordinatorWorkflowRunNowRoute::Automation);
    }

    let visible_prompt = format!("@{name}");
    let rich_text = serde_json::json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [{
                "type": WORKFLOW_REFERENCE_NODE_TYPE,
                "attrs": { "id": id, "label": name }
            }]
        }]
    })
    .to_string();

    let enabled = workflow
        .get("isEnabledForAgent")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let runner_prompt = if enabled {
        let identity = match source.as_str() {
            "managed" => format!("managed skill id {id}"),
            "plugin" => format!(
                "plugin skill id {id}, file {}",
                string_field("filePath").unwrap_or_default()
            ),
            _ => format!("folder {id}"),
        };
        let mut lines = vec![format!(
            "The user invoked the \"{name}\" workflow ({identity}). Run it now."
        )];
        if let Some(description) = string_field("description") {
            lines.push(format!("What it does: {description}"));
        }
        lines.push("Recipe to follow:".into());
        lines.push(
            workflow
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .chars()
                .take(WORKFLOW_INJECTED_BODY_LIMIT)
                .collect::<String>(),
        );
        let helper_scripts = workflow
            .get("helperScripts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !helper_scripts.is_empty() {
            let file_path = string_field("filePath").unwrap_or_default();
            let workflow_dir = Path::new(&file_path)
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            lines.push(format!(
                "Helper files live beside this workflow in {workflow_dir}: {}. Use them with Shell as the recipe directs.",
                helper_scripts.join(", ")
            ));
        }
        lines.push(
            "Carry out the recipe now, adapting it to anything else the user said in this message."
                .into(),
        );
        format!("{}\n\n{visible_prompt}", lines.join("\n"))
    } else {
        visible_prompt.clone()
    };

    Ok(CoordinatorWorkflowRunNowRoute::Reference {
        send_args: serde_json::json!({
            "agentId": agent_id,
            "prompt": visible_prompt,
            "richText": rich_text,
            "_runnerPrompt": runner_prompt,
            "awaitTurn": true,
            "source": "workflow-reference",
        }),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostRoutedPromptAcceptance {
    pub duplicate: bool,
    pub echo_entry_id: Option<String>,
    pub user_message_id: Option<String>,
    pub recent_user_messages: Vec<Value>,
}

pub fn parse_host_routed_prompt_acceptance(
    value: &Value,
) -> Result<HostRoutedPromptAcceptance, Failure> {
    if value.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(Failure::new(
            "INFERENCE_HOST_ACCEPT_REJECTED",
            "Host did not accept the routed prompt",
        ));
    }
    let optional_id = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    Ok(HostRoutedPromptAcceptance {
        duplicate: value.get("duplicate").and_then(Value::as_bool) == Some(true),
        echo_entry_id: optional_id("echoEntryId"),
        user_message_id: optional_id("userMessageId"),
        recent_user_messages: value
            .get("recentUserMessages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceStreamSupersede {
    None,
    DeferredUntilAccepted { stream_id: String },
    CancelNow { stream_id: String },
}

#[derive(Debug, Clone)]
struct ActiveInferenceStream {
    stream_id: String,
    accepted: bool,
    supersede_requested: bool,
}

#[derive(Debug, Default)]
pub struct ActiveInferenceStreamRegistry {
    by_agent: Mutex<HashMap<String, ActiveInferenceStream>>,
}

impl ActiveInferenceStreamRegistry {
    pub fn begin(&self, agent_id: &str, stream_id: &str) -> Result<(), Failure> {
        let agent_id = agent_id.trim();
        let stream_id = stream_id.trim();
        if agent_id.is_empty() || stream_id.is_empty() {
            return Err(Failure::new(
                "INFERENCE_ACTIVE_STREAM_INVALID",
                "active inference stream requires non-empty agent and stream ids",
            ));
        }
        let mut active = self.by_agent.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_ACTIVE_STREAM_LOCK_FAILED",
                "active inference stream registry lock poisoned",
            )
        })?;
        if let Some(existing) = active.get(agent_id) {
            return Err(Failure::new(
                "INFERENCE_ACTIVE_STREAM_CONFLICT",
                format!(
                    "agent {agent_id} already owns inference stream {}",
                    existing.stream_id
                ),
            ));
        }
        active.insert(
            agent_id.to_string(),
            ActiveInferenceStream {
                stream_id: stream_id.to_string(),
                accepted: false,
                supersede_requested: false,
            },
        );
        Ok(())
    }

    pub fn request_supersede(&self, agent_id: &str) -> InferenceStreamSupersede {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return InferenceStreamSupersede::None;
        }
        let Ok(mut active) = self.by_agent.lock() else {
            return InferenceStreamSupersede::None;
        };
        let Some(stream) = active.get_mut(agent_id) else {
            return InferenceStreamSupersede::None;
        };
        if stream.accepted {
            InferenceStreamSupersede::CancelNow {
                stream_id: stream.stream_id.clone(),
            }
        } else {
            stream.supersede_requested = true;
            InferenceStreamSupersede::DeferredUntilAccepted {
                stream_id: stream.stream_id.clone(),
            }
        }
    }

    pub fn mark_accepted(&self, agent_id: &str, stream_id: &str) -> Result<bool, Failure> {
        let mut active = self.by_agent.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_ACTIVE_STREAM_LOCK_FAILED",
                "active inference stream registry lock poisoned",
            )
        })?;
        let stream = active.get_mut(agent_id).ok_or_else(|| {
            Failure::new(
                "INFERENCE_ACTIVE_STREAM_MISSING",
                format!("agent {agent_id} has no active inference stream"),
            )
        })?;
        if stream.stream_id != stream_id {
            return Err(Failure::new(
                "INFERENCE_ACTIVE_STREAM_STALE",
                format!(
                    "agent {agent_id} active stream changed from {stream_id} to {}",
                    stream.stream_id
                ),
            ));
        }
        stream.accepted = true;
        Ok(stream.supersede_requested)
    }

    pub fn finish(&self, agent_id: &str, stream_id: &str) -> bool {
        let Ok(mut active) = self.by_agent.lock() else {
            return false;
        };
        let matches = active
            .get(agent_id)
            .is_some_and(|stream| stream.stream_id == stream_id);
        if matches {
            active.remove(agent_id);
        }
        matches
    }

    pub fn current_stream_id(&self, agent_id: &str) -> Option<String> {
        self.by_agent
            .lock()
            .ok()
            .and_then(|active| active.get(agent_id).map(|stream| stream.stream_id.clone()))
    }
}

pub fn should_append_user_message(send_args: &Value) -> bool {
    send_args
        .get("appendUserMessage")
        .and_then(Value::as_bool)
        != Some(false)
}

pub fn is_direct_user_send(send_args: &Value) -> bool {
    let automation = send_args.get("automationWake");
    let group = send_args.get("groupContext");
    let agent_wake = send_args.get("agentWake");
    automation.is_none_or(Value::is_null)
        && group.is_none_or(Value::is_null)
        && agent_wake.is_none_or(Value::is_null)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoordinatorAgentWakeRoute {
    pub agent_id: String,
    pub send_args: Value,
}

pub const PRIORITY_AGENT_WAKE_SUPERSEDE_REASON: &str =
    "superseded by a priority agent message";

pub fn agent_inbound_failure_gateway_args(
    agent_id: &str,
    error_code: &str,
    message: &str,
) -> Value {
    serde_json::json!({
        "agentId": agent_id,
        "errorCode": error_code,
        "message": message,
    })
}

pub fn redrive_agent_inbound_after_priority_preemption(
    send_args: &Value,
    cancellation_message: &str,
) -> Option<Value> {
    if cancellation_message != PRIORITY_AGENT_WAKE_SUPERSEDE_REASON
        || send_args.get("requestSource").and_then(Value::as_str) != Some("agent-inbound")
    {
        return None;
    }
    let wake = send_args.get("agentWake")?.as_object()?;
    if wake.get("isRedriven").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let mut redrive = send_args.clone();
    redrive
        .get_mut("agentWake")?
        .as_object_mut()?
        .insert("isRedriven".into(), Value::Bool(true));
    Some(redrive)
}

pub fn agent_inbound_wake_prefers_urgent_queue(send_args: &Value) -> bool {
    if send_args.get("requestSource").and_then(Value::as_str) != Some("agent-inbound") {
        return false;
    }
    let Some(wake) = send_args.get("agentWake").and_then(Value::as_object) else {
        return false;
    };
    wake.get("priority").and_then(Value::as_bool) == Some(true)
        || wake.get("isRedriven").and_then(Value::as_bool) == Some(true)
}

pub fn prepare_agent_inbound_wake_routes(
    payload: &Value,
) -> Result<Vec<CoordinatorAgentWakeRoute>, Failure> {
    let payload = payload.as_object().ok_or_else(|| {
        Failure::new(
            "INFERENCE_AGENT_WAKE_INVALID",
            "agent inbound wake payload must be an object",
        )
    })?;
    let required = |field: &str| {
        payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                Failure::new(
                    "INFERENCE_AGENT_WAKE_INVALID",
                    format!("agent inbound wake is missing {field}"),
                )
            })
    };
    let target_agent_id = required("agentId")?;
    let source_agent_id = required("sourceAgentId")?;
    let prompt = required("prompt")?;
    let priority = payload
        .get("priority")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let selected_images = match payload.get("selectedImages") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values.clone(),
        Some(_) => {
            return Err(Failure::new(
                "INFERENCE_AGENT_WAKE_INVALID",
                "agent inbound wake selectedImages must be an array",
            ));
        }
    };
    let mut send_args = serde_json::json!({
        "agentId": target_agent_id.clone(),
        "prompt": prompt.clone(),
        "_runnerPrompt": prompt,
        "appendUserMessage": false,
        "hidden": true,
        "isSilenceAllowed": true,
        "requestSource": "agent-inbound",
        "skipAckObligation": true,
        "agentWake": {
            "sourceAgentId": source_agent_id,
            "priority": priority,
        },
    });
    if !selected_images.is_empty() {
        send_args["selectedImages"] = Value::Array(selected_images);
    }
    if let Some(inbound) = payload.get("inbound").filter(|value| !value.is_null()) {
        send_args["agentWake"]["inbound"] = inbound.clone();
        if let Some(reply_context) = inbound.get("replyContext") {
            let context = reply_context.as_object().ok_or_else(|| {
                Failure::new(
                    "INFERENCE_AGENT_WAKE_INVALID",
                    "agent inbound replyContext must be an object",
                )
            })?;
            for field in ["targetId", "quote"] {
                if context
                    .get(field)
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty())
                {
                    return Err(Failure::new(
                        "INFERENCE_AGENT_WAKE_INVALID",
                        format!("agent inbound replyContext requires non-empty {field}"),
                    ));
                }
            }
            send_args["replyContext"] = reply_context.clone();
        }
    }
    Ok(vec![CoordinatorAgentWakeRoute {
        agent_id: target_agent_id,
        send_args,
    }])

}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceProvider {
    Fabushi,
    Cursor,
    Codex,
    ClaudeCode,
    OpenRouter,
}

impl InferenceProvider {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fabushi" => Some(Self::Fabushi),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "claude-code" => Some(Self::ClaudeCode),
            "openrouter" => Some(Self::OpenRouter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabushi => "fabushi",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::OpenRouter => "openrouter",
        }
    }
}

pub fn configured_inference_provider(settings_path: &Path) -> Option<InferenceProvider> {
    let value: Value =
        serde_json::from_str(&fs::read_to_string(settings_path).ok()?).ok()?;
    [
        value.get("inferenceProvider"),
        value.get("inference_provider"),
        value.get("router").and_then(|router| router.get("provider")),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_str().and_then(InferenceProvider::parse))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerInferenceEvent {
    Delta { content: String },
    Completed { content: String },
    Failed { message: String },
    Cancelled { message: String },
}

pub fn parse_runner_inference_event(
    value: &Value,
) -> Result<(String, RunnerInferenceEvent), Failure> {
    let stream_id = value
        .get("streamId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Failure::new(
            "INFERENCE_RUNNER_EVENT_INVALID",
            "Runner inference event is missing streamId",
        ))?
        .to_string();
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let event = match event_type {
        "delta" => RunnerInferenceEvent::Delta {
            content: value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "completed" => RunnerInferenceEvent::Completed {
            content: value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        "failed" => RunnerInferenceEvent::Failed {
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Runner inference failed")
                .to_string(),
        },
        "cancelled" => RunnerInferenceEvent::Cancelled {
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Runner inference cancelled")
                .to_string(),
        },
        other => {
            return Err(Failure::new(
                "INFERENCE_RUNNER_EVENT_INVALID",
                format!("unknown Runner inference event type: {other}"),
            ));
        }
    };
    Ok((stream_id, event))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceRoute {
    pub provider: String,
    pub host_slot: String,
}

type InferenceTask = Box<dyn FnOnce() + Send + 'static>;

struct QueuedInferenceTask {
    generation: u64,
    generation_gate: Arc<AtomicU64>,
    task: InferenceTask,
}

#[derive(Clone)]
struct InferenceTaskWorker {
    urgent: Sender<QueuedInferenceTask>,
    normal: Sender<QueuedInferenceTask>,
    generation: Arc<AtomicU64>,
}

fn run_inference_task(task: QueuedInferenceTask) {
    if task.generation_gate.load(Ordering::SeqCst) != task.generation {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(task.task));
}

#[derive(Default)]
pub struct InferenceTaskQueue {
    workers: Mutex<HashMap<String, InferenceTaskWorker>>,
}

impl InferenceTaskQueue {
    fn spawn_worker(agent_id: &str) -> Result<InferenceTaskWorker, Failure> {
        let (urgent_tx, urgent_rx) = mpsc::channel::<QueuedInferenceTask>();
        let (normal_tx, normal_rx) = mpsc::channel::<QueuedInferenceTask>();
        let generation = Arc::new(AtomicU64::new(0));
        thread::Builder::new()
            .name(format!("inference-router-{agent_id}"))
            .spawn(move || loop {
                while let Ok(task) = urgent_rx.try_recv() {
                    run_inference_task(task);
                }
                match normal_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(normal_task) => {
                        while let Ok(task) = urgent_rx.try_recv() {
                            run_inference_task(task);
                        }
                        run_inference_task(normal_task);
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => {
                        while let Ok(task) = urgent_rx.try_recv() {
                            run_inference_task(task);
                        }
                        if matches!(urgent_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                            break;
                        }
                    }
                }
            })
            .map_err(|error| {
                Failure::new(
                    "INFERENCE_QUEUE_SPAWN_FAILED",
                    format!("could not start inference queue worker: {error}"),
                )
            })?;
        Ok(InferenceTaskWorker {
            urgent: urgent_tx,
            normal: normal_tx,
            generation,
        })
    }

    fn queued_task(worker: &InferenceTaskWorker, task: InferenceTask) -> QueuedInferenceTask {
        QueuedInferenceTask {
            generation: worker.generation.load(Ordering::SeqCst),
            generation_gate: Arc::clone(&worker.generation),
            task,
        }
    }

    pub fn enqueue<F>(&self, agent_id: &str, task: F) -> Result<(), Failure>
    where
        F: FnOnce() + Send + 'static,
    {
        self.enqueue_lane(agent_id, false, Box::new(task))
    }

    pub fn enqueue_urgent<F>(&self, agent_id: &str, task: F) -> Result<(), Failure>
    where
        F: FnOnce() + Send + 'static,
    {
        self.enqueue_lane(agent_id, true, Box::new(task))
    }

    fn enqueue_lane(
        &self,
        agent_id: &str,
        urgent: bool,
        mut task: InferenceTask,
    ) -> Result<(), Failure> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(Failure::new(
                "INFERENCE_QUEUE_AGENT_REQUIRED",
                "local inference routing requires a non-empty agentId",
            ));
        }
        let mut workers = self.workers.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_QUEUE_LOCK_FAILED",
                "inference queue lock poisoned",
            )
        })?;

        if let Some(worker) = workers.get(agent_id) {
            let sender = if urgent { &worker.urgent } else { &worker.normal };
            match sender.send(Self::queued_task(worker, task)) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    task = error.0.task;
                    worker.generation.fetch_add(1, Ordering::SeqCst);
                    workers.remove(agent_id);
                }
            }
        }

        let worker = Self::spawn_worker(agent_id)?;
        let sender = if urgent { &worker.urgent } else { &worker.normal };
        sender
            .send(Self::queued_task(&worker, task))
            .map_err(|error| {
                Failure::new(
                    "INFERENCE_QUEUE_DISCONNECTED",
                    format!("inference queue worker stopped before enqueue: {error}"),
                )
            })?;
        workers.insert(agent_id.to_string(), worker);
        Ok(())
    }

    pub fn clear_agent(&self, agent_id: &str) -> usize {
        let Ok(mut workers) = self.workers.lock() else {
            return 0;
        };
        let Some(worker) = workers.remove(agent_id.trim()) else {
            return 0;
        };
        worker.generation.fetch_add(1, Ordering::SeqCst);
        1
    }

    pub fn worker_count(&self) -> usize {
        self.workers
            .lock()
            .map(|workers| workers.len())
            .unwrap_or_default()
    }

    pub fn dispose(&self) {
        if let Ok(mut workers) = self.workers.lock() {
            for worker in workers.values() {
                worker.generation.fetch_add(1, Ordering::SeqCst);
            }
            workers.clear();
        }
    }
}

#[derive(Debug, Default)]
pub struct InferenceRouter {
    by_agent: HashMap<String, InferenceRoute>,
    default: Option<InferenceRoute>,
}

impl InferenceRouter {
    pub fn set_default(&mut self, route: InferenceRoute) {
        self.default = Some(route);
    }

    pub fn clear_default(&mut self) {
        self.default = None;
    }

    pub fn bind_agent(&mut self, agent_id: impl Into<String>, route: InferenceRoute) {
        self.by_agent.insert(agent_id.into(), route);
    }

    pub fn unbind_agent(&mut self, agent_id: &str) {
        self.by_agent.remove(agent_id);
    }

    pub fn resolve(&self, agent_id: &str) -> Option<&InferenceRoute> {
        self.by_agent.get(agent_id).or(self.default.as_ref())
    }
}

#[derive(Debug)]
pub struct CoordinatorInferenceRouter {
    settings_path: PathBuf,
    fallback_provider: InferenceProvider,
    routes: Mutex<InferenceRouter>,
}

impl CoordinatorInferenceRouter {
    pub fn new(settings_path: impl Into<PathBuf>) -> Self {
        Self::new_with_fallback(settings_path, InferenceProvider::Cursor)
    }

    pub fn new_with_fallback(
        settings_path: impl Into<PathBuf>,
        fallback_provider: InferenceProvider,
    ) -> Self {
        Self {
            settings_path: settings_path.into(),
            fallback_provider,
            routes: Mutex::new(InferenceRouter::default()),
        }
    }

    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn resolve(&self, agent_id: &str) -> InferenceRoute {
        let configured = configured_inference_provider(&self.settings_path)
            .unwrap_or(self.fallback_provider);
        let fallback = InferenceRoute {
            provider: configured.as_str().to_string(),
            host_slot: "host".into(),
        };
        let Ok(mut routes) = self.routes.lock() else {
            return fallback;
        };
        routes.set_default(fallback.clone());
        routes.resolve(agent_id).cloned().unwrap_or(fallback)
    }

    pub fn bind_agent(
        &self,
        agent_id: impl Into<String>,
        route: InferenceRoute,
    ) -> Result<(), Failure> {
        let mut routes = self.routes.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_ROUTER_LOCK_FAILED",
                "Coordinator inference router lock poisoned",
            )
        })?;
        routes.bind_agent(agent_id, route);
        Ok(())
    }

    pub fn unbind_agent(&self, agent_id: &str) -> Result<(), Failure> {
        let mut routes = self.routes.lock().map_err(|_| {
            Failure::new(
                "INFERENCE_ROUTER_LOCK_FAILED",
                "Coordinator inference router lock poisoned",
            )
        })?;
        routes.unbind_agent(agent_id);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoredRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredReaction {
    pub emoji: String,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAttachment {
    pub path: String,
    pub name: String,
}

pub fn parse_send_prompt_attachments(args: &Value) -> Result<Vec<StoredAttachment>, Failure> {
    let paths = match args.get("attachmentPaths") {
        None => &[][..],
        Some(Value::Array(paths)) => paths.as_slice(),
        Some(_) => {
            return Err(Failure::new(
                "INFERENCE_ROUTER_INVALID_ATTACHMENTS",
                "attachmentPaths must be an array",
            ));
        }
    };
    let names = match args.get("attachmentNames") {
        None => &[][..],
        Some(Value::Array(names)) => names.as_slice(),
        Some(_) => {
            return Err(Failure::new(
                "INFERENCE_ROUTER_INVALID_ATTACHMENTS",
                "attachmentNames must be an array",
            ));
        }
    };
    if paths.is_empty() && names.is_empty() {
        return Ok(Vec::new());
    }
    if paths.len() != names.len() {
        return Err(Failure::new(
            "INFERENCE_ROUTER_INVALID_ATTACHMENTS",
            "attachmentPaths and attachmentNames must have the same length",
        ));
    }
    paths
        .iter()
        .zip(names)
        .map(|(path, name)| {
            let path = path.as_str().unwrap_or("").trim();
            let name = name.as_str().unwrap_or("").trim();
            if path.is_empty() || name.is_empty() {
                return Err(Failure::new(
                    "INFERENCE_ROUTER_INVALID_ATTACHMENTS",
                    "attachment paths and names must be non-empty strings",
                ));
            }
            Ok(StoredAttachment {
                path: path.to_string(),
                name: name.to_string(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredEntry {
    pub provider: String,
    pub role: StoredRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rich_text: Option<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<StoredAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<StoredReaction>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptStore {
    pub schema_version: u32,
    pub agents: HashMap<String, Vec<StoredEntry>>,
}

impl Default for TranscriptStore {
    fn default() -> Self {
        Self {
            schema_version: INFERENCE_TRANSCRIPT_SCHEMA_VERSION,
            agents: HashMap::new(),
        }
    }
}

impl TranscriptStore {
    pub fn parse(value: Value) -> Self {
        let Some(root) = value.as_object() else {
            return Self::default();
        };
        if root.get("schemaVersion").and_then(Value::as_u64)
            != Some(INFERENCE_TRANSCRIPT_SCHEMA_VERSION as u64)
        {
            return Self::default();
        }
        let Some(agents) = root.get("agents").and_then(Value::as_object) else {
            return Self::default();
        };

        let mut parsed = HashMap::new();
        for (agent_id, raw_entries) in agents {
            let Some(rows) = raw_entries.as_array() else {
                continue;
            };
            let mut entries = rows
                .iter()
                .filter_map(|row| serde_json::from_value::<StoredEntry>(row.clone()).ok())
                .filter(valid_entry)
                .collect::<Vec<_>>();
            if entries.len() > INFERENCE_TRANSCRIPT_LIMIT {
                entries.drain(..entries.len() - INFERENCE_TRANSCRIPT_LIMIT);
            }
            parsed.insert(agent_id.clone(), entries);
        }
        Self {
            schema_version: INFERENCE_TRANSCRIPT_SCHEMA_VERSION,
            agents: parsed,
        }
    }

    pub fn append(&mut self, agent_id: &str, new_entries: impl IntoIterator<Item = StoredEntry>) {
        let entries = self.agents.entry(agent_id.to_string()).or_default();
        entries.extend(new_entries.into_iter().filter(valid_entry));
        if entries.len() > INFERENCE_TRANSCRIPT_LIMIT {
            entries.drain(..entries.len() - INFERENCE_TRANSCRIPT_LIMIT);
        }
    }

    pub fn entries(&self, agent_id: &str) -> &[StoredEntry] {
        self.agents.get(agent_id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn toggle_local_reaction(
        &mut self,
        agent_id: &str,
        entry_id: &str,
        emoji: &str,
    ) -> Option<&StoredEntry> {
        let emoji = emoji.trim();
        if emoji.is_empty() {
            return None;
        }
        let entry = self
            .agents
            .get_mut(agent_id)?
            .iter_mut()
            .find(|entry| entry.id == entry_id)?;
        if let Some(index) = entry
            .reactions
            .iter()
            .position(|reaction| reaction.emoji == emoji && reaction.by == "me")
        {
            entry.reactions.remove(index);
        } else {
            entry.reactions.push(StoredReaction {
                emoji: emoji.to_string(),
                by: "me".into(),
            });
        }
        Some(entry)
    }

    pub fn next_turn_number(
        &self,
        agent_id: &str,
        remote_entry_ids: impl IntoIterator<Item = String>,
    ) -> u64 {
        let remote_entry_ids = remote_entry_ids.into_iter().collect::<Vec<_>>();
        self.entries(agent_id)
            .iter()
            .map(|entry| entry.id.as_str())
            .chain(remote_entry_ids.iter().map(String::as_str))
            .filter_map(turn_number_from_id)
            .max()
            .map_or(0, |turn| turn.saturating_add(1))
    }
}

fn valid_entry(entry: &StoredEntry) -> bool {
    matches!(
        entry.provider.as_str(),
        "codex" | "claude-code" | "openrouter" | "fabushi"
    ) && !entry.id.trim().is_empty()
}

pub fn project_transcript_entry(entry: &StoredEntry) -> Value {
    match entry.role {
        StoredRole::User => {
            let mut value = json!({
                "kind": "message",
                "id": entry.id,
                "role": "user",
                "content": entry.content,
                "isStreaming": false,
                "timestampMs": entry.timestamp_ms,
            });
            if let Some(rich_text) = &entry.rich_text {
                value["richText"] = Value::String(rich_text.clone());
            }
            if let Some(client_nonce) = &entry.client_nonce {
                value["clientNonce"] = Value::String(client_nonce.clone());
            }
            if !entry.attachments.is_empty() {
                value["attachments"] =
                    serde_json::to_value(&entry.attachments).unwrap_or(Value::Null);
            }
            if !entry.reactions.is_empty() {
                value["reactions"] = serde_json::to_value(&entry.reactions).unwrap_or(Value::Null);
            }
            value
        }
        StoredRole::Assistant => {
            let mut value = json!({
                "kind": "send-message",
                "id": entry.id,
                "message": {
                    "type": "text",
                    "content": entry.content,
                },
                "timestampMs": entry.timestamp_ms,
            });
            if !entry.reactions.is_empty() {
                value["reactions"] = serde_json::to_value(&entry.reactions).unwrap_or(Value::Null);
            }
            value
        }
    }
}


/// Build provider conversation history from the Host-owned canonical transcript.
///
/// User turns are ordinary transcript messages. User-visible Agent output is
/// durably represented by `send-message` entries, not by the Coordinator's
/// provider-completion shadow. Projecting those sends as assistant history keeps
/// later turns from treating already-answered user messages as pending work.
pub fn provider_messages_from_host_transcript(transcript: &Value) -> Vec<Value> {
    transcript
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            match entry.get("kind").and_then(Value::as_str) {
                Some("message")
                    if entry.get("role").and_then(Value::as_str) == Some("user") =>
                {
                    Some(json!({
                        "role": "user",
                        "content": entry
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    }))
                }
                Some("send-message")
                    if entry
                        .get("message")
                        .and_then(Value::as_object)
                        .and_then(|message| message.get("type"))
                        .and_then(Value::as_str)
                        == Some("text") =>
                {
                    Some(json!({
                        "role": "assistant",
                        "content": entry
                            .get("message")
                            .and_then(Value::as_object)
                            .and_then(|message| message.get("content"))
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    }))
                }
                _ => None,
            }
        })
        .collect()
}

pub fn turn_number_from_id(id: &str) -> Option<u64> {
    let rest = id.strip_prefix('t')?;
    let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let (digits, suffix) = rest.split_at(digit_count);
    let valid_suffix = suffix == "u"
        || suffix
            .strip_prefix('s')
            .is_some_and(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()));
    valid_suffix.then(|| digits.parse::<u64>().ok()).flatten()
}

#[derive(Debug, Clone)]
pub struct InferenceTranscriptFile {
    path: PathBuf,
}

impl InferenceTranscriptFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> TranscriptStore {
        let value = fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        value.map(TranscriptStore::parse).unwrap_or_default()
    }

    pub fn persist(&self, store: &TranscriptStore) -> Result<(), Failure> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                Failure::new(
                    "INFERENCE_STORE_WRITE_FAILED",
                    format!("could not create transcript directory: {error}"),
                )
            })?;
        }
        let temporary = self.path.with_extension(format!(
            "{}.tmp",
            Uuid::new_v4()
        ));
        let bytes = serde_json::to_vec_pretty(store).map_err(|error| {
            Failure::new(
                "INFERENCE_STORE_SERIALIZE_FAILED",
                format!("could not serialize transcript store: {error}"),
            )
        })?;
        fs::write(&temporary, bytes).map_err(|error| {
            Failure::new(
                "INFERENCE_STORE_WRITE_FAILED",
                format!("could not write temporary transcript: {error}"),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(
                |error| {
                    Failure::new(
                        "INFERENCE_STORE_WRITE_FAILED",
                        format!("could not secure temporary transcript: {error}"),
                    )
                },
            )?;
        }
        fs::rename(&temporary, &self.path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            Failure::new(
                "INFERENCE_STORE_RENAME_FAILED",
                format!("could not atomically replace transcript store: {error}"),
            )
        })
    }

    pub fn append(
        &self,
        agent_id: &str,
        entries: impl IntoIterator<Item = StoredEntry>,
    ) -> Result<TranscriptStore, Failure> {
        let mut store = self.load();
        store.append(agent_id, entries);
        self.persist(&store)?;
        Ok(store)
    }

    pub fn toggle_local_reaction(
        &self,
        agent_id: &str,
        entry_id: &str,
        emoji: &str,
    ) -> Result<Option<StoredEntry>, Failure> {
        let mut store = self.load();
        let updated = store
            .toggle_local_reaction(agent_id, entry_id, emoji)
            .cloned();
        if updated.is_some() {
            self.persist(&store)?;
        }
        Ok(updated)
    }
}
