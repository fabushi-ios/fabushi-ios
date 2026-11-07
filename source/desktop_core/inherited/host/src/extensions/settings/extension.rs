use std::sync::Arc;

use crate::extensions::extension_ids_generated::HostExtensionId;

use super::settings_service::SettingsService;

pub const SETTINGS_EXTENSION_ID: &str = "settings";
pub const SETTINGS_EXTENSION_DEPENDENCIES: &[HostExtensionId] = &[];

pub fn settings_extension_id() -> HostExtensionId {
    HostExtensionId::Settings
}

pub fn start_settings_extension() -> Arc<SettingsService> {
    Arc::new(SettingsService::production())
}
