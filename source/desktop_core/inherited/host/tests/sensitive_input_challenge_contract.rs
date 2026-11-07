use mahayana_host_runtime::sensitive_input_challenge::{
    SensitiveInputBinding, SensitiveInputChallengeError, SensitiveInputChallengeOwner,
    SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS,
};

fn binding(
    account: &str,
    device: &str,
    client: &str,
    session: &str,
    operation: &str,
    generation: i64,
) -> SensitiveInputBinding {
    SensitiveInputBinding {
        account_id: account.into(),
        device_id: device.into(),
        client_id: client.into(),
        session_id: session.into(),
        operation_id: operation.into(),
        request_sha256: "a".repeat(64),
        network_generation: 11,
        authorization_generation: generation,
    }
}

#[test]
fn challenge_is_short_lived_one_time_and_bound_to_full_authorization_context() {
    let owner = SensitiveInputChallengeOwner::default();
    let exact = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
    let challenge = owner
        .issue(exact.clone(), 1_000, 30_000)
        .expect("issue challenge");

    let delivered = owner
        .consume(
            &challenge.challenge_id,
            &exact,
            1_500,
            b"super-secret".to_vec(),
            |secret| secret.len(),
        )
        .expect("consume exact binding");
    assert_eq!(delivered, 12);

    assert_eq!(
        owner.consume(
            &challenge.challenge_id,
            &exact,
            1_600,
            b"replay".to_vec(),
            |_| (),
        ),
        Err(SensitiveInputChallengeError::UnknownChallenge)
    );
    assert_eq!(owner.active_count_for_tests().unwrap(), 0);
}

#[test]
fn wrong_device_session_operation_or_generation_consumes_and_invalidates_challenge() {
    let mut wrong_request = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
    wrong_request.request_sha256 = "b".repeat(64);
    let mut wrong_network = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
    wrong_network.network_generation = 12;
    for wrong in [
        binding("acct-b", "device-a", "client-a", "session-a", "op-a", 7),
        binding("acct-a", "device-b", "client-a", "session-a", "op-a", 7),
        binding("acct-a", "device-a", "client-b", "session-a", "op-a", 7),
        binding("acct-a", "device-a", "client-a", "session-b", "op-a", 7),
        binding("acct-a", "device-a", "client-a", "session-a", "op-b", 7),
        wrong_request,
        wrong_network,
        binding("acct-a", "device-a", "client-a", "session-a", "op-a", 8),
    ] {
        let owner = SensitiveInputChallengeOwner::default();
        let exact = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
        let challenge = owner.issue(exact.clone(), 10, 100).unwrap();
        assert_eq!(
            owner.consume(
                &challenge.challenge_id,
                &wrong,
                20,
                b"secret".to_vec(),
                |_| (),
            ),
            Err(SensitiveInputChallengeError::BindingMismatch)
        );
        assert_eq!(
            owner.consume(
                &challenge.challenge_id,
                &exact,
                21,
                b"secret".to_vec(),
                |_| (),
            ),
            Err(SensitiveInputChallengeError::UnknownChallenge)
        );
    }
}

#[test]
fn expired_or_revoked_challenges_fail_closed() {
    let owner = SensitiveInputChallengeOwner::default();
    let exact = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
    let challenge = owner.issue(exact.clone(), 100, 10).unwrap();
    assert_eq!(
        owner.consume(
            &challenge.challenge_id,
            &exact,
            110,
            b"secret".to_vec(),
            |_| (),
        ),
        Err(SensitiveInputChallengeError::Expired)
    );

    let challenge = owner.issue(exact.clone(), 200, 100).unwrap();
    assert_eq!(
        owner
            .revoke_authorization("acct-a", "device-a", "client-a", "session-a", 7)
            .unwrap(),
        1
    );
    assert_eq!(
        owner.consume(
            &challenge.challenge_id,
            &exact,
            210,
            b"secret".to_vec(),
            |_| (),
        ),
        Err(SensitiveInputChallengeError::UnknownChallenge)
    );
}

#[test]
fn challenge_rejects_invalid_ttl_binding_and_payload_without_exposing_secret() {
    let owner = SensitiveInputChallengeOwner::default();
    let exact = binding("acct-a", "device-a", "client-a", "session-a", "op-a", 7);
    assert_eq!(
        owner.issue(exact.clone(), 0, SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS + 1),
        Err(SensitiveInputChallengeError::InvalidTtl)
    );
    assert_eq!(
        owner.issue(binding("", "device-a", "client-a", "session-a", "op-a", 7), 0, 100),
        Err(SensitiveInputChallengeError::InvalidBinding)
    );

    let challenge = owner.issue(exact.clone(), 0, 100).unwrap();
    assert_eq!(
        owner.consume(&challenge.challenge_id, &exact, 1, Vec::new(), |_| ()),
        Err(SensitiveInputChallengeError::InvalidPayload)
    );

    let debug = format!("{challenge:?}");
    assert!(!debug.contains("super-secret"));
    assert!(!debug.contains("password"));
    assert!(!debug.contains("otp"));
}


const TARGET_SOURCE: &str = include_str!("../src/remote_computer_target.rs");

#[test]
fn target_host_sensitive_input_is_operation_bound_one_time_and_uses_existing_native_fences() {
    for required in [
        "RemoteTargetPrivateRequest::SensitiveInputIssue",
        "RemoteTargetPrivateRequest::SensitiveInputSubmit",
        "self.claim_operation(&session_id, &request_id, \"Computer\", &request_sha256)?",
        "request_sha256: request_sha256.clone()",
        "network_generation",
        "authorization_generation: grant_generation",
        "encoded.fill(0)",
        "sensitive_inputs: SensitiveInputChallengeOwner",
        ".consume(",
        "ComputerActionKind::Type",
        "mahayana_computer::execute_with_lease_fenced(",
        "Some(snapshot_generation)",
        "text.into_bytes()",
        "bytes.fill(0)",
        "\"sensitiveInputDelivered\": true",
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing sensitive-input Host invariant: {required}");
    }
    for forbidden in [
        "\"secret\":",
        "\"password\":",
        "\"otp\":",
        "\"payloadBase64\": payload_base64",
        "println!(",
        "eprintln!(",
    ] {
        assert!(!TARGET_SOURCE.contains(forbidden), "sensitive input must not enter ordinary logs/result fields: {forbidden}");
    }
}
