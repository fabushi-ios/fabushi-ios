use std::sync::Arc;

use crate::extensions::extension_ids_generated::HostExtensionId;

use super::state_backstop_service::{
    SandStateBackstop, StateBackstopOptions, StateBackstopSnapshotResult,
    is_state_backstop_enabled,
};

pub const STATE_BACKSTOP_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::BoxStoreSync,
    HostExtensionId::SourceMap,
];

pub fn state_backstop_extension_id() -> HostExtensionId {
    HostExtensionId::StateBackstop
}

pub enum HostStateBackstopExtension {
    Disabled,
    Enabled(Arc<SandStateBackstop>),
}

impl HostStateBackstopExtension {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    pub fn schedule_snapshot(&self, agent_id: impl Into<String>) {
        if let Self::Enabled(service) = self {
            service.schedule_snapshot(agent_id);
        }
    }

    pub fn snapshot_now(&self, agent_id: &str) -> StateBackstopSnapshotResult {
        match self {
            Self::Disabled => StateBackstopSnapshotResult::Skipped {
                reason: "backstop disabled".into(),
            },
            Self::Enabled(service) => service.snapshot_now(agent_id),
        }
    }

    pub fn read_snapshot(&self, agent_id: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Self::Disabled => Ok(None),
            Self::Enabled(service) => service.read_snapshot(agent_id),
        }
    }

    pub fn stop(&self) {
        if let Self::Enabled(service) = self {
            service.dispose();
        }
    }
}

pub fn start_state_backstop_extension(
    options: StateBackstopOptions,
) -> HostStateBackstopExtension {
    if !is_state_backstop_enabled() {
        return HostStateBackstopExtension::Disabled;
    }
    HostStateBackstopExtension::Enabled(SandStateBackstop::new(options))
}

pub fn start_state_backstop_extension_with_gate(
    enabled: bool,
    options: StateBackstopOptions,
) -> HostStateBackstopExtension {
    if !enabled {
        return HostStateBackstopExtension::Disabled;
    }
    HostStateBackstopExtension::Enabled(SandStateBackstop::new(options))
}
