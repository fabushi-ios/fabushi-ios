use std::sync::{Arc, mpsc::Sender};

use serde_json::Value;

use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::gateway_server::{GatewayBridgeClose, GatewayBridgeHub};
use crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use crate::extensions::telemetry::local_exec_telemetry::{
    LocalExecFailedReport, LocalExecProviderReport, LocalExecRefusalCause,
    LocalExecRefusedReport,
};
use crate::extensions::process::command_custody::CommandProcessCustody;

use super::gateway_local_exec_sand_box::{
    GatewayLocalExecFailureReport, GatewayLocalExecFailureReporter,
    GatewayLocalExecSandBox, GatewayLocalToolGate,
};
use super::local_exec_bridge::{
    LocalExecComputer, LocalExecProviderInfo, LocalExecProviderLifecycleReport,
    LocalExecProviderRegistration, LocalExecRefusalCause as BridgeRefusalCause,
    LocalExecRefusalReport as BridgeRefusalReport, SandLocalExecBridge,
};
use super::local_exec_failure_classifier::LocalExecFailureClass;

pub const LOCAL_EXEC_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::LocalToolPermission,
    HostExtensionId::Telemetry,
];

pub fn local_exec_extension_id() -> HostExtensionId {
    HostExtensionId::LocalExec
}

fn failure_class_name(error_class: LocalExecFailureClass) -> &'static str {
    match error_class {
        LocalExecFailureClass::Other => "other",
        LocalExecFailureClass::SpawnEnoent => "spawn_enoent",
        LocalExecFailureClass::SpawnPermissions => "spawn_permissions",
        LocalExecFailureClass::SpawnOther => "spawn_other",
    }
}

/// Host owner for the desktop local-exec provider transport.
///
/// The production Gateway provider channel is live and reports the frozen
/// provider lifecycle telemetry. Permission-authorized exec streaming and the
/// Runner's external-shell/file-transfer consumers remain separate mapped work.
#[derive(Clone)]
pub struct HostLocalExecExtension {
    bridge: SandLocalExecBridge,
    logs: HostStructuredLogTelemetry,
    process_custody: Arc<CommandProcessCustody>,
}

impl HostLocalExecExtension {
    pub fn register_provider(&self, send: Sender<Value>) -> LocalExecProviderRegistration {
        self.bridge.register_provider(send)
    }

    pub fn submit_responses(&self, batch: Value) {
        self.bridge.submit_responses(batch);
    }

    pub fn check_live_computer_for_ask(&self, agent_id: Option<&str>) -> bool {
        self.bridge.check_live_computer_for_ask_with_agent(agent_id)
    }

    pub fn list_computers(&self) -> Vec<LocalExecComputer> {
        self.bridge.list_computers()
    }

    pub fn active_computer(&self) -> Option<LocalExecComputer> {
        self.bridge.active_computer()
    }

    pub fn provider_info(&self) -> Option<LocalExecProviderInfo> {
        self.bridge.get_provider_info()
    }

    pub fn retire_approval(&self, approval_id: &str) {
        self.bridge.retire_approval(approval_id);
    }

    pub fn bridge(&self) -> SandLocalExecBridge {
        self.bridge.clone()
    }

    pub fn sandbox(
        &self,
        gate: Arc<dyn GatewayLocalToolGate>,
    ) -> GatewayLocalExecSandBox {
        let logs = self.logs.clone();
        let failure_reporter: GatewayLocalExecFailureReporter =
            Arc::new(move |report: GatewayLocalExecFailureReport| {
                let report = LocalExecFailedReport {
                    error_class: failure_class_name(report.error_class).to_string(),
                    errno: report.errno,
                    site: report.site,
                    conversation_id: report.conversation_id.unwrap_or_default(),
                };
                let _ = logs.report_local_exec_failed(&report);
            });
        GatewayLocalExecSandBox::new(self.bridge(), gate)
            .with_process_custody(Arc::clone(&self.process_custody))
            .with_failure_reporter(failure_reporter)
    }

    pub fn gateway_bridge(self: &Arc<Self>) -> GatewayBridgeHub {
        let registration_owner = Arc::clone(self);
        let response_owner = Arc::clone(self);
        GatewayBridgeHub::with_handlers(
            move |send| {
                let registration = registration_owner.register_provider(send);
                Box::new(move || drop(registration)) as GatewayBridgeClose
            },
            move |batch| response_owner.submit_responses(batch),
        )
    }
}

pub fn start_local_exec_extension(
    logs: HostStructuredLogTelemetry,
) -> HostLocalExecExtension {
    let provider_logs = logs.clone();
    let provider_reporter = Arc::new(move |report: LocalExecProviderLifecycleReport| {
        let telemetry = match report {
            LocalExecProviderLifecycleReport::Registered {
                provider_id,
                provider_count,
            } => LocalExecProviderReport::Registered {
                provider_id,
                provider_count: provider_count.try_into().unwrap_or(i64::MAX),
            },
            LocalExecProviderLifecycleReport::Hello {
                provider_id,
                provider_count,
                hello_delay_ms,
                computer_id_present,
                rehello,
                supervised,
                variant,
            } => LocalExecProviderReport::Hello {
                provider_id,
                provider_count: provider_count.try_into().unwrap_or(i64::MAX),
                hello_delay_ms: hello_delay_ms as f64,
                computer_id_present,
                rehello,
                supervised,
                variant,
            },
            LocalExecProviderLifecycleReport::Detached {
                provider_id,
                provider_count,
                age_ms,
                had_hello,
                has_heartbeat,
                was_live,
                emptied,
            } => LocalExecProviderReport::Detached {
                provider_id,
                provider_count: provider_count.try_into().unwrap_or(i64::MAX),
                age_ms: age_ms as f64,
                had_hello,
                has_heartbeat,
                was_live,
                emptied,
            },
        };
        let _ = provider_logs.report_local_exec_provider(&telemetry);
    });
    let refusal_logs = logs.clone();
    let refusal_reporter = Arc::new(move |report: BridgeRefusalReport| {
        let cause = match report.cause {
            BridgeRefusalCause::NoProviders => LocalExecRefusalCause::NoProviders,
            BridgeRefusalCause::ComputerUnknown => LocalExecRefusalCause::ComputerUnknown,
            BridgeRefusalCause::StaleHeartbeat => LocalExecRefusalCause::StaleHeartbeat,
        };
        let report = LocalExecRefusedReport {
            cause,
            site: report.site,
            conversation_id: report.conversation_id.unwrap_or_default(),
            provider_count: report.provider_count.try_into().unwrap_or(i64::MAX),
            live_provider_count: report.live_provider_count.try_into().unwrap_or(i64::MAX),
            ever_registered: report.ever_registered,
            empty_for_ms: report.empty_for_ms.map(|value| value as f64),
        };
        let _ = refusal_logs.report_local_exec_refused(&report);
    });
    HostLocalExecExtension {
        bridge: SandLocalExecBridge::production_with_reporters(
            Some(provider_reporter),
            Some(refusal_reporter),
        ),
        logs,
        process_custody: Arc::new(CommandProcessCustody::new()),
    }
}
