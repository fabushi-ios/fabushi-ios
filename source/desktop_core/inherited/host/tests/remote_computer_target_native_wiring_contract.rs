const TARGET: &str = include_str!("../src/remote_computer_target.rs");
const MAIN: &str = include_str!("../app/src/main.rs");
const CARGO: &str = include_str!("../Cargo.toml");

#[test]
fn target_reuses_product_native_computer_owner_and_remote_policy() {
    for required in [
        "pub struct NativeDesktopRemoteToolCatalog",
        "pub struct NativeDesktopRemoteToolAuthorizer",
        "pub struct NativeDesktopRemoteExecutor",
        "mahayana_computer::control_policy().remote_control_enabled",
        "mahayana_computer::ComputerControlLeaseRequest::new(",
        "ComputerControlOrigin::RemoteMobile",
        "mahayana_computer::execute_with_lease_fenced(",
        "parsed.snapshot_generation",
        "Err(mahayana_computer::ComputerError::SnapshotStale { expected, current })",
        "\"code\":\"snapshot_stale\"",
        "\"expectedSnapshotGeneration\": expected",
        "\"currentSnapshotGeneration\": current",
        "REMOTE_NATIVE_TOOL_GRANT_MAX_SECONDS",
        "\"screenshotTransferred\": false",
        "\"screenshot\", \"click\"",
        "sensitive_result_required",
    ] {
        assert!(TARGET.contains(required), "missing native target invariant: {required}");
    }
    assert!(
        !TARGET.contains("mahayana_computer::execute_with_lease("),
        "remote Computer production target must keep snapshot-generation fencing and must not regress to the unfenced lease API"
    );
    assert!(TARGET.contains("tool_name != \"Computer\""));
    assert!(!TARGET.contains("AllowAllRemoteComputerToolAuthorizer"));
    assert!(!TARGET.contains("sensitive_result_channel_unavailable"));
    assert!(CARGO.contains("mahayana-computer = { path = \"../mahayana/mahayana-rs/mahayana-computer\" }"));
}

#[test]
fn sensitive_result_channel_is_bounded_one_time_and_identity_fenced() {
    for required in [
        "REMOTE_SENSITIVE_RESULT_MAX_BYTES",
        "REMOTE_SENSITIVE_RESULT_CHUNK_BYTES",
        "REMOTE_SENSITIVE_RESULT_TTL_SECONDS",
        "REMOTE_SENSITIVE_RESULT_MAX_ENTRIES",
        "SensitiveResultResume",
        "SensitiveResultChunk",
        "SensitiveResultRevoke",
        "validate_sensitive_result_access(",
        "validate_sensitive_entry(",
        "entry.account_user_id == credential.user_id",
        "entry.device_id == credential.device_id",
        "entry.network_generation == network_generation",
        "entry.session_id == session_id",
        "entry.client_id == client_id",
        "entry.request_id == request_id",
        "entry.request_sha256 == request_sha256",
        "entry.grant_generation == grant_generation",
        "self.require_current_network_binding(device_id, network_generation)?",
        "response.get(\"resultSha256\")",
        "current.next_offset != offset",
        "vault.entries.remove(result_ref)",
        "Uuid::new_v4()",
    ] {
        assert!(TARGET.contains(required), "missing sensitive-result invariant: {required}");
    }
    for forbidden in [
        "\"screenshotData\"",
        "\"screenshotBase64\"",
        "println!(",
        "eprintln!(",
    ] {
        assert!(!TARGET.contains(forbidden), "sensitive screenshot must not enter ordinary logs/result fields: {forbidden}");
    }
}

#[test]
fn sensitive_screenshot_uses_native_capture_and_never_persists_plaintext_to_core() {
    for required in [
        "RemoteComputerNativeOutcome::SensitiveCompleted",
        "decode_sensitive_snapshot(",
        "image/jpeg",
        "image/png",
        "image/webp",
        "self.publish_result(claimed, \"completed\", &result_sha256, None)",
        "if operation.sensitive_result && result.is_some()",
        "\"sensitiveResult\": true",
        "\"resultRef\": result_ref",
    ] {
        assert!(TARGET.contains(required), "missing sensitive screenshot invariant: {required}");
    }
    assert!(!TARGET.contains("publish_result(claimed, \"completed\", &result_sha256, Some("));
}

#[test]
fn remote_target_advertises_double_click_and_maps_it_to_the_existing_native_click_executor() {
    for required in [
        "\"doubleClick\"",
        "ComputerActionName::DoubleClick => ComputerActionKind::Click",
        "args.action == ComputerActionName::DoubleClick",
        "Some(2)",
        "doubleClick has a fixed click count of 2",
    ] {
        assert!(TARGET.contains(required), "missing Double Click target invariant: {required}");
    }
}

#[test]
fn shipping_host_attaches_target_service_to_same_network_owner() {
    for required in [
        "production_remote_computer_target_service(",
        "NetworkEnrollmentSupervisor::start_with_service(owner, target_service)",
        "FABUSHI_API_BASE_URL_ENV",
        "DEFAULT_FABUSHI_API_BASE_URL",
    ] {
        assert!(MAIN.contains(required), "missing shipping target wiring: {required}");
    }
}
