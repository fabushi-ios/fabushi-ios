use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::settings::extension::{
    SETTINGS_EXTENSION_DEPENDENCIES, SETTINGS_EXTENSION_ID, settings_extension_id,
    start_settings_extension,
};
use mahayana_host_runtime::host_paths::get_sand_root_dir;

#[test]
fn settings_extension_matches_frozen_identity_dependencies_and_production_owner() {
    assert_eq!(SETTINGS_EXTENSION_ID, "settings");
    assert!(SETTINGS_EXTENSION_DEPENDENCIES.is_empty());
    assert_eq!(settings_extension_id(), HostExtensionId::Settings);

    let service = start_settings_extension();
    assert_eq!(
        service.get_settings_path(),
        get_sand_root_dir().join("settings.json")
    );
}
