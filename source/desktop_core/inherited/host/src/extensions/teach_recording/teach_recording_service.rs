use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{SecondsFormat, TimeZone, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::r#box::box_shell_command::{HostShellArgsInput, build_host_shell_args};
use crate::r#box::generated_production::ProductionShellResult;
use crate::extensions::forever_box::ForeverBoxService;

use super::extension::{
    TeachRecordingService, TeachRecordingServiceDeps, TeachRecordingServiceFactory,
};

pub const TEACH_SESSIONS_DIR: &str = "/workspace/teach-sessions";
pub const TEACH_QUEUES_DIR: &str = "/workspace/teach-sessions/queues";
pub const LEARN_SKILL_NAME: &str = "learn-from-demonstration";
pub const TEACH_PRIVATE_MONITOR_MESSAGE: &str =
    "Teach recording requires a private desktop monitor.";
pub const SAND_TEACH_MAX_DURATION_MS: u64 = 10 * 60_000;
pub const SAND_BOX_FIRST_FORK_WINDOW_INDEX: u32 = 2;
pub const SAND_MONITOR_WIDTH: u32 = 1280;
pub const SAND_MONITOR_HEIGHT: u32 = 800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub agent_id: String,
    pub started_at_ms: u64,
    pub session_dir: String,
    pub display_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRecording {
    pub agent_id: String,
    pub queue_file: String,
    pub client_nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeachStatus {
    pub state: String,
    pub agent_id: Option<String>,
    pub started_at_ms: Option<u64>,
    pub max_duration_ms: u64,
}

impl TeachStatus {
    pub fn idle() -> Self {
        Self {
            state: "idle".into(),
            agent_id: None,
            started_at_ms: None,
            max_duration_ms: SAND_TEACH_MAX_DURATION_MS,
        }
    }

    fn recording(recording: &Recording) -> Self {
        Self {
            state: "recording".into(),
            agent_id: Some(recording.agent_id.clone()),
            started_at_ms: Some(recording.started_at_ms),
            max_duration_ms: SAND_TEACH_MAX_DURATION_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ShellResult {
    pub fn success() -> Self {
        Self {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

pub trait TeachRecordingRuntimePort: Send + Sync {
    fn ensure_ready(&self, agent_id: &str) -> Result<(), String>;
    fn get_agent_window_index(&self, agent_id: &str) -> Option<u32>;
    fn run_shell(
        &self,
        agent_id: &str,
        command: &str,
        tool_call_id: &str,
    ) -> Result<ShellResult, String>;
}

#[derive(Clone)]
pub struct ProductionTeachRecordingRuntime {
    forever_box: Arc<ForeverBoxService>,
}

impl ProductionTeachRecordingRuntime {
    pub fn new(forever_box: Arc<ForeverBoxService>) -> Self {
        Self { forever_box }
    }
}

impl TeachRecordingRuntimePort for ProductionTeachRecordingRuntime {
    fn ensure_ready(&self, agent_id: &str) -> Result<(), String> {
        self.forever_box
            .ensure(agent_id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn get_agent_window_index(&self, agent_id: &str) -> Option<u32> {
        self.forever_box.box_().get_agent_window_index(agent_id)
    }

    fn run_shell(
        &self,
        agent_id: &str,
        command: &str,
        tool_call_id: &str,
    ) -> Result<ShellResult, String> {
        let mut ready = self
            .forever_box
            .box_()
            .ensure_ready(agent_id)
            .map_err(|error| error.to_string())?;
        let args = build_host_shell_args(HostShellArgsInput {
            command: command.to_string(),
            name: "teach-recording".into(),
            working_directory: "/workspace".into(),
            tool_call_id: tool_call_id.to_string(),
        });
        let result = ready
            .remote_accessor
            .execute_shell_result(&(), args)
            .map_err(|error| error.to_string())?;
        match result {
            ProductionShellResult::Success {
                exit_code,
                stdout,
                stderr,
            }
            | ProductionShellResult::Failure {
                exit_code,
                stdout,
                stderr,
                ..
            } => Ok(ShellResult {
                exit_code,
                stdout,
                stderr,
            }),
            ProductionShellResult::SpawnError { error } => {
                Err(format!("shell spawn failed: {error}"))
            }
            ProductionShellResult::PermissionDenied { error } => {
                Err(format!("shell permission denied: {error}"))
            }
            ProductionShellResult::Rejected { reason } => {
                Err(format!("shell request rejected: {reason}"))
            }
            ProductionShellResult::Timeout { timeout_ms } => {
                Err(format!("shell request timed out after {timeout_ms}ms"))
            }
            ProductionShellResult::Other { case } => {
                Err(format!("shell request returned unsupported result: {case}"))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandTeachRecordingError {
    pub kind: String,
    pub message: String,
}

impl SandTeachRecordingError {
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalizationPhase {
    Preparing,
    WindingDown,
    CheckingWorkflow,
    DispatchingPrompt,
}

struct ServiceState {
    active: Option<Recording>,
    failed_start: Option<Recording>,
    finalizing: Option<FinalizationPhase>,
    queue_persisted: bool,
    disposed: bool,
    listeners: HashMap<u64, Arc<dyn Fn(TeachStatus) + Send + Sync>>,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self {
            active: None,
            failed_start: None,
            finalizing: None,
            queue_persisted: false,
            disposed: false,
            listeners: HashMap::new(),
        }
    }
}

struct Inner {
    deps: TeachRecordingServiceDeps,
    runtime: Arc<dyn TeachRecordingRuntimePort>,
    state: Mutex<ServiceState>,
    operation: Mutex<()>,
    next_listener_id: AtomicU64,
    cap_generation: AtomicU64,
    discard_requested: AtomicBool,
}

#[derive(Clone)]
pub struct SandTeachRecordingService {
    inner: Arc<Inner>,
}

#[derive(Clone)]
pub struct TeachRecordingApi {
    service: SandTeachRecordingService,
}

impl TeachRecordingApi {
    pub fn start(
        &self,
        agent_id: &str,
        entry_point: Option<&str>,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        self.service.start(agent_id, entry_point)
    }

    pub fn stop(
        &self,
        agent_id: &str,
        save: bool,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        self.service.stop(agent_id, save)
    }

    pub fn get_status(&self) -> TeachStatus {
        self.service.status()
    }

    pub fn subscribe(
        &self,
        listener: Arc<dyn Fn(TeachStatus) + Send + Sync>,
    ) -> TeachRecordingSubscription {
        self.service.subscribe(listener)
    }
}

pub struct TeachRecordingSubscription {
    inner: Weak<Inner>,
    listener_id: u64,
}

impl Drop for TeachRecordingSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .state
                .lock()
                .expect("teach recording state poisoned")
                .listeners
                .remove(&self.listener_id);
        }
    }
}

pub struct SandTeachRecordingServiceFactory {
    runtime: Arc<dyn TeachRecordingRuntimePort>,
}

impl SandTeachRecordingServiceFactory {
    pub fn new(runtime: Arc<dyn TeachRecordingRuntimePort>) -> Self {
        Self { runtime }
    }
}

impl TeachRecordingServiceFactory for SandTeachRecordingServiceFactory {
    type Service = SandTeachRecordingService;

    fn create(&self, deps: TeachRecordingServiceDeps) -> Self::Service {
        SandTeachRecordingService::new(deps, Arc::clone(&self.runtime))
    }
}

impl TeachRecordingService for SandTeachRecordingService {
    type Api = TeachRecordingApi;

    fn recover_pending(&self) -> Result<(), String> {
        SandTeachRecordingService::recover_pending(self).map_err(|error| error.to_string())
    }

    fn api(&self) -> Self::Api {
        TeachRecordingApi {
            service: self.clone(),
        }
    }

    fn dispose(&self) {
        SandTeachRecordingService::dispose(self);
    }
}

impl SandTeachRecordingService {
    pub fn new(
        deps: TeachRecordingServiceDeps,
        runtime: Arc<dyn TeachRecordingRuntimePort>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                deps,
                runtime,
                state: Mutex::new(ServiceState::default()),
                operation: Mutex::new(()),
                next_listener_id: AtomicU64::new(1),
                cap_generation: AtomicU64::new(0),
                discard_requested: AtomicBool::new(false),
            }),
        }
    }

    pub fn status(&self) -> TeachStatus {
        let state = self.inner.state.lock().expect("teach recording state poisoned");
        status_of(&state)
    }

    pub fn subscribe(
        &self,
        listener: Arc<dyn Fn(TeachStatus) + Send + Sync>,
    ) -> TeachRecordingSubscription {
        let listener_id = self
            .inner
            .next_listener_id
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .state
            .lock()
            .expect("teach recording state poisoned")
            .listeners
            .insert(listener_id, listener);
        TeachRecordingSubscription {
            inner: Arc::downgrade(&self.inner),
            listener_id,
        }
    }

    pub fn start(
        &self,
        agent_id: &str,
        entry_point: Option<&str>,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        let _operation = self
            .inner
            .operation
            .lock()
            .expect("teach recording operation poisoned");

        {
            let state = self.inner.state.lock().expect("teach recording state poisoned");
            if state.disposed {
                let error = SandTeachRecordingError::new(
                    "shutting_down",
                    "teach-recording: recording service is shutting down",
                );
                drop(state);
                self.report_start_failure(agent_id, entry_point, &error);
                return Err(error);
            }
            if let Some(active) = &state.active {
                return Ok(TeachStatus::recording(active));
            }
        }

        if !(self.inner.deps.is_enabled)() {
            let error = SandTeachRecordingError::new(
                "disabled",
                "teach-recording: the feature gate is off",
            );
            self.report_start_failure(agent_id, entry_point, &error);
            return Err(error);
        }

        if let Err(error) = self.recover_failed_start() {
            self.report_start_failure(agent_id, entry_point, &error);
            return Err(error);
        }

        if let Err(error) = self.inner.runtime.ensure_ready(agent_id) {
            let error = SandTeachRecordingError::new(
                "box",
                format!("teach-recording: box is unavailable: {error}"),
            );
            self.report_start_failure(agent_id, entry_point, &error);
            return Err(error);
        }
        let window_index = self.inner.runtime.get_agent_window_index(agent_id);
        if !is_fork_window_index(window_index) {
            let error =
                SandTeachRecordingError::new("no_monitor", TEACH_PRIVATE_MONITOR_MESSAGE);
            self.report_start_failure(agent_id, entry_point, &error);
            return Err(error);
        }
        let window_index = window_index.expect("fork window checked above");
        let now_ms = now_ms();
        let stamp = Utc
            .timestamp_millis_opt(now_ms as i64)
            .single()
            .unwrap_or_else(Utc::now)
            .format("%Y%m%dT%H%M%SZ")
            .to_string();
        let recording = Recording {
            agent_id: agent_id.to_string(),
            started_at_ms: now_ms,
            session_dir: format!(
                "{TEACH_SESSIONS_DIR}/teach-{stamp}-{}",
                Uuid::new_v4()
            ),
            display_label: format!(":{window_index}"),
        };

        {
            let mut state = self.inner.state.lock().expect("teach recording state poisoned");
            state.failed_start = Some(recording.clone());
        }
        let command = start_recording_command(&recording, window_index);
        let start_result = self.run_shell(agent_id, &command, "sand-teach-recording-start");
        match start_result {
            Ok(result) if result.exit_code == 0 => {}
            Ok(result) => {
                let error = SandTeachRecordingError::new(
                    "start_failed",
                    format!(
                        "teach-recording: ffmpeg failed to start: {} {}",
                        result.stdout, result.stderr
                    ),
                );
                if let Err(cleanup) = self.cleanup_failed_start(&recording) {
                    let cleanup = SandTeachRecordingError::new(
                        "cleanup_failed",
                        format!(
                            "teach-recording: ffmpeg startup cleanup failed: {cleanup}"
                        ),
                    );
                    self.report_start_failure(agent_id, entry_point, &cleanup);
                    return Err(cleanup);
                }
                self.report_start_failure(agent_id, entry_point, &error);
                return Err(error);
            }
            Err(error) => {
                let startup = SandTeachRecordingError::new(
                    "shell",
                    format!("teach-recording: ffmpeg start command failed: {error}"),
                );
                if let Err(cleanup) = self.cleanup_failed_start(&recording) {
                    let cleanup = SandTeachRecordingError::new(
                        "cleanup_failed",
                        format!(
                            "teach-recording: ffmpeg startup cleanup failed: {cleanup}"
                        ),
                    );
                    self.report_start_failure(agent_id, entry_point, &cleanup);
                    return Err(cleanup);
                }
                self.report_start_failure(agent_id, entry_point, &startup);
                return Err(startup);
            }
        }

        {
            let mut state = self.inner.state.lock().expect("teach recording state poisoned");
            state.failed_start = None;
            state.active = Some(recording.clone());
            state.queue_persisted = false;
            state.finalizing = None;
        }
        self.inner.discard_requested.store(false, Ordering::Release);
        (self.inner.deps.track_recording_started)(json!({
            "agent_id": agent_id,
            "entry_point": entry_point,
        }));
        self.emit();
        self.arm_cap(&recording);
        Ok(self.status())
    }

    pub fn stop(
        &self,
        agent_id: &str,
        save: bool,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        self.stop_internal(agent_id, save, true)
    }

    fn stop_internal(
        &self,
        agent_id: &str,
        save: bool,
        track_completion: bool,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        if !save {
            let state = self.inner.state.lock().expect("teach recording state poisoned");
            if state.finalizing != Some(FinalizationPhase::DispatchingPrompt) {
                self.inner.discard_requested.store(true, Ordering::Release);
            }
        }

        let _operation = self
            .inner
            .operation
            .lock()
            .expect("teach recording operation poisoned");

        let recording = {
            let state = self.inner.state.lock().expect("teach recording state poisoned");
            let Some(recording) = state.active.clone() else {
                return Ok(status_of(&state));
            };
            if recording.agent_id != agent_id {
                return Err(SandTeachRecordingError::new(
                    "agent_mismatch",
                    "teach-recording: recording belongs to a different agent",
                ));
            }
            recording
        };

        if save {
            self.inner.discard_requested.store(false, Ordering::Release);
        }
        self.set_phase(FinalizationPhase::Preparing);
        self.invalidate_cap();
        let ended_at_ms = now_ms();

        let outcome = self.finalize_recording(
            &recording,
            ended_at_ms,
            save,
            track_completion,
        );

        {
            let mut state = self.inner.state.lock().expect("teach recording state poisoned");
            state.finalizing = None;
        }

        match outcome {
            Ok(status) => Ok(status),
            Err(error) => {
                let should_rearm = {
                    let state = self.inner.state.lock().expect("teach recording state poisoned");
                    state.active.as_ref() == Some(&recording) && !state.disposed
                };
                if should_rearm {
                    self.arm_cap(&recording);
                }
                Err(error)
            }
        }
    }

    fn finalize_recording(
        &self,
        recording: &Recording,
        ended_at_ms: u64,
        save: bool,
        track_completion: bool,
    ) -> Result<TeachStatus, SandTeachRecordingError> {
        self.inner
            .runtime
            .ensure_ready(&recording.agent_id)
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "box",
                    format!("teach-recording: box is unavailable: {error}"),
                )
            })?;

        let should_discard = || {
            !save || self.inner.discard_requested.load(Ordering::Acquire)
        };

        self.set_phase(FinalizationPhase::WindingDown);
        if should_discard() {
            self.discard_recording(recording)?;
        } else {
            let key = (self.inner.deps.queue_signature_key)().map_err(|error| {
                SandTeachRecordingError::new(
                    "finalize_failed",
                    format!("teach-recording: queue signing key unavailable: {error}"),
                )
            })?;
            let signature = sign_queue_entry(
                &key,
                &recording.agent_id,
                &queue_file_for_session_dir(&recording.session_dir),
            );
            let command = save_recording_command(recording, ended_at_ms, &signature);
            let result = self
                .run_shell(
                    &recording.agent_id,
                    &command,
                    "sand-teach-recording-stop",
                )
                .map_err(|error| {
                    SandTeachRecordingError::new(
                        "finalize_failed",
                        format!("teach-recording: failed to finalize recording: {error}"),
                    )
                })?;
            if result.exit_code != 0 {
                return Err(SandTeachRecordingError::new(
                    "finalize_failed",
                    format!(
                        "teach-recording: failed to finalize recording: {} {}",
                        result.stdout, result.stderr
                    ),
                ));
            }
            self.inner
                .state
                .lock()
                .expect("teach recording state poisoned")
                .queue_persisted = true;

            if should_discard() {
                self.discard_recording(recording)?;
            }
        }

        if !should_discard() {
            self.set_phase(FinalizationPhase::CheckingWorkflow);
            let workflow_available =
                (self.inner.deps.ensure_learning_workflow)().map_err(|error| {
                    SandTeachRecordingError::new(
                        "workflow_unavailable",
                        format!(
                            "teach-recording: learning workflow availability failed: {error}"
                        ),
                    )
                })?;
            if should_discard() {
                self.discard_recording(recording)?;
            } else if !workflow_available {
                return Err(SandTeachRecordingError::new(
                    "workflow_unavailable",
                    "teach-recording: learning workflow is unavailable",
                ));
            } else {
                self.ensure_authentic_queue(&recording.agent_id)?;
                if should_discard() {
                    self.discard_recording(recording)?;
                } else {
                    self.set_phase(FinalizationPhase::DispatchingPrompt);
                    let pending = PendingRecording {
                        agent_id: recording.agent_id.clone(),
                        queue_file: queue_file_for_session_dir(&recording.session_dir),
                        client_nonce: learning_prompt_nonce(recording),
                    };
                    let rich_text = learning_prompt_rich_text(&recording.agent_id);
                    (self.inner.deps.send_learning_prompt)(
                        &recording.agent_id,
                        "The recording is finished. Learn the task from it.",
                        &pending.client_nonce,
                        Some(&rich_text),
                    )
                    .map_err(|error| {
                        SandTeachRecordingError::new(
                            "prompt_failed",
                            format!(
                                "teach-recording: failed to send learning prompt: {error}"
                            ),
                        )
                    })?;
                    self.mark_prompt_delivered(&recording.agent_id, &pending)?;
                }
            }
        }

        let outcome_name = if should_discard() {
            "discarded"
        } else {
            "saved"
        };
        {
            let mut state = self.inner.state.lock().expect("teach recording state poisoned");
            state.active = None;
            state.failed_start = None;
            state.queue_persisted = false;
        }
        self.invalidate_cap();
        self.emit();
        if track_completion {
            (self.inner.deps.track_recording_stopped)(json!({
                "agent_id": recording.agent_id,
                "outcome": outcome_name,
                "duration_seconds": ((ended_at_ms.saturating_sub(recording.started_at_ms) + 500) / 1000),
            }));
        }
        Ok(self.status())
    }

    pub fn recover_pending(&self) -> Result<(), SandTeachRecordingError> {
        if !(self.inner.deps.is_enabled)() {
            return Ok(());
        }
        let agent_ids = (self.inner.deps.list_agent_ids)().map_err(|error| {
            SandTeachRecordingError::new(
                "recover_failed",
                format!("teach-recording: failed to list agents: {error}"),
            )
        })?;
        let Some(shell_agent_id) = agent_ids.first() else {
            return Ok(());
        };
        self.inner
            .runtime
            .ensure_ready(shell_agent_id)
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "recover_failed",
                    format!("teach-recording: failed to open box for recovery: {error}"),
                )
            })?;
        let key = (self.inner.deps.queue_signature_key)().map_err(|error| {
            SandTeachRecordingError::new(
                "recover_failed",
                format!("teach-recording: queue signing key unavailable: {error}"),
            )
        })?;
        let agents_by_scope = agent_ids
            .iter()
            .map(|agent_id| (queue_scope(agent_id), agent_id.clone()))
            .collect::<HashMap<_, _>>();
        let scan = self
            .run_shell(
                shell_agent_id,
                &queue_scan_command(&format!("{TEACH_QUEUES_DIR}/*/pending/*.json")),
                "sand-teach-recording-recover",
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "recover_failed",
                    format!("teach-recording: failed to recover pending recordings: {error}"),
                )
            })?;
        if scan.exit_code != 0 {
            return Err(SandTeachRecordingError::new(
                "recover_failed",
                format!(
                    "teach-recording: failed to recover pending recordings: {} {}",
                    scan.stdout, scan.stderr
                ),
            ));
        }
        let entries = parse_queue_scan(&scan.stdout, &key, &agents_by_scope);
        let forged = entries
            .iter()
            .filter(|entry| entry.recording.is_none())
            .map(|entry| (entry.scope.clone(), entry.queue_file.clone()))
            .collect::<Vec<_>>();
        self.quarantine_queue_files(shell_agent_id, &forged)?;
        let pending = entries
            .into_iter()
            .filter_map(|entry| {
                if entry.delivered {
                    None
                } else {
                    entry.recording
                }
            })
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }
        let workflow_available =
            (self.inner.deps.ensure_learning_workflow)().map_err(|error| {
                SandTeachRecordingError::new(
                    "recover_failed",
                    format!("teach-recording: learning workflow check failed: {error}"),
                )
            })?;
        if !workflow_available {
            return Ok(());
        }
        for recording in pending {
            let rich_text = learning_prompt_rich_text(&recording.agent_id);
            (self.inner.deps.send_learning_prompt)(
                &recording.agent_id,
                "The recording is finished. Learn the task from it.",
                &recording.client_nonce,
                Some(&rich_text),
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "recover_failed",
                    format!("teach-recording: failed to redeliver learning prompt: {error}"),
                )
            })?;
            self.mark_prompt_delivered(shell_agent_id, &recording)?;
        }
        Ok(())
    }

    pub fn dispose(&self) {
        let active = {
            let mut state = self.inner.state.lock().expect("teach recording state poisoned");
            if state.disposed {
                return;
            }
            state.disposed = true;
            state.active.clone().filter(|_| !state.queue_persisted)
        };
        self.invalidate_cap();
        if let Some(recording) = active {
            let _ = self.stop_internal(&recording.agent_id, false, false);
        }
        self.inner
            .state
            .lock()
            .expect("teach recording state poisoned")
            .listeners
            .clear();
    }

    fn recover_failed_start(&self) -> Result<(), SandTeachRecordingError> {
        let recording = self
            .inner
            .state
            .lock()
            .expect("teach recording state poisoned")
            .failed_start
            .clone();
        let Some(recording) = recording else {
            return Ok(());
        };
        self.cleanup_failed_start(&recording)?;
        let mut state = self.inner.state.lock().expect("teach recording state poisoned");
        if state.failed_start.as_ref() == Some(&recording) {
            state.failed_start = None;
        }
        Ok(())
    }

    fn cleanup_failed_start(
        &self,
        recording: &Recording,
    ) -> Result<(), SandTeachRecordingError> {
        self.inner
            .runtime
            .ensure_ready(&recording.agent_id)
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "cleanup_failed",
                    format!("teach-recording: cleanup box unavailable: {error}"),
                )
            })?;
        self.discard_recording(recording)
    }

    fn discard_recording(
        &self,
        recording: &Recording,
    ) -> Result<(), SandTeachRecordingError> {
        let command = discard_recording_command(recording);
        let result = self
            .run_shell(
                &recording.agent_id,
                &command,
                "sand-teach-recording-discard",
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "cleanup_failed",
                    format!("teach-recording: discard command failed: {error}"),
                )
            })?;
        if result.exit_code != 0 {
            return Err(SandTeachRecordingError::new(
                "finalize_failed",
                format!(
                    "teach-recording: failed to finalize recording: {} {}",
                    result.stdout, result.stderr
                ),
            ));
        }
        self.inner
            .state
            .lock()
            .expect("teach recording state poisoned")
            .queue_persisted = false;
        Ok(())
    }

    fn ensure_authentic_queue(&self, agent_id: &str) -> Result<(), SandTeachRecordingError> {
        let key = (self.inner.deps.queue_signature_key)().map_err(|error| {
            SandTeachRecordingError::new(
                "finalize_failed",
                format!("teach-recording: queue signing key unavailable: {error}"),
            )
        })?;
        let scope = queue_scope(agent_id);
        let scan = self
            .run_shell(
                agent_id,
                &queue_scan_command(&format!(
                    "{TEACH_QUEUES_DIR}/{scope}/pending/*.json"
                )),
                "sand-teach-recording-verify",
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "finalize_failed",
                    format!("teach-recording: failed to verify queue authenticity: {error}"),
                )
            })?;
        if scan.exit_code != 0 {
            return Err(SandTeachRecordingError::new(
                "finalize_failed",
                format!(
                    "teach-recording: failed to verify queue authenticity: {} {}",
                    scan.stdout, scan.stderr
                ),
            ));
        }
        let agents = HashMap::from([(scope, agent_id.to_string())]);
        let entries = parse_queue_scan(&scan.stdout, &key, &agents);
        let forged = entries
            .iter()
            .filter(|entry| entry.recording.is_none())
            .map(|entry| (entry.scope.clone(), entry.queue_file.clone()))
            .collect::<Vec<_>>();
        self.quarantine_queue_files(agent_id, &forged)
    }

    fn quarantine_queue_files(
        &self,
        shell_agent_id: &str,
        entries: &[(String, String)],
    ) -> Result<(), SandTeachRecordingError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut commands = Vec::new();
        for (scope, queue_file) in entries {
            let dir = format!("{TEACH_QUEUES_DIR}/{scope}");
            commands.push(format!("mkdir -p {dir}/rejected"));
            commands.push(format!(
                "mv -f {dir}/pending/{queue_file} {dir}/rejected/{queue_file} 2>/dev/null || true"
            ));
            commands.push(format!(
                "rm -f {dir}/pending/{queue_file}.prompt-delivered"
            ));
        }
        let result = self
            .run_shell(
                shell_agent_id,
                &commands.join("\n"),
                "sand-teach-recording-quarantine",
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "quarantine_failed",
                    format!("teach-recording: failed to quarantine forged queue entries: {error}"),
                )
            })?;
        if result.exit_code != 0 {
            return Err(SandTeachRecordingError::new(
                "quarantine_failed",
                format!(
                    "teach-recording: failed to quarantine forged queue entries: {} {}",
                    result.stdout, result.stderr
                ),
            ));
        }
        Ok(())
    }

    fn mark_prompt_delivered(
        &self,
        shell_agent_id: &str,
        pending: &PendingRecording,
    ) -> Result<(), SandTeachRecordingError> {
        let scope = queue_scope(&pending.agent_id);
        let command = format!(
            "queue_dir={TEACH_QUEUES_DIR}/{scope}\nqueue_file=\"$queue_dir/pending/{}\"\nif [ -f \"$queue_file\" ]; then : > \"$queue_file.prompt-delivered\"; fi",
            pending.queue_file
        );
        let result = self
            .run_shell(
                shell_agent_id,
                &command,
                "sand-teach-recording-mark-prompt-delivered",
            )
            .map_err(|error| {
                SandTeachRecordingError::new(
                    "mark_prompt_failed",
                    format!("teach-recording: failed to mark learning prompt delivered: {error}"),
                )
            })?;
        if result.exit_code != 0 {
            return Err(SandTeachRecordingError::new(
                "mark_prompt_failed",
                format!(
                    "teach-recording: failed to mark learning prompt delivered: {} {}",
                    result.stdout, result.stderr
                ),
            ));
        }
        Ok(())
    }

    fn run_shell(
        &self,
        agent_id: &str,
        command: &str,
        tool_call_id: &str,
    ) -> Result<ShellResult, String> {
        self.inner
            .runtime
            .run_shell(agent_id, command, tool_call_id)
    }

    fn set_phase(&self, phase: FinalizationPhase) {
        self.inner
            .state
            .lock()
            .expect("teach recording state poisoned")
            .finalizing = Some(phase);
    }

    fn arm_cap(&self, recording: &Recording) {
        let generation = self
            .inner
            .cap_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let delay_ms = self.inner.deps.cap_delay_ms;
        if delay_ms == 0 {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let agent_id = recording.agent_id.clone();
        let session_dir = recording.session_dir.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if inner.cap_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let should_fire = {
                let state = inner.state.lock().expect("teach recording state poisoned");
                !state.disposed
                    && state
                        .active
                        .as_ref()
                        .map(|recording| {
                            recording.agent_id == agent_id
                                && recording.session_dir == session_dir
                        })
                        .unwrap_or(false)
            };
            if !should_fire {
                return;
            }
            let service = SandTeachRecordingService { inner };
            if let Err(error) = service.stop_internal(&agent_id, true, true) {
                (service.inner.deps.report_cap_stop_failed)(json!({
                    "errorClass": "SandTeachRecordingError",
                    "kind": error.kind,
                }));
            }
        });
    }

    fn invalidate_cap(&self) {
        self.inner.cap_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn emit(&self) {
        let (status, listeners) = {
            let state = self.inner.state.lock().expect("teach recording state poisoned");
            (
                status_of(&state),
                state.listeners.values().cloned().collect::<Vec<_>>(),
            )
        };
        for listener in listeners {
            listener(status.clone());
        }
    }

    fn report_start_failure(
        &self,
        agent_id: &str,
        entry_point: Option<&str>,
        error: &SandTeachRecordingError,
    ) {
        (self.inner.deps.report_start_failed)(json!({
            "kind": error.kind,
            "errorClass": "SandTeachRecordingError",
            "windowIndex": self.inner.runtime.get_agent_window_index(agent_id),
            "entryPoint": entry_point,
        }));
    }
}

