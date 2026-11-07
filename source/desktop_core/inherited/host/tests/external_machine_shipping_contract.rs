const EXTERNAL_MACHINE_EXECUTOR: &str =
    include_str!("../src/runner/host_external_machine_dependencies.rs");
const EXTERNAL_MACHINE_TOOLS: &str =
    include_str!("../src/runner/tools/sand_external_machine_tools.rs");

const SHIPPING_HOST_MAIN: &str = include_str!("../app/src/main.rs");

#[test]
fn shipping_host_consumes_external_machine_through_runner_bridge() {
    for needle in [
        "ProductionExternalMachineExecutor::for_turn(",
        "external_machine_executor: Some(external_machine_executor)",
        "external_shell_review: Some(external_shell_review)",
        "ShellApprovalSurface::HostShell",
        "current_modes().host_shell",
    ] {
        assert!(
            SHIPPING_HOST_MAIN.contains(needle),
            "shipping Host is missing external-machine production binding: {needle}"
        );
    }
}

#[test]
fn shipping_terminal_surfaces_stable_identity_and_unknown_outcome_without_retry_guidance() {
    for needle in [
        "\"requestId\": result.request_id.clone()",
        "\"operationId\": result.request_id",
        "\"outcomeUnknown\": result.outcome_unknown",
        "\"stdoutTruncated\": result.stdout_truncated",
        "\"stderrTruncated\": result.stderr_truncated",
    ] {
        assert!(
            EXTERNAL_MACHINE_EXECUTOR.contains(needle),
            "shipping external-machine result is missing terminal recovery field: {needle}"
        );
    }
    assert!(
        EXTERNAL_MACHINE_TOOLS.contains("never automatically retried after a lost result"),
        "terminal tool description must forbid blind replay after outcome_unknown"
    );
}


#[test]
fn shipping_terminal_projects_accepted_running_and_terminal_journal_states() {
    for needle in [
        "GatewayLocalExecOperationStatus::Accepted => \"accepted\"",
        "GatewayLocalExecOperationStatus::Running => \"running\"",
        "GatewayLocalExecOperationStatus::Completed => \"completed\"",
        "GatewayLocalExecOperationStatus::Cancelled => \"cancelled\"",
        "GatewayLocalExecOperationStatus::OutcomeUnknown => \"outcome_unknown\"",
        "GatewayLocalExecOperationStatus::Failed => \"failed\"",
    ] {
        assert!(
            EXTERNAL_MACHINE_EXECUTOR.contains(needle),
            "shipping terminal status projection is missing: {needle}"
        );
    }
}
