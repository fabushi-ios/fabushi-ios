use std::error::Error;

use thiserror::Error;

pub const SAND_BOX_NOT_READY_MESSAGE: &str =
    "The computer is still starting up (downloading its image or booting). Try again in a moment.";
pub const SAND_BOX_NOT_RESPONDING_MESSAGE: &str =
    "The computer isn't responding — it may be wedged. Try again in a moment, or recover it from Settings → Updates → Update Fabushi's Computer.";
pub const SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE: &str =
    "Every desktop monitor on the shared computer is in use right now, so this agent can't get its own screen. Wait a moment and try again — one frees up when another agent or parallel computer-use subagent finishes.";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct SandBoxDaemonUnreachableError {
    pub outcome: String,
    pub message: String,
}

impl SandBoxDaemonUnreachableError {
    pub fn new(outcome: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            outcome: outcome.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SandBoxNoMonitorAvailableError(pub String);

impl Default for SandBoxNoMonitorAvailableError {
    fn default() -> Self {
        Self("No private desktop monitor is available on the shared box.".into())
    }
}

pub fn box_not_ready_message_for_error(error: &(dyn Error + 'static)) -> &'static str {
    if error.downcast_ref::<SandBoxNoMonitorAvailableError>().is_some() {
        return SAND_BOX_NO_MONITOR_AVAILABLE_MESSAGE;
    }
    if let Some(error) = error.downcast_ref::<SandBoxDaemonUnreachableError>() {
        if matches!(error.outcome.as_str(), "timeout" | "crash") {
            return SAND_BOX_NOT_RESPONDING_MESSAGE;
        }
    }
    SAND_BOX_NOT_READY_MESSAGE
}

#[derive(Debug)]
pub struct NoMonitorComputerUseExecutor {
    identity: u8,
}

impl NoMonitorComputerUseExecutor {
    pub const fn detached() -> Self {
        Self { identity: 1 }
    }

    pub fn execute<T>(&self) -> Result<T, SandBoxNoMonitorAvailableError> {
        let _ = self.identity;
        Err(SandBoxNoMonitorAvailableError::default())
    }
}

pub static NO_MONITOR_COMPUTER_USE_EXECUTOR: NoMonitorComputerUseExecutor =
    NoMonitorComputerUseExecutor { identity: 0 };

pub fn is_no_monitor_computer_use_executor(executor: &NoMonitorComputerUseExecutor) -> bool {
    std::ptr::eq(executor, &NO_MONITOR_COMPUTER_USE_EXECUTOR)
}

pub const SAND_BOX_PRIMARY_WINDOW_INDEX: u32 = 1;
pub const SAND_BOX_FIRST_FORK_WINDOW_INDEX: u32 = 2;

pub fn is_primary_window_index(window_index: u32) -> bool {
    window_index <= SAND_BOX_PRIMARY_WINDOW_INDEX
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("This box does not support environment sync.")]
pub struct BoxEnvironmentSyncUnsupportedError;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("This box does not support running MCP servers.")]
pub struct BoxMcpUnsupportedError;