fn status_of(state: &ServiceState) -> TeachStatus {
    state
        .active
        .as_ref()
        .map(TeachStatus::recording)
        .unwrap_or_else(TeachStatus::idle)
}

pub fn is_fork_window_index(index: Option<u32>) -> bool {
    index
        .map(|index| index >= SAND_BOX_FIRST_FORK_WINDOW_INDEX)
        .unwrap_or(false)
}

pub fn queue_scope(agent_id: &str) -> String {
    hex_lower(&Sha256::digest(agent_id.as_bytes()))
}

pub fn sign_queue_entry(key: &[u8], agent_id: &str, file: &str) -> String {
    let payload = format!("{agent_id}\n{file}");
    hex_lower(&hmac_sha256(key, payload.as_bytes()))
}

pub fn queue_file_for_session_dir(dir: &str) -> String {
    format!("{}.json", dir.rsplit('/').next().unwrap_or_default())
}

pub fn learning_prompt_nonce(recording: &Recording) -> String {
    format!(
        "teach-recording:{}:{}",
        queue_scope(&recording.agent_id),
        queue_file_for_session_dir(&recording.session_dir)
    )
}

pub fn learning_prompt_rich_text(agent_id: &str) -> String {
    json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [{
                "type": "workflowReference",
                "attrs": {
                    "id": LEARN_SKILL_NAME,
                    "label": "Learn from demonstration",
                    "teachQueueScope": queue_scope(agent_id),
                }
            }]
        }]
    })
    .to_string()
}

