#[test]
fn shipping_runner_routes_started_error_and_stalled_to_unique_host_owner() {
    const HOST: &str = include_str!("../app/src/main.rs");
    const OBSERVATION: &str = include_str!("../src/runner/turn_observation.rs");
    const OWNER: &str = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    const COORDINATOR: &str = include_str!("../../node-agent-coordinator/src/main.rs");
    const ELECTRON: &str = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");

    assert!(HOST.contains("observation.set_tool_call_telemetry_handler(Arc::new(move |event|"));
    assert!(HOST.contains("tool_call_logs.report_tool_call_started("));
    assert!(HOST.contains("tool_call_logs.report_tool_call_error("));
    assert!(HOST.contains("tool_call_logs.report_tool_call_stalled("));
    assert!(OBSERVATION.contains("ToolCallTelemetryEvent::Started {"));
    assert!(OBSERVATION.contains("ToolCallTelemetryEvent::Error {"));
    assert!(OBSERVATION.contains("ToolCallTelemetryEvent::Stalled {"));
    assert!(OBSERVATION.contains("begin_mcp_exec_observation_with_threshold("));

    for method in [
        "pub fn report_tool_call_started(",
        "pub fn report_tool_call_error(",
        "pub fn report_tool_call_stalled(",
    ] {
        assert!(OWNER.contains(method), "{method}");
    }
    for event in [
        "sand.tool_call.started",
        "sand.tool_call.error",
        "sand.tool_call.stalled",
    ] {
        assert!(!COORDINATOR.contains(event), "{event}");
        assert!(!ELECTRON.contains(event), "{event}");
    }
}
