use super::turn_execution_service::TurnExecutionRegistry;
use crate::extensions::extension_ids_generated::HostExtensionId;

pub fn turn_execution_extension() -> (HostExtensionId, TurnExecutionRegistry) {
    (HostExtensionId::TurnExecution, TurnExecutionRegistry::default())
}