fn hmac_sha256(key: &[u8], payload: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = Sha256::digest(key);
        key_block[..32].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK];
    let mut outer_pad = [0x5c_u8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = [0_u8; 32];
    for index in 0..32 {
        output[index] =
            u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

fn iso_timestamp(ms: u64) -> String {
    Utc.timestamp_millis_opt(ms as i64)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn session_json(recording: &Recording, end: Option<(u64, &str)>) -> String {
    let mut value = json!({
        "startedAt": iso_timestamp(recording.started_at_ms),
        "display": recording.display_label,
        "maxDurationMs": SAND_TEACH_MAX_DURATION_MS,
        "videoPath": format!("{}/demo.mp4", recording.session_dir),
        "ffmpegPid": "__FFMPEG_PID__",
    });
    if let Some((ended_at_ms, end_reason)) = end {
        value["endedAt"] = Value::String(iso_timestamp(ended_at_ms));
        value["endReason"] = Value::String(end_reason.to_string());
    }
    serde_json::to_string_pretty(&value)
        .unwrap_or_default()
        .replace("\"__FFMPEG_PID__\"", "$pid")
}

fn completed_session_json(
    recording: &Recording,
    ended_at_ms: u64,
    signature: &str,
) -> String {
    serde_json::to_string_pretty(&json!({
        "agentId": recording.agent_id,
        "sessionDir": recording.session_dir,
        "clientNonce": learning_prompt_nonce(recording),
        "signature": signature,
        "startedAt": iso_timestamp(recording.started_at_ms),
        "endedAt": iso_timestamp(ended_at_ms),
    }))
    .unwrap_or_default()
}

fn ffmpeg_cmdline_owns_video(pid_expr: &str, video_path: &str) -> String {
    format!(
        "kill -0 {pid_expr} 2>/dev/null && tr '\\0' ' ' < /proc/{pid_expr}/cmdline 2>/dev/null | grep -qF {}",
        serde_json::to_string(video_path).unwrap_or_else(|_| "\"\"".into())
    )
}

fn start_recording_command(recording: &Recording, window_index: u32) -> String {
    let video_path = format!("{}/demo.mp4", recording.session_dir);
    let max_seconds = SAND_TEACH_MAX_DURATION_MS / 1_000;
    let verify = ffmpeg_cmdline_owns_video("\"$pid\"", &video_path);
    [
        format!("mkdir -p {}", recording.session_dir),
        format!(
            "nohup ffmpeg -y -v error -f x11grab -video_size {SAND_MONITOR_WIDTH}x{SAND_MONITOR_HEIGHT} -i :{window_index}.0 -framerate 15 -t {max_seconds} -c:v libx264 -preset ultrafast -g 75 -pix_fmt yuv420p -an {video_path} > {}/ffmpeg.log 2>&1 &",
            recording.session_dir
        ),
        format!(
            "printf '%s\\n' \"$!\" > {}/ffmpeg.pid",
            recording.session_dir
        ),
        "sleep 0.4".into(),
        format!(
            "pid=$(tr -d '[:space:]' < {}/ffmpeg.pid)",
            recording.session_dir
        ),
        format!(
            "case \"$pid\" in ''|*[!0-9]*) cat {}/ffmpeg.log; exit 1 ;; esac",
            recording.session_dir
        ),
        format!(
            "{verify} || {{ cat {}/ffmpeg.log; exit 1; }}",
            recording.session_dir
        ),
        format!(
            "cat > {}/session.json <<SESSION_JSON",
            recording.session_dir
        ),
        session_json(recording, None),
        "SESSION_JSON".into(),
    ]
    .join("\n")
}

fn discard_recording_command(recording: &Recording) -> String {
    let video_path = format!("{}/demo.mp4", recording.session_dir);
    let verify = ffmpeg_cmdline_owns_video("\"$pid\"", &video_path);
    let scope = queue_scope(&recording.agent_id);
    let queue_file = queue_file_for_session_dir(&recording.session_dir);
    [
        format!(
            "pid=$(tr -d '[:space:]' < {}/ffmpeg.pid 2>/dev/null || true)",
            recording.session_dir
        ),
        format!("if [ -n \"$pid\" ] && {verify}; then"),
        "  kill -INT \"$pid\" || true".into(),
        "  sleep 1".into(),
        format!("  if {verify}; then kill -KILL \"$pid\" || true; fi"),
        "  for _ in 1 2 3 4 5; do".into(),
        format!("    if ! ( {verify} ); then break; fi"),
        "    sleep 0.2".into(),
        "  done".into(),
        format!("  if {verify}; then"),
        format!("    : > {}/startup.invalid", recording.session_dir),
        "    exit 1".into(),
        "  fi".into(),
        "fi".into(),
        format!("queue_dir={TEACH_QUEUES_DIR}/{scope}"),
        format!("queue_file=\"$queue_dir/pending/{queue_file}\""),
        format!(
            "rm -f \"$queue_file\" \"$queue_file.prompt-delivered\" \"$queue_dir/claimed/{queue_file}\""
        ),
        format!("rm -rf {}", recording.session_dir),
    ]
    .join("\n")
}

fn save_recording_command(
    recording: &Recording,
    ended_at_ms: u64,
    signature: &str,
) -> String {
    let video_path = format!("{}/demo.mp4", recording.session_dir);
    let verify = ffmpeg_cmdline_owns_video("\"$pid\"", &video_path);
    let scope = queue_scope(&recording.agent_id);
    let queue_file = queue_file_for_session_dir(&recording.session_dir);
    [
        "set -e".into(),
        format!(
            "pid=$(tr -d '[:space:]' < {}/ffmpeg.pid) || exit 1",
            recording.session_dir
        ),
        "case \"$pid\" in ''|*[!0-9]*) exit 1 ;; esac".into(),
        format!("if {verify}; then kill -INT \"$pid\"; fi"),
        format!(
            "cat > {}/session.json <<SESSION_JSON",
            recording.session_dir
        ),
        session_json(recording, Some((ended_at_ms, "stopped"))),
        "SESSION_JSON".into(),
        format!("queue_dir={TEACH_QUEUES_DIR}/{scope}"),
        format!("queue_file=\"$queue_dir/pending/{queue_file}\""),
        "mkdir -p \"$queue_dir/pending\" \"$queue_dir/claimed\"".into(),
        "cat > \"$queue_file.tmp.$$\" <<COMPLETED_SESSION_JSON".into(),
        completed_session_json(recording, ended_at_ms, signature),
        "COMPLETED_SESSION_JSON".into(),
        "if [ ! -e \"$queue_file\" ] && [ ! -e \"$queue_dir/claimed/$(basename \"$queue_file\")\" ]; then mv \"$queue_file.tmp.$$\" \"$queue_file\"; else rm -f \"$queue_file.tmp.$$\"; fi".into(),
    ]
    .join("\n")
}

fn queue_scan_command(glob: &str) -> String {
    [
        format!("for queue_file in {glob}; do"),
        "  [ -f \"$queue_file\" ] || continue".into(),
        "  if [ -e \"$queue_file.prompt-delivered\" ]; then marker=1; else marker=0; fi".into(),
        "  printf '%s\\t%s\\t%s\\t' \"$(basename \"$(dirname \"$(dirname \"$queue_file\")\")\")\" \"$(basename \"$queue_file\")\" \"$marker\"".into(),
        "  base64 \"$queue_file\" | tr -d '\\n'".into(),
        "  printf '\\n'".into(),
        "done".into(),
    ]
    .join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueScanEntry {
    scope: String,
    queue_file: String,
    delivered: bool,
    recording: Option<PendingRecording>,
}

fn parse_queue_scan(
    stdout: &str,
    key: &[u8],
    agents_by_scope: &HashMap<String, String>,
) -> Vec<QueueScanEntry> {
    stdout
        .lines()
        .filter_map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            if parts.len() != 4 {
                return None;
            }
            let scope = parts[0];
            let queue_file = parts[1];
            let marker = parts[2];
            let content = parts[3];
            if !is_hex_64(scope)
                || !is_queue_file_name(queue_file)
                || !matches!(marker, "0" | "1")
            {
                return None;
            }
            let agent_id = agents_by_scope.get(scope)?;
            Some(QueueScanEntry {
                scope: scope.to_string(),
                queue_file: queue_file.to_string(),
                delivered: marker == "1",
                recording: pending_recording(agent_id, key, queue_file, content),
            })
        })
        .collect()
}

fn pending_recording(
    agent_id: &str,
    key: &[u8],
    queued_file: &str,
    content_base64: &str,
) -> Option<PendingRecording> {
    let bytes = BASE64_STANDARD.decode(content_base64).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let session_name = queued_file.strip_suffix(".json")?;
    let session_dir = format!("{TEACH_SESSIONS_DIR}/{session_name}");
    let client_nonce = format!(
        "teach-recording:{}:{queued_file}",
        queue_scope(agent_id)
    );
    if value.get("agentId").and_then(Value::as_str) != Some(agent_id)
        || value.get("sessionDir").and_then(Value::as_str) != Some(session_dir.as_str())
        || value.get("clientNonce").and_then(Value::as_str) != Some(client_nonce.as_str())
    {
        return None;
    }
    let signature = value.get("signature").and_then(Value::as_str)?;
    let given = decode_hex_32(signature)?;
    let expected = decode_hex_32(&sign_queue_entry(key, agent_id, queued_file))?;
    if !bool::from(expected.as_slice().ct_eq(given.as_slice())) {
        return None;
    }
    Some(PendingRecording {
        agent_id: agent_id.to_string(),
        queue_file: queued_file.to_string(),
        client_nonce,
    })
}

fn is_hex_64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_queue_file_name(value: &str) -> bool {
    let Some(core) = value
        .strip_prefix("teach-")
        .and_then(|value| value.strip_suffix(".json"))
    else {
        return false;
    };
    if core.len() != 53 {
        return false;
    }
    let stamp = &core[..16];
    if stamp.as_bytes()[8] != b'T'
        || stamp.as_bytes()[15] != b'Z'
        || !stamp[..8].bytes().all(|byte| byte.is_ascii_digit())
        || !stamp[9..15].bytes().all(|byte| byte.is_ascii_digit())
        || core.as_bytes()[16] != b'-'
    {
        return false;
    }
    Uuid::parse_str(&core[17..]).is_ok()
}

fn now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}
