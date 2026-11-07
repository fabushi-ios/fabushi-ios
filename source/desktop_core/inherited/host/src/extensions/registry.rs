use std::collections::BTreeMap;

use super::extension_ids_generated::HostExtensionId;

pub const HOST_EXTENSION_ORDER: &[HostExtensionId] = &[
    HostExtensionId::Notifications, HostExtensionId::ContentSearch, HostExtensionId::Memory,
    HostExtensionId::CrossUserSharing, HostExtensionId::StateBackstop, HostExtensionId::SourceMap,
    HostExtensionId::Telemetry, HostExtensionId::Trays, HostExtensionId::Auth, HostExtensionId::Experiments,
    HostExtensionId::BrowserUa, HostExtensionId::Inference, HostExtensionId::LocalExec,
    HostExtensionId::LocalToolPermission, HostExtensionId::Attachments, HostExtensionId::ForeverBox,
    HostExtensionId::Secrets, HostExtensionId::TurnExecution, HostExtensionId::Transcript,
    HostExtensionId::Session, HostExtensionId::Automations, HostExtensionId::Settings,
    HostExtensionId::BoxLifecycle, HostExtensionId::ManagedSetup, HostExtensionId::Mcp,
    HostExtensionId::BoxStoreSync, HostExtensionId::CloudAgents, HostExtensionId::CloudWorkers,
    HostExtensionId::ActionAudit,
    HostExtensionId::HostUpgrade, HostExtensionId::AutoReview, HostExtensionId::CodebaseTelemetry,
    HostExtensionId::TeachRecording, HostExtensionId::WebauthnProxy, HostExtensionId::NotifyBus,
    HostExtensionId::Wallpaper,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExtensionDeclaration<T> {
    pub id: HostExtensionId,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostExtensionRegistryError {
    #[error("host extension registry is missing \"{0}\"")]
    Missing(&'static str),
    #[error("host extension registry slot \"{slot}\" received \"{received}\"")]
    Mismatched { slot: &'static str, received: &'static str },
}

pub fn assemble_host_extension_registry<T>(
    mut by_id: BTreeMap<HostExtensionId, HostExtensionDeclaration<T>>,
) -> Result<Vec<HostExtensionDeclaration<T>>, HostExtensionRegistryError> {
    let mut ordered = Vec::with_capacity(HOST_EXTENSION_ORDER.len());
    for id in HOST_EXTENSION_ORDER {
        let extension = by_id
            .remove(id)
            .ok_or_else(|| HostExtensionRegistryError::Missing(id.as_str()))?;
        if extension.id != *id {
            return Err(HostExtensionRegistryError::Mismatched {
                slot: id.as_str(),
                received: extension.id.as_str(),
            });
        }
        ordered.push(extension);
    }
    Ok(ordered)
}
