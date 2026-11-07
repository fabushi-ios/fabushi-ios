#[test]
fn shipping_connector_auth_uses_real_producers_coordinator_and_unique_host_owner() {
    let auth_watch = include_str!("../../shared/node/mcp/mcp-auth-watch-lifecycle.ts");
    for needle in [
        "phase: \"card_clicked\"",
        "phase: \"flow_started\"",
        "SandError.connectorAuthStartRefused",
        "SandError.connectorAuthStartFailed",
        "reauth: forceReauth",
        "phase: \"token_stored\"",
        "outcome: \"timeout\"",
        "SandError.connectorAuthAbandoned",
        "outcome: \"cancelled\"",
        "outcome: \"ok\"",
    ] {
        assert!(auth_watch.contains(needle), "missing shipping auth-watch producer: {needle}");
    }

    let loopback = include_str!("../../shared/node/mcp/mcp-oauth-loopback.ts");
    for needle in [
        "phase: \"callback_received\"",
        "SandError.connectorOauthCallbackFailed",
        "\"provider_error\"",
        "\"missing_code\"",
        "\"completion_rejected\"",
        "\"completion_timeout\"",
        "outcome: \"ok\"",
    ] {
        assert!(loopback.contains(needle), "missing shipping loopback producer: {needle}");
    }

    let desktop = include_str!("../../electron-main/mcp/desktop-mcp-manager.ts");
    assert!(desktop.contains("onConnectorAuth: options.onConnectorAuth"));
    let runtime = include_str!("../../electron-main/mcp/mcp-runtime.ts");
    assert!(runtime.contains("onConnectorAuth: (report) => deps.reportConnectorAuth(report)"));

    let adapter = include_str!("../../electron-main/adapters/mcp-oauth.ts");
    assert!(adapter.contains("context.readTelemetry()?.telemetry"));
    assert!(adapter.contains("context.coordinatorLegs.legs.reportConnectorAuth"));
    assert!(adapter.contains("reportConnectorAuth: (report) => reportConnectorAuth(context, report)"));
    assert!(!adapter.contains("reportConnectorAuth: (report) => reportMcpDiagnostic"));

    let desktop_telemetry = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(desktop_telemetry.contains("surface: \"desktop\""));

    let methods = include_str!("../../shared/rpc/coordinator-main.ts");
    assert!(methods.contains("reportConnectorAuth: { args: \"object\" }"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(coordinator.contains("fn dispatch_to_host("));
    assert!(!coordinator.contains("sand.connector_auth"));

    let shipping_host = include_str!("../app/src/main.rs");
    assert!(shipping_host.contains("dispatch_connector_auth_gateway(&self.telemetry_logs, method, &args)"));
    assert!(!shipping_host.contains("event: Some(\"sand.connector_auth\")"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_connector_auth("));
    assert!(owner.contains("connector_auth_telemetry(report, \"host\")"));

    let mapper = include_str!("../src/extensions/telemetry/connector_auth_telemetry.rs");
    assert_eq!(mapper.matches("sand.connector_auth").count(), 1);
}
