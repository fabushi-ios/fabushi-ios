use std::fs;
use std::path::PathBuf;

fn target_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/remote_computer_target.rs");
    fs::read_to_string(path).expect("read remote computer target source")
}

#[test]
fn network_enrollment_client_stays_inside_first_party_host_authority() {
    let source = target_source();
    for required in [
        "network-enrollment-grants",
        "api\", \"fdn\", \"network-enrollment\"",
        "deviceSecret",
        "grantSecret",
        "FabushiCoreCredentialProvider",
        "RemoteComputerDeviceSecretProvider",
        "bearer_auth(credential.access_token())",
    ] {
        assert!(source.contains(required), "missing enrollment authority contract: {required}");
    }
    for forbidden in [
        "registerDeviceTools",
        "CursorAuth",
        "SandInferenceCredential",
        "renderer",
    ] {
        assert!(!source.contains(forbidden), "forbidden enrollment authority dependency: {forbidden}");
    }
}

#[test]
fn enrollment_secrets_are_redacted_from_debug_contracts() {
    let source = target_source();
    assert!(source.contains(".field(\"grant_secret\", &\"<redacted>\")"));
    assert!(source.contains(".field(\"auth_key\", &\"<redacted>\")"));
    assert!(source.contains("pub fn grant_secret(&self) -> &str"));
    assert!(source.contains("pub fn auth_key(&self) -> &str"));
    assert!(!source.contains("pub grant_secret: String"));
    assert!(!source.contains("pub auth_key: String"));
}

#[test]
fn grant_and_broker_identity_fences_are_explicit() {
    let source = target_source();
    assert!(source.contains("response.get(\"requestId\")"));
    assert!(source.contains("Some(grant.grant_id.as_str())"));
    assert!(source.contains("Some(credential.device_id.as_str())"));
    assert!(source.contains("Some(grant.cell_id.as_str())"));
    assert!(source.contains("Some(started.cell_id.as_str())"));
    assert!(source.contains("Some(grant.generation)"));
    assert!(source.contains("Some(started.generation)"));
    assert!(source.contains("Some(true)"));
}
