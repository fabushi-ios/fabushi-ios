use std::error::Error;
use std::sync::Arc;

use thiserror::Error;

use crate::ports::r#box::{
    SAND_BOX_NOT_READY_MESSAGE, SandBoxNoMonitorAvailableError,
    box_not_ready_message_for_error,
};

use super::sand_auto_review::{SandAutoReviewMode, SandAutoReviewModes};

pub type RemoteConnectError = Arc<dyn Error + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SandBoxNotReadyError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConnection<Resource> {
    pub terminals_folder: String,
    pub resource: Resource,
    pub owns_monitor: bool,
    pub window_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteShellKind {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteResourceKind {
    ShellStream,
    BackgroundShell,
    Read,
    Shell,
    ComputerUse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteResourceLifecycleEvent {
    AutoReviewBarrier,
    AuditShell {
        agent_id: String,
        kind: RemoteShellKind,
        command: String,
        turn_id: Option<String>,
        box_id: String,
    },
    CaptureNavigationBaseline {
        window_index: u32,
    },
    TouchMonitorBusyLease {
        window_index: u32,
    },
    Delegate {
        resource: RemoteResourceKind,
    },
    RecordComputerAuditIntent {
        action_case: Option<String>,
    },
    ProbeNavigation,
}

#[derive(Debug, Clone)]
pub struct RemoteBoxResourceCoordinator<Resource> {
    cached_connection: Option<RemoteConnection<Resource>>,
    prepared_connection: Option<RemoteConnection<Resource>>,
    remote_box_has_desktop: bool,
    terminals_folder: Option<String>,
}

impl<Resource> RemoteBoxResourceCoordinator<Resource>
where
    Resource: Clone,
{
    pub fn new(
        remote_box_has_desktop: bool,
        prepared_connection: Option<RemoteConnection<Resource>>,
    ) -> Self {
        Self {
            cached_connection: None,
            prepared_connection,
            remote_box_has_desktop,
            terminals_folder: None,
        }
    }

    pub fn terminals_folder(&self) -> Option<&str> {
        self.terminals_folder.as_deref()
    }

    pub fn has_cached_connection(&self) -> bool {
        self.cached_connection.is_some()
    }

    pub fn clear_connection(&mut self) {
        self.cached_connection = None;
    }

    pub fn connect<Ensure>(
        &mut self,
        box_preparing: bool,
        ensure_ready: Ensure,
    ) -> Result<RemoteConnection<Resource>, SandBoxNotReadyError>
    where
        Ensure: FnOnce() -> Result<RemoteConnection<Resource>, RemoteConnectError>,
    {
        if box_preparing {
            return Err(SandBoxNotReadyError(SAND_BOX_NOT_READY_MESSAGE.to_string()));
        }
        if let Some(connection) = self.cached_connection.clone() {
            return Ok(connection);
        }

        let resolved = match self.prepared_connection.take() {
            Some(connection) => Ok(connection),
            None => ensure_ready(),
        };
        match resolved {
            Ok(connection) => {
                self.terminals_folder = Some(connection.terminals_folder.clone());
                self.cached_connection = Some(connection.clone());
                Ok(connection)
            }
            Err(error) => {
                self.cached_connection = None;
                Err(SandBoxNotReadyError(
                    box_not_ready_message_for_error(error.as_ref()).to_string(),
                ))
            }
        }
    }

    pub fn no_monitor_error(&mut self) -> SandBoxNotReadyError {
        self.cached_connection = None;
        let error = SandBoxNoMonitorAvailableError::default();
        SandBoxNotReadyError(box_not_ready_message_for_error(&error).to_string())
    }

    fn owns_monitor_for_navigation_audit(
        &self,
        connection: &RemoteConnection<Resource>,
    ) -> bool {
        self.remote_box_has_desktop && connection.owns_monitor
    }

    pub fn shell_stream_plan(
        &self,
        connection: &RemoteConnection<Resource>,
        agent_id: &str,
        command: &str,
        turn_id: Option<&str>,
        box_id: &str,
    ) -> Vec<RemoteResourceLifecycleEvent> {
        self.shell_side_effect_plan(
            connection,
            RemoteShellKind::Foreground,
            RemoteResourceKind::ShellStream,
            agent_id,
            command,
            turn_id,
            box_id,
        )
    }

    pub fn background_shell_plan(
        &self,
        connection: &RemoteConnection<Resource>,
        agent_id: &str,
        command: &str,
        turn_id: Option<&str>,
        box_id: &str,
    ) -> Vec<RemoteResourceLifecycleEvent> {
        self.shell_side_effect_plan(
            connection,
            RemoteShellKind::Background,
            RemoteResourceKind::BackgroundShell,
            agent_id,
            command,
            turn_id,
            box_id,
        )
    }

    fn shell_side_effect_plan(
        &self,
        connection: &RemoteConnection<Resource>,
        kind: RemoteShellKind,
        resource: RemoteResourceKind,
        agent_id: &str,
        command: &str,
        turn_id: Option<&str>,
        box_id: &str,
    ) -> Vec<RemoteResourceLifecycleEvent> {
        let owns_monitor = self.owns_monitor_for_navigation_audit(connection);
        let mut events = vec![
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::AuditShell {
                agent_id: agent_id.to_string(),
                kind,
                command: command.to_string(),
                turn_id: turn_id.map(ToOwned::to_owned),
                box_id: box_id.to_string(),
            },
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
        ];
        if owns_monitor {
            events.push(RemoteResourceLifecycleEvent::CaptureNavigationBaseline {
                window_index: connection.window_index,
            });
        }
        events.push(RemoteResourceLifecycleEvent::AutoReviewBarrier);
        events.push(RemoteResourceLifecycleEvent::Delegate { resource });
        if owns_monitor {
            events.push(RemoteResourceLifecycleEvent::ProbeNavigation);
        }
        events
    }

    pub fn read_plan(&self) -> Vec<RemoteResourceLifecycleEvent> {
        vec![RemoteResourceLifecycleEvent::Delegate {
            resource: RemoteResourceKind::Read,
        }]
    }

    pub fn shell_plan(&self) -> Vec<RemoteResourceLifecycleEvent> {
        vec![
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::AutoReviewBarrier,
            RemoteResourceLifecycleEvent::Delegate {
                resource: RemoteResourceKind::Shell,
            },
        ]
    }

    pub fn computer_use_plan(
        &self,
        connection: &RemoteConnection<Resource>,
        action_case: Option<&str>,
    ) -> Result<Vec<RemoteResourceLifecycleEvent>, SandBoxNotReadyError> {
        if !connection.owns_monitor {
            let error = SandBoxNoMonitorAvailableError::default();
            return Err(SandBoxNotReadyError(
                box_not_ready_message_for_error(&error).to_string(),
            ));
        }
        let mut events = Vec::new();
        if self.owns_monitor_for_navigation_audit(connection) {
            events.push(RemoteResourceLifecycleEvent::CaptureNavigationBaseline {
                window_index: connection.window_index,
            });
            events.push(RemoteResourceLifecycleEvent::TouchMonitorBusyLease {
                window_index: connection.window_index,
            });
        }
        events.push(RemoteResourceLifecycleEvent::AutoReviewBarrier);
        events.push(RemoteResourceLifecycleEvent::Delegate {
            resource: RemoteResourceKind::ComputerUse,
        });
        events.push(RemoteResourceLifecycleEvent::RecordComputerAuditIntent {
            action_case: action_case.map(ToOwned::to_owned),
        });
        if self.owns_monitor_for_navigation_audit(connection) {
            events.push(RemoteResourceLifecycleEvent::ProbeNavigation);
        }
        Ok(events)
    }
}

/// Execute a frozen remote-resource lifecycle plan while preserving Grok's
/// delegate/finally semantics. Post-delegate audit intent only belongs to a
/// successful ComputerUse call, while navigation probing is a finally effect
/// and still runs after a delegate error.
pub fn execute_remote_resource_lifecycle_plan<T, E, Event, Delegate>(
    plan: &[RemoteResourceLifecycleEvent],
    mut execute_event: Event,
    mut delegate: Delegate,
) -> Result<T, E>
where
    Event: FnMut(&RemoteResourceLifecycleEvent) -> Result<(), E>,
    Delegate: FnMut(RemoteResourceKind) -> Result<T, E>,
{
    let mut delegated = None;
    let mut delegate_failed = false;
    for event in plan {
        match event {
            RemoteResourceLifecycleEvent::Delegate { resource } => {
                assert!(
                    delegated.is_none(),
                    "remote resource lifecycle plan delegated more than once"
                );
                let result = delegate(*resource);
                delegate_failed = result.is_err();
                delegated = Some(result);
            }
            RemoteResourceLifecycleEvent::RecordComputerAuditIntent { .. }
                if delegate_failed => {}
            _ => execute_event(event)?,
        }
    }
    delegated
        .expect("remote resource lifecycle plan must delegate exactly once")
}

pub fn should_register_auto_review_classifier(
    executor_available: bool,
    modes: &SandAutoReviewModes,
) -> bool {
    executor_available
        && [
            modes.host_shell,
            modes.box_shell,
            modes.mcp,
            modes.computer,
            modes.automation_write,
            modes.cloud_agent,
            modes.subagent_launch,
        ]
        .into_iter()
        .any(|mode| mode != SandAutoReviewMode::Off)
}
