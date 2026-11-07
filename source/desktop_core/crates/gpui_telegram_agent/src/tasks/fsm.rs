use gpui_telegram_core::error::{Result, TelegramError};
use mahayana_host_runtime::runner::background_work::{
    BackgroundWorkRecord, RevivingBackgroundWorkRegistry,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Running,
    WaitingAuth,
    WaitingConnection,
    Completed,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::WaitingAuth | Self::WaitingConnection
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::WaitingAuth => "waiting_auth",
            Self::WaitingConnection => "waiting_connection",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "waiting_auth" => Some(Self::WaitingAuth),
            "waiting_connection" => Some(Self::WaitingConnection),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFsm {
    pub task_id: String,
    pub state: TaskState,
    pub state_history: Vec<(TaskState, u64)>,
    pub abort_reason: Option<String>,
}

impl TaskFsm {
    pub fn new(task_id: String) -> Self {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        Self {
            task_id,
            state: TaskState::Pending,
            state_history: vec![(TaskState::Pending, now_ms)],
            abort_reason: None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    pub fn transition_to(&mut self, target: TaskState) -> Result<()> {
        let valid = match (self.state, target) {
            // From Pending: Started by worker or cancelled before launch
            (TaskState::Pending, TaskState::Running) => true,
            (TaskState::Pending, TaskState::Cancelled) => true,

            // From Running: Requires user approval, network dropped, finished, failed, or cancelled
            (TaskState::Running, TaskState::WaitingAuth) => true,
            (TaskState::Running, TaskState::WaitingConnection) => true,
            (TaskState::Running, TaskState::Completed) => true,
            (TaskState::Running, TaskState::Failed) => true,
            (TaskState::Running, TaskState::Cancelled) => true,

            // From WaitingAuth: Approved -> Running; Denied / Expired -> Cancelled
            (TaskState::WaitingAuth, TaskState::Running) => true,
            (TaskState::WaitingAuth, TaskState::Cancelled) => true,

            // From WaitingConnection: Reconnected -> Running; Disconnected -> Cancelled
            (TaskState::WaitingConnection, TaskState::Running) => true,
            (TaskState::WaitingConnection, TaskState::Cancelled) => true,

            // Terminal states (Completed, Failed, Cancelled) cannot transition
            _ => false,
        };

        if valid {
            let now_ms = chrono::Utc::now().timestamp_millis() as u64;
            self.state = target;
            self.state_history.push((target, now_ms));
            Ok(())
        } else {
            Err(TelegramError::Validation(format!(
                "Invalid task state transition from {:?} to {:?}",
                self.state, target
            )))
        }
    }

    pub fn abort(&mut self, reason: impl Into<String>) -> Result<()> {
        self.abort_reason = Some(reason.into());
        self.transition_to(TaskState::Cancelled)
    }

    /// Drive and synchronize state with inherited RevivingBackgroundWorkRegistry
    pub fn sync_to_background_registry(&self, registry: &mut RevivingBackgroundWorkRegistry) {
        let record = BackgroundWorkRecord {
            id: self.task_id.clone(),
            kind: "agent_task".into(),
            state: self.state.as_str().into(),
            owner_id: None,
            metadata: Some(serde_json::json!({
                "state_history_count": self.state_history.len(),
                "abort_reason": self.abort_reason,
            })),
            abort: None,
        };
        registry.upsert_work(record);
    }

    /// Update FSM from an external BackgroundWorkRecord
    pub fn update_from_background_record(
        &mut self,
        record: &BackgroundWorkRecord,
    ) -> Result<()> {
        if let Some(target_state) = TaskState::from_str(&record.state) {
            if target_state != self.state {
                self.transition_to(target_state)?;
            }
        }
        Ok(())
    }

    /// Construct a TaskFsm from an existing BackgroundWorkRecord
    pub fn from_background_record(record: &BackgroundWorkRecord) -> Option<Self> {
        let state = TaskState::from_str(&record.state)?;
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        Some(Self {
            task_id: record.id.clone(),
            state,
            state_history: vec![(state, now_ms)],
            abort_reason: None,
        })
    }
}
