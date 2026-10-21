//! Static package runner state machine. Terminal states cannot be reopened.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunnerPhase {
    WaitingForPlatform,
    Running,
    Cancelled,
    Failed,
    Closed,
}

#[derive(Clone, Debug)]
pub struct RunnerState {
    phase: RunnerPhase,
    revision: u64,
    terminal_message: Option<String>,
}

impl Default for RunnerState {
    fn default() -> Self {
        Self {
            phase: RunnerPhase::WaitingForPlatform,
            revision: 1,
            terminal_message: None,
        }
    }
}

impl RunnerState {
    pub fn phase(&self) -> RunnerPhase {
        self.phase
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn terminal_message(&self) -> Option<&str> {
        self.terminal_message.as_deref()
    }

    pub fn platform_ready(&mut self) -> Result<(), String> {
        if self.phase != RunnerPhase::WaitingForPlatform {
            return Err("Dandelion runner bootstrap cannot settle from its current state".into());
        }
        self.phase = RunnerPhase::Running;
        self.bump();
        Ok(())
    }

    pub fn cancel(&mut self, reason: impl Into<String>) {
        if matches!(self.phase, RunnerPhase::Closed | RunnerPhase::Cancelled) {
            return;
        }
        self.phase = RunnerPhase::Cancelled;
        self.terminal_message = Some(reason.into());
        self.bump();
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        if matches!(
            self.phase,
            RunnerPhase::Closed | RunnerPhase::Cancelled | RunnerPhase::Failed
        ) {
            return;
        }
        self.phase = RunnerPhase::Failed;
        self.terminal_message = Some(reason.into());
        self.bump();
    }

    pub fn close(&mut self) {
        if self.phase == RunnerPhase::Closed {
            return;
        }
        self.phase = RunnerPhase::Closed;
        self.bump();
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        if self.revision == 0 {
            self.revision = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_state_never_reopens() {
        let mut state = RunnerState::default();
        state.cancel("account changed");
        state.fail("late platform failure");
        assert_eq!(state.phase(), RunnerPhase::Cancelled);
        assert!(state.platform_ready().is_err());
        state.close();
        assert_eq!(state.phase(), RunnerPhase::Closed);
    }
}
