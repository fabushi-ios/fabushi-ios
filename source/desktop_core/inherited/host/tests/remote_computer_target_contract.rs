const TARGET_SOURCE: &str = include_str!("../src/remote_computer_target.rs");

#[test]
fn target_core_client_keeps_controller_and_device_credentials_secret() {
    for required in [
        "FabushiCoreCredentialProvider",
        "RemoteComputerDeviceSecretProvider",
        ".bearer_auth(credential.access_token())",
        r#""deviceSecret": device_secret"#,
        r#".field("client_token", &"<redacted>")"#,
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing target credential invariant: {required}");
    }
    assert!(
        !TARGET_SOURCE.contains("#[derive(Debug, Clone, PartialEq, Eq)]\npub struct RemoteServiceClientGrant"),
        "service client grant must not derive Debug over clientToken"
    );
}

#[test]
fn target_service_grant_is_account_device_and_service_binding_correlated() {
    for required in [
        r#""sessions", "grant""#,
        r#""connectionId": connection_id"#,
        r#""delegationId": delegation_id"#,
        r#""requestId": request_id"#,
        r#"response.get("serviceBound").and_then(Value::as_bool) != Some(true)"#,
        r#"response.get("targetGranted").and_then(Value::as_bool) != Some(true)"#,
        r#"response.get("connectionId").and_then(Value::as_str) != Some(connection_id)"#,
        r#"response.get("delegationId").and_then(Value::as_str) != Some(delegation_id)"#,
        r#"response.get("requestId").and_then(Value::as_str) != Some(request_id)"#,
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing service grant correlation invariant: {required}");
    }
}

#[test]
fn target_operation_claim_and_result_keep_request_digest_and_generation_fences() {
    for required in [
        r#"response.get("requestId").and_then(Value::as_str) != Some(request_id)"#,
        r#"response.get("sessionId").and_then(Value::as_str) != Some(session_id)"#,
        r#"response.get("toolName").and_then(Value::as_str) != Some(expected_tool_name)"#,
        r#"response.get("requestSha256").and_then(Value::as_str) != Some(expected_request_sha256)"#,
        r#".get("grantGeneration")"#,
        r#"response.get("resultSha256").and_then(Value::as_str) != Some(result_sha256)"#,
        "if operation.sensitive_result && result.is_some()",
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing target operation invariant: {required}");
    }
}


#[test]
fn operation_digest_is_canonical_across_object_key_order() {
    use mahayana_host_runtime::remote_computer_target::operation_request_sha256;
    use serde_json::json;

    let left = operation_request_sha256(
        "computer_state",
        &json!({"z":1,"nested":{"b":2,"a":1},"list":[{"y":2,"x":1}]}),
        &json!({"impact":"none","reason":"no growth","targets":[]}),
    )
    .expect("canonical digest");
    let right = operation_request_sha256(
        "computer_state",
        &json!({"list":[{"x":1,"y":2}],"nested":{"a":1,"b":2},"z":1}),
        &json!({"targets":[],"reason":"no growth","impact":"none"}),
    )
    .expect("canonical digest");
    assert_eq!(left.len(), 64);
    assert_eq!(left, right);
    assert_ne!(
        left,
        operation_request_sha256(
            "computer_state",
            &json!({"list":[{"x":1,"y":2}],"nested":{"a":1,"b":3},"z":1}),
            &json!({"impact":"none","reason":"no growth","targets":[]}),
        )
        .expect("changed digest")
    );
}


#[test]
fn target_private_transport_is_peer_bound_generation_fenced_and_never_auto_replays() {
    for required in [
        "stream_listen(expected_peer, local_port, REMOTE_TARGET_STREAM_TIMEOUT_MS)",
        "RemoteTargetPrivateRequest::GrantController",
        "RemoteTargetPrivateRequest::AuthorizeTool",
        "RemoteTargetPrivateRequest::Operation",
        "operation_request_sha256(&tool_name, &arguments, &disk_preflight)",
        "claimed.client_id != client_id || claimed.grant_generation != grant_generation",
        "RemoteComputerNativeOutcome::OutcomeUnknown",
        "map_err(|_| RemoteComputerTargetError::OutcomeUnknown)",
        "binding.generation != expected_generation",
        "binding.expires_at <= unix_seconds()?",
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing private target invariant: {required}");
    }
    assert!(
        TARGET_SOURCE.matches("self.require_current_network_binding(&device_id, network_generation)?;").count() >= 4,
        "target transport must fence network binding both before authorization/claim and again before native side effects"
    );
    assert_eq!(
        TARGET_SOURCE.matches("executor.execute(&claimed, &arguments, &disk_preflight)").count(),
        1,
        "a claimed side-effecting operation must be dispatched to the native executor at most once"
    );
    assert!(
        !TARGET_SOURCE.contains("executor.execute(&claimed, &arguments, &disk_preflight).retry"),
        "native execution must never be automatically retried after transport uncertainty"
    );
}


