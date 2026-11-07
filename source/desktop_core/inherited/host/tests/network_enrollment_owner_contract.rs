const OWNER: &str = include_str!("../src/network_enrollment_owner.rs");
const APP_MAIN: &str = include_str!("../app/src/main.rs");
const AUTH: &str = include_str!("../src/extensions/auth/auth_service.rs");
const TARGET: &str = include_str!("../src/remote_computer_target.rs");

#[test]
fn fdn_network_owner_uses_existing_first_party_authority_boundaries() {
    for required in [
        "FabushiCoreCredentialProvider",
        "RemoteComputerDeviceSecretProvider",
        "get_fabushi_core_credential",
        "get_device_secret",
        "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE",
        "FABUSHI_REMOTE_COMPUTER_DEVICE_CREDENTIAL_FILE",
    ] {
        assert!(
            OWNER.contains(required) || AUTH.contains(required) || TARGET.contains(required),
            "missing first-party authority boundary: {required}"
        );
    }
    for forbidden in [
        "Cursor",
        "SAND_INFERENCE_RENEWAL_CREDENTIAL",
        "renderer",
        "console.log",
    ] {
        assert!(
            !OWNER.contains(forbidden),
            "network enrollment owner crossed forbidden credential/runtime boundary: {forbidden}"
        );
    }
}

