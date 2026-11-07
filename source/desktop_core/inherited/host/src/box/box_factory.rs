use super::loopback_sand_box::{LoopbackReady, LoopbackSandBox, LoopbackSandBoxError, LoopbackSandBoxOptions};
use super::shared_desktop_sand_box::SharedDesktopSandBox;
use super::box_env::BoxEnvironmentUpdate;
use super::generated_production::ProductionBoxResourceAccessor;

pub fn create_sand_box(options: LoopbackSandBoxOptions) -> LoopbackSandBox {
    LoopbackSandBox::new(options)
}

#[derive(Debug, Clone)]
pub enum SandBoxComposition {
    Loopback(LoopbackSandBox),
    SharedDesktop(SharedDesktopSandBox),
}

impl SandBoxComposition {
    pub fn loopback(&self) -> &LoopbackSandBox {
        match self {
            Self::Loopback(inner) => inner,
            Self::SharedDesktop(shared) => shared.inner(),
        }
    }

    pub fn shared_desktop(&self) -> Option<&SharedDesktopSandBox> {
        match self {
            Self::Loopback(_) => None,
            Self::SharedDesktop(shared) => Some(shared),
        }
    }

    pub fn ensure_ready<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
    ) -> Result<LoopbackReady, LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.ensure_ready(ctx, agent_id).map(|ready| LoopbackReady {
                remote_accessor: ready.remote_accessor.with_no_monitor_computer_use(),
                vnc_url: ready.vnc_url,
                terminals_folder: ready.terminals_folder,
            }),
            Self::SharedDesktop(shared) => shared.ensure_ready(ctx, agent_id),
        }
    }

    pub fn apply_environment<Ctx>(
        &self,
        ctx: &Ctx,
        update: &BoxEnvironmentUpdate,
    ) -> Result<(), LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.apply_environment(ctx, update),
            Self::SharedDesktop(shared) => shared.apply_environment(ctx, update),
        }
    }

    pub fn load_mcp_servers<Ctx>(
        &self,
        ctx: &Ctx,
        config_json: &str,
    ) -> Result<Vec<String>, LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.load_mcp_servers(ctx, config_json),
            Self::SharedDesktop(shared) => shared.load_mcp_servers(ctx, config_json),
        }
    }

    pub fn mcp_resource_accessor<Ctx>(
        &self,
        ctx: &Ctx,
    ) -> Result<ProductionBoxResourceAccessor, LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.mcp_resource_accessor(ctx),
            Self::SharedDesktop(shared) => shared.mcp_resource_accessor(ctx),
        }
    }
    pub fn release_window<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
    ) -> Result<(), LoopbackSandBoxError> {
        match self {
            Self::Loopback(_) => Ok(()),
            Self::SharedDesktop(shared) => shared.release_window(ctx, agent_id),
        }
    }

    pub fn run_state(&self) -> &'static str {
        match self {
            Self::Loopback(inner) => inner.run_state(),
            Self::SharedDesktop(shared) => shared.run_state(),
        }
    }

    pub fn list_boxes(&self) -> Vec<(String, bool)> {
        match self {
            Self::Loopback(inner) => inner.list_boxes(),
            Self::SharedDesktop(shared) => shared.list_boxes(),
        }
    }

    pub fn get_agent_window_index(&self, agent_id: &str) -> Option<u32> {
        match self {
            Self::Loopback(_) => None,
            Self::SharedDesktop(shared) => shared.get_agent_window_index(agent_id),
        }
    }

    pub fn terminals_folder(&self) -> &'static str {
        self.loopback().terminals_folder()
    }

    pub fn is_available(&self) -> bool {
        self.loopback().is_available()
    }

    pub fn upload_file<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.upload_file(ctx, agent_id, path, data),
            Self::SharedDesktop(shared) => shared.upload_file(ctx, agent_id, path, data),
        }
    }

    pub fn download_file<Ctx>(
        &self,
        ctx: &Ctx,
        agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, LoopbackSandBoxError> {
        match self {
            Self::Loopback(inner) => inner.download_file(ctx, agent_id, path),
            Self::SharedDesktop(shared) => shared.download_file(ctx, agent_id, path),
        }
    }
}

pub fn apply_shared_desktop(
    box_: LoopbackSandBox,
    enabled: bool,
    persist_assignments: bool,
) -> SandBoxComposition {
    if enabled && should_apply_shared_desktop(box_.max_windows()) {
        SandBoxComposition::SharedDesktop(SharedDesktopSandBox::new(
            box_,
            None,
            persist_assignments,
        ))
    } else {
        SandBoxComposition::Loopback(box_)
    }
}

pub fn format_sand_box_startup_summary(auto_update_enabled: bool, is_packaged: bool) -> String {
    format!(
        "[sand-host] agent box backend: loopback (in-box); image: host's own container; auto-update: {}; build: {}",
        if auto_update_enabled { "on" } else { "off" },
        if is_packaged { "packaged" } else { "dev" },
    )
}

pub fn should_apply_shared_desktop(max_windows: u32) -> bool {
    max_windows > 1
}