#[test]
fn target_tool_description_is_service_bound_and_sourced_from_existing_host_bridge() {
    for required in [
        "pub struct RoutedRemoteComputerToolCatalog",
        "bridge: Arc<dyn RoutedToolBridge>",
        ".list_tools()",
        "RemoteTargetPrivateRequest::DescribeTool",
        "self.grant_service_client(&connection_id, &delegation_id, &request_id, &label)?",
        "catalog.describe_tool(&tool_name)?",
        r#""type": "toolDescription""#,
        r#""schemaVersion": schema_version"#,
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing target tool projection invariant: {required}");
    }
    assert!(
        !TARGET_SOURCE.contains(r#""clientToken": grant.client_token(),\n                        "type": "toolDescription""#),
        "tool description responses must not expose the service-bound controller credential"
    );
}


#[test]
fn remote_computer_removes_superseded_host_adapter_after_native_sensitive_result_wiring() {
    for required in [
        "pub struct NativeDesktopRemoteExecutor",
        "parse_remote_computer_action(arguments)",
        r#""sensitive_result_required""#,
        r#""screenshotTransferred": false"#,
        "RemoteComputerNativeOutcome::SensitiveCompleted",
    ] {
        assert!(TARGET_SOURCE.contains(required), "missing native Remote Computer invariant: {required}");
    }
    for forbidden in [
        "pub struct HostComputerRemoteExecutor",
        "computer: Arc<dyn ComputerToolExecutor>",
        "self.computer.execute(&action, &operation.request_id)",
        "sensitive_result_channel_unavailable",
    ] {
        assert!(
            !TARGET_SOURCE.contains(forbidden),
            "superseded Host Computer adapter invariant remains in the shipping target: {forbidden}"
        );
    }
}

#[test]
fn target_ingress_peer_is_core_projected_same_cell_and_exact_peer_only() {
    for required in [
        "pub struct RemoteNetworkIngress",
        r#"self.endpoint(&["v1", "computers", "network-ingress"])"#,
        r#""deviceId": credential.device_id"#,
        r#""deviceSecret": device_secret"#,
        r#""serviceId": service_id"#,
        r#"service_id != "official-device-mcp""#,
        r#"response.get("active").and_then(Value::as_bool) != Some(true)"#,
        "is_fabushi_tailnet_peer(&node_id)",
        "binding.cell_id != ingress.cell_id",
        "binding.generation != ingress.target_generation",
        "binding.expires_at <= now",
        "stream_listen(expected_peer, local_port, timeout_ms)",
        "|| !is_fabushi_tailnet_peer(expected_peer)",
    ] {
        assert!(
            TARGET_SOURCE.contains(required),
            "missing exact target ingress peer fence: {required}"
        );
    }
    assert!(
        !TARGET_SOURCE.contains(r#"stream_listen("*""#),
        "target listener must never accept a wildcard peer"
    );
}

#[test]
fn target_network_service_requires_explicit_authorizer_catalog_and_executor() {
    for required in [
        "pub struct RemoteComputerTargetNetworkService",
        "catalog: Arc<dyn RemoteComputerToolCatalog>",
        "authorizer: Arc<dyn RemoteComputerToolAuthorizer>",
        "executor: Arc<dyn RemoteComputerNativeExecutor>",
        "impl NetworkWorkerService for RemoteComputerTargetNetworkService",
        r#"self.client.network_ingress("official-device-mcp")"#,
        "REMOTE_TARGET_SERVICE_STREAM_TIMEOUT_MS",
        "self.authorizer.as_ref()",
        "self.executor.as_ref()",
    ] {
        assert!(
            TARGET_SOURCE.contains(required),
            "missing target network service dependency: {required}"
        );
    }
    assert!(
        !TARGET_SOURCE.contains("AllowAllRemoteComputerToolAuthorizer"),
        "private network availability must not synthesize blanket tool authorization"
    );
}


#[test]
fn native_snapshot_generation_is_projected_and_stale_fenced() {
    for required in [
        r#""snapshotGeneration": {"type": "integer", "minimum": 1}"#,
        "mahayana_computer::execute_with_lease_fenced(",
        "parsed.snapshot_generation",
        "ComputerError::SnapshotStale { expected, current }",
        r#""code":"snapshot_stale""#,
        r#""snapshotGeneration": observed.snapshot_generation"#,
        "pub snapshot_generation: u64",
        r#""snapshotGeneration": access.snapshot_generation"#,
        r#""snapshotGeneration": snapshot_generation"#,
    ] {
        assert!(
            TARGET_SOURCE.contains(required),
            "missing native snapshot generation/stale-fence invariant: {required}"
        );
    }
}
