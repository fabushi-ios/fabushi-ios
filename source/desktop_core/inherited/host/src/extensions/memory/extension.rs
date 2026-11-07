use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::agent_state::SandAgentState;
use super::memory_service::{MemoryService, MemorySynthesisBridge};
use super::memory_synthesis_service::MemorySynthesisService;

pub const MEMORY_EXTENSION_ID: &str = "memory";
pub const MEMORY_EXTENSION_DEPENDENCIES: &[&str] = &["experiments", "inference", "telemetry"];

struct HostMemoryExtensionState {
    service: Arc<MemoryService>,
    agents_root_dir: PathBuf,
    synthesis: Mutex<Option<Arc<MemorySynthesisService>>>,
}

impl Drop for HostMemoryExtensionState {
    fn drop(&mut self) {
        self.service.dispose();
        if let Ok(slot) = self.synthesis.get_mut() {
            if let Some(synthesis) = slot.take() {
                synthesis.dispose();
            }
        }
    }
}

#[derive(Clone)]
pub struct HostMemoryExtension {
    state: Arc<HostMemoryExtensionState>,
}

impl HostMemoryExtension {
    pub fn service(&self) -> Arc<MemoryService> {
        Arc::clone(&self.state.service)
    }

    pub fn agents_root_dir(&self) -> &Path {
        &self.state.agents_root_dir
    }

    pub fn create_agent_state(&self, agent_id: &str) -> Result<SandAgentState, String> {
        SandAgentState::new(self.state.service.sand_root(), agent_id)
    }

    pub fn enable_memory_synthesis(&self, synthesis: Arc<MemorySynthesisService>) {
        let mut slot = self
            .state
            .synthesis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.state.service.clear_synthesis_bridge();
        if let Some(previous) = slot.take() {
            previous.dispose();
        }
        synthesis.start_background();
        let bridge: Arc<dyn MemorySynthesisBridge> = synthesis.clone();
        self.state.service.set_synthesis_bridge(Arc::downgrade(&bridge));
        *slot = Some(synthesis);
    }

    pub fn disable_memory_synthesis(&self) {
        let mut slot = self
            .state
            .synthesis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.state.service.clear_synthesis_bridge();
        if let Some(synthesis) = slot.take() {
            synthesis.dispose();
        }
    }

    pub fn memory_synthesis(&self) -> Option<Arc<MemorySynthesisService>> {
        self.state
            .synthesis
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

pub fn start_memory_extension(
    agents_root_dir: impl Into<PathBuf>,
) -> HostMemoryExtension {
    let agents_root_dir = agents_root_dir.into();
    HostMemoryExtension {
        state: Arc::new(HostMemoryExtensionState {
            service: Arc::new(MemoryService::new(agents_root_dir.clone())),
            agents_root_dir,
            synthesis: Mutex::new(None),
        }),
    }
}