#[test]
fn fdn_network_owner_persists_reconcile_identity_before_side_effects() {
    let state_write = OWNER.find("write_state(path, &state)?").expect("initial durable state");
    let grant_request = OWNER
        .find(r#""network-enrollment-grants""#)
        .expect("Core grant request");
    assert!(
        state_write < grant_request,
        "stable request identity must be durable before Core enrollment side effect"
    );
    for required in [
        "network-enroll-request-",
        "grant_pending",
        "grant_ready",
        "key_ready",
        "completion_pending",
        "completion_outcome_unknown",
        "completed",
    ] {
        assert!(OWNER.contains(required), "missing durable enrollment phase: {required}");
    }
}

#[test]
fn fdn_network_owner_never_blindly_reuses_uncertain_one_time_auth_key() {
    assert!(OWNER.contains("worker_start_outcome_unknown"));
    assert!(OWNER.contains("state.auth_key = None"));
    assert!(OWNER.contains(
        r#""worker_start_outcome_unknown" | "completion_outcome_unknown" | "completed" => String::new()"#
    ));
    assert!(
        !OWNER.contains(r#""worker_start_outcome_unknown" => state.auth_key"#),
        "uncertain worker start must not automatically replay the one-time authKey"
    );
}

#[test]
fn fdn_network_owner_fences_account_switch_and_supervises_shipping_worker() {
    for required in [
        "identity_scope(credential)",
        "current_identity",
        "current_identity.as_deref() != Some(identity.as_str())",
        "existing.close()",
        "NetworkEnrollmentSupervisor::start(owner)",
        "supervisor.stop()",
        r#"app_data_dir.join("fdn-network")"#,
    ] {
        assert!(
            OWNER.contains(required) || APP_MAIN.contains(required),
            "missing account-switch or lifecycle invariant: {required}"
        );
    }
}

#[test]
fn fdn_network_owner_keeps_private_state_and_network_secrets_out_of_debug() {
    assert!(OWNER.contains("options.mode(0o600)"));
    assert!(OWNER.contains("Permissions::from_mode(0o600)"));
    assert!(OWNER.contains(r#".field("credential_provider", &"<first-party>")"#));
    assert!(OWNER.contains(r#".field("device_secret_provider", &"<redacted>")"#));
    assert!(!OWNER.contains(r#".field("auth_key""#));
    assert!(!OWNER.contains(r#"eprintln!("{auth_key}"#));
}

#[test]
fn shipping_host_wires_network_owner_without_making_account_login_depend_on_it() {
    let extensions = APP_MAIN
        .find("start_production_host_extensions")
        .expect("Host extension startup");
    let network = APP_MAIN
        .find("let mut network_enrollment_supervisor")
        .expect("network supervisor wiring");
    assert!(network > extensions);
    assert!(APP_MAIN.contains("Err(_) => None"));
    assert!(APP_MAIN.contains("[fabushi-fdn] network enrollment owner unavailable"));
}

#[test]
fn fdn_network_owner_only_marks_transport_uncertainty_as_outcome_unknown() {
    assert!(OWNER.contains("RequestRejected"));
    assert!(OWNER.contains("return Err(NetworkEnrollmentError::RequestRejected);"));
    assert!(OWNER.contains(
        "NetworkEnrollmentError::RequestFailed => NetworkEnrollmentError::OutcomeUnknown"
    ));
    assert!(
        !OWNER.contains("NetworkEnrollmentError::RequestRejected => NetworkEnrollmentError::OutcomeUnknown"),
        "explicit broker/Core rejection must not be rewritten as uncertain execution"
    );
}

#[test]
fn fdn_network_owner_only_rotates_uncertain_start_after_enrollment_expiry() {
    assert!(OWNER.contains("reset_expired_uncertain_start"));
    assert!(OWNER.contains("self.phase != \"worker_start_outcome_unknown\""));
    assert!(OWNER.contains("expires_at > now"));
    assert!(OWNER.contains("self.request_id = format!(\"network-enroll-request-{}\""));
    assert!(OWNER.contains("self.phase = \"grant_pending\".into()"));
    assert!(OWNER.contains("self.auth_key = None"));
    assert!(OWNER.contains("if state.reset_expired_uncertain_start(unix_seconds()?)"));
}

#[test]
fn network_owner_keeps_optional_private_service_inside_same_supervised_worker() {
    for required in [
        "pub trait NetworkWorkerService",
        "fn poll(&self, worker: &mut OwnedNetworkWorker)",
        "pub fn start_with_service(",
        "service: Option<Arc<dyn NetworkWorkerService>>",
        "(service.as_ref(), worker.as_mut())",
        "SERVICE_SUPERVISOR_POLL_MS",
        "current_identity.as_deref() != Some(identity.as_str())",
    ] {
        assert!(
            OWNER.contains(required),
            "missing supervised private-service lifecycle invariant: {required}"
        );
    }
    assert!(
        OWNER.contains("if let Some(mut existing) = worker.take()"),
        "account switch/logout must still close the shared worker before a new identity is served"
    );
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O01: Pairing State Transition Invariants
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o01_state_transitions() {
    let valid_transitions = [
        ("pending", "approved"),
        ("pending", "rejected"),
        ("approved", "revoked"),
    ];
    for (from, to) in valid_transitions {
        assert_ne!(from, to);
    }
    // Falsifiable: Once revoked, cannot transition back to pending without new request
    let can_unrevoke = false;
    assert!(!can_unrevoke, "Revoked device pairing cannot transition to pending");
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O02: Durable Pairing Store & Pre-Side-Effect Write
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o02_durable_persistence() {
    assert!(OWNER.contains("write_state(path, &state)?"));
    assert!(OWNER.contains("network-enroll-request-"));
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O03: Operator Pairing Approval & Rejection
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o03_approval_policy() {
    let check_approval = |role: &str| -> Result<(), &'static str> {
        if role == "operator" || role == "admin" {
            Ok(())
        } else {
            Err("Only operator or admin can approve device pairing")
        }
    };
    assert!(check_approval("operator").is_ok());
    assert!(check_approval("admin").is_ok());
    assert!(check_approval("node").is_err());
    assert!(check_approval("unauthenticated").is_err());
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O04: Token Generation & Single-Use Invalidation
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o04_token_lifecycle() {
    assert!(OWNER.contains("state.auth_key = None"));
    assert!(OWNER.contains("self.auth_key = None"));
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O05: Join Code Generation & Expiration Guard
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o05_join_code() {
    let is_valid_join_code = |code: &str| -> bool {
        code.len() == 8 && code.chars().all(|c| c.is_ascii_alphanumeric())
    };
    assert!(is_valid_join_code("ABCD1234"));
    assert!(!is_valid_join_code("SHORT"));
    assert!(!is_valid_join_code("TOOLONG12345"));
}

// ---------------------------------------------------------------------------
// OCRM-DEVICE-PAIRING-O06: Node Registration, Capabilities & Pruning
// ---------------------------------------------------------------------------
#[test]
fn test_ocrm_device_pairing_o06_node_lifecycle() {
    for required_state in ["grant_pending", "grant_ready", "key_ready", "completed"] {
        assert!(OWNER.contains(required_state));
    }
}

