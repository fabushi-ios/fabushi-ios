use std::path::{Path, PathBuf};

use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_capability::CsnapsUnavailableReason;
use mahayana_host_runtime::extensions::codebase_telemetry::extension::{
    CODEBASE_TELEMETRY_DEPENDENCIES, CodebaseTelemetryExtension,
    build_upload_credentials, codebase_telemetry_extension_id,
    codebase_telemetry_paths,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

#[test]
fn extension_keeps_the_frozen_auth_and_experiments_dependencies() {
    assert_eq!(codebase_telemetry_extension_id(), HostExtensionId::CodebaseTelemetry);
    assert_eq!(
        CODEBASE_TELEMETRY_DEPENDENCIES,
        &[HostExtensionId::Auth, HostExtensionId::Experiments]
    );
}

#[test]
fn production_paths_match_the_frozen_extension_contract() {
    let paths = codebase_telemetry_paths(Path::new("/sand-root"));
    assert_eq!(
        paths.codebase_uuid_state_path,
        PathBuf::from("/sand-root/telemetry/codebase-uuids.json")
    );
    assert_eq!(
        paths.snapshots_base_dir,
        PathBuf::from("/var/lib/sand/telemetry/codebase")
    );
}

#[test]
fn upload_credentials_include_checksum_client_metadata_and_privacy_header() {
    let credentials = build_upload_credentials("token", "machine", 1_000_000);
    assert_eq!(credentials["authToken"], "token");
    let headers = credentials["requestHeaders"].as_object().unwrap();
    assert!(headers["x-cursor-checksum"].as_str().unwrap().ends_with("machine"));
    assert_eq!(headers["x-ghost-mode"], "false");
    assert!(!headers["x-cursor-client-type"].as_str().unwrap().is_empty());
    assert!(!headers["x-cursor-client-version"].as_str().unwrap().is_empty());
    assert!(!headers["x-sand-box-namespace"].as_str().unwrap().is_empty());
}

#[test]
fn unavailable_csnaps_degrades_to_noop_extension_api() {
    let extension = CodebaseTelemetryExtension::disabled(
        PathBuf::from("/missing/csnaps"),
        CsnapsUnavailableReason::Missing,
    );
    assert!(!extension.available());
    let (path, reason) = extension.unavailable_reason().unwrap();
    assert_eq!(path, Path::new("/missing/csnaps"));
    assert_eq!(reason, CsnapsUnavailableReason::Missing);
    extension.flush_pending_uploads().unwrap();
}
