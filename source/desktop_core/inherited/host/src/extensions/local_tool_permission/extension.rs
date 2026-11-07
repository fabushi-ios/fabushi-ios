use std::sync::{Arc, Mutex};

use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::settings::settings_service::SettingsService;
use crate::extensions::transcript::widget_responses::WidgetResponses;

use super::local_tool_permission_controller::{
    SandLocalToolAskRequest, SandLocalToolControllerSubscription, SandLocalToolDecision,
    SandLocalToolPermissionController, SandLocalToolRequest, SandLocalToolScope,
};
use super::local_tool_permission_resolution::{
    LocalToolPermissionResolutionArgs, SandLocalToolPermissionResolutionError,
    resolve_local_tool_permission_ask,
};

pub const LOCAL_TOOL_PERMISSION_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::Settings,
    HostExtensionId::Telemetry,
    HostExtensionId::Transcript,
];

pub fn local_tool_permission_extension_id() -> HostExtensionId {
    HostExtensionId::LocalToolPermission
}

#[derive(Clone)]
struct TranscriptBinding {
    widget_responses: Arc<WidgetResponses>,
    boot_sweep_started_at_ms: u64,
    on_stranded_retirement: Option<Arc<dyn Fn() + Send + Sync>>,
    log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

#[derive(Clone)]
pub struct HostLocalToolPermissionExtension {
    controller: Arc<SandLocalToolPermissionController>,
    transcript: Arc<Mutex<Option<TranscriptBinding>>>,
}

impl HostLocalToolPermissionExtension {
    pub fn controller(&self) -> Arc<SandLocalToolPermissionController> {
        Arc::clone(&self.controller)
    }

    pub fn blocked_reason(&self) -> Option<String> {
        self.controller.blocked_reason()
    }

    pub fn requires_approval(&self) -> bool {
        self.controller.requires_approval()
    }

    pub fn authorize(
        &self,
        scope: Option<&SandLocalToolScope>,
        request: &SandLocalToolRequest,
    ) -> SandLocalToolDecision {
        self.controller.authorize(scope, request)
    }

    pub fn bind_ask_surfaces(
        &self,
        provider: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        self.controller.bind_ask_surfaces(provider);
    }

    pub fn bind_live_computer_check(
        &self,
        provider: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) {
        self.controller.bind_live_computer_check(provider);
    }

    pub fn bind_event_sink(
        &self,
        sink: Option<Arc<dyn Fn(SandLocalToolAskRequest) + Send + Sync>>,
    ) {
        self.controller.bind_event_sink(sink);
    }

    pub fn bind_approval_retired_sink(
        &self,
        sink: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    ) {
        self.controller.bind_approval_retired_sink(sink);
    }

    pub fn subscribe(
        &self,
        listener: Arc<
            dyn Fn(super::local_tool_permission_controller::SandLocalToolControllerEvent)
                + Send
                + Sync,
        >,
    ) -> SandLocalToolControllerSubscription {
        self.controller.subscribe(listener)
    }

    pub fn bind_transcript(
        &self,
        widget_responses: Arc<WidgetResponses>,
        boot_sweep_started_at_ms: u64,
        on_stranded_retirement: Option<Arc<dyn Fn() + Send + Sync>>,
        log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    ) {
        *self
            .transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(TranscriptBinding {
            widget_responses,
            boot_sweep_started_at_ms,
            on_stranded_retirement,
            log,
        });
    }

    pub fn background_work_ready(&self) -> usize {
        let binding = self
            .transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(binding) = binding else {
            return 0;
        };
        binding
            .widget_responses
            .expire_all_pending_local_tool_permission_cards(Some(
                binding.boot_sweep_started_at_ms,
            ))
    }

    pub fn resolve_ask(
        &self,
        args: &LocalToolPermissionResolutionArgs,
    ) -> Result<(), SandLocalToolPermissionResolutionError> {
        let binding = self
            .transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| SandLocalToolPermissionResolutionError::Transcript(
                "local-tool permission transcript is not bound".into(),
            ))?;
        let stranded = binding
            .on_stranded_retirement
            .as_deref()
            .map(|callback| callback as &dyn Fn());
        resolve_local_tool_permission_ask(
            self.controller.as_ref(),
            binding.widget_responses.as_ref(),
            args,
            stranded,
        )
    }

    pub fn note_permission_changed(&self) {
        self.controller.note_permission_changed();
    }

    pub fn forget_agent(&self, agent_id: &str) {
        self.controller.forget_agent(agent_id);
    }

    pub fn log_boot_sweep_failure(&self, message: &str) {
        let binding = self
            .transcript
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(log) = binding.and_then(|binding| binding.log) {
            log(message);
        }
    }
}

pub fn start_local_tool_permission_extension(
    settings: Arc<SettingsService>,
) -> HostLocalToolPermissionExtension {
    HostLocalToolPermissionExtension {
        controller: Arc::new(SandLocalToolPermissionController::production(settings)),
        transcript: Arc::new(Mutex::new(None)),
    }
}
