#[test]
fn shipping_mcp_discovery_failure_uses_dedicated_producer_coordinator_and_host_owner() {
    let discovery = include_str!("../../shared/node/mcp/tools-discovery.ts");
    assert!(discovery.contains("settleMcpDiscoverySuccess({ tools, resolvedKey })"));
    assert!(discovery.contains("settleMcpDiscoveryFailure("));
    assert!(discovery.contains("Date.now() - startedAtMs"));
    assert!(discovery.contains("deps.onDiscoveryFailed?.(settlement.report)"));
    assert!(discovery.contains("if (settlement.clearCache) toolsCacheEntry = null;"));

    let settlement = include_str!("../../shared/node/mcp/mcp-discovery-settlement.ts");
    assert!(settlement.contains("servedStale: staleTools !== undefined"));
    assert!(settlement.contains("clearCache: staleTools === undefined"));

    let desktop = include_str!("../../electron-main/mcp/desktop-mcp-manager.ts");
    assert!(desktop.contains("onDiscoveryFailed: options.onMcpDiscoveryFailed"));
    assert!(
        !desktop.contains("onDiscoveryFailed: options.onMcpDiagnostic"),
        "canonical discovery failure must not degrade to generic MCP diagnostics"
    );

    let runtime = include_str!("../../electron-main/mcp/mcp-runtime.ts");
    assert!(runtime.contains(
        "onMcpDiscoveryFailed: (report) => deps.reportMcpDiscoveryFailed(report)"
    ));

    let adapter = include_str!("../../electron-main/adapters/mcp-oauth.ts");
    assert!(adapter.contains("context.coordinatorLegs.legs.reportMcpDiscoveryFailed"));
    assert!(adapter.contains(
        "reportMcpDiscoveryFailed: (report) => reportMcpDiscoveryFailed(context, report)"
    ));

    let methods = include_str!("../../shared/rpc/coordinator-main.ts");
    assert!(methods.contains("reportMcpDiscoveryFailed: { args: \"object\" }"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(coordinator.contains("fn dispatch_to_host("));
    assert!(
        !coordinator.contains("sand.mcp.discovery_failed"),
        "Coordinator relays the typed request and must not become a second structured-log owner"
    );

    let shipping_host = include_str!("../app/src/main.rs");
    assert!(shipping_host.contains(
        "dispatch_mcp_discovery_failed_gateway(&self.telemetry_logs, method, &args)"
    ));
    assert!(
        !shipping_host.contains("event: Some(\"sand.mcp.discovery_failed\")"),
        "shipping app must delegate event ownership to HostStructuredLogTelemetry"
    );

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_mcp_discovery_failed("));
    assert!(owner.contains("mcp_discovery_failed_telemetry(report)"));

    let mapper = include_str!("../src/extensions/telemetry/mcp_discovery_telemetry.rs");
    assert_eq!(
        mapper.matches("sand.mcp.discovery_failed").count(),
        1,
        "canonical event literal must have one Host mapper owner"
    );
}
