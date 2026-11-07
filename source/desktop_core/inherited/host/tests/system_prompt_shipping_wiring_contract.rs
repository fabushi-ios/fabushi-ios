const SHIPPING_BOX_RUNNER: &str = include_str!("../src/extensions/forever_box/runner_tools.rs");
const HOST_RUNNER_COMPOSITION: &str = include_str!("../src/host_runner_composition.rs");
const COMPUTER_USE_OWNER: &str = include_str!("../src/runner/computer_use.rs");
const SHIPPING_HOST_MAIN: &str = include_str!("../app/src/main.rs");
const PROMPT_COLLECTOR_GLUE: &str = include_str!("../src/runner/prompt_collector_glue.rs");
const RUNNER_PROMPT_GLUE: &str = include_str!("../src/runner/runner_prompt_glue.rs");

#[test]
fn shipping_provider_consumes_live_box_and_computer_state_in_frozen_prompt_order() {
    let live_runtime = SHIPPING_HOST_MAIN
        .find(".append_live_runtime_sections(")
        .expect("shipping RunnerPromptGlue live-runtime binding");

    let live_runtime_owner = RUNNER_PROMPT_GLUE
        .find("pub fn append_live_runtime_sections(")
        .expect("RunnerPromptGlue live-runtime owner");
    let live_runtime_body = &RUNNER_PROMPT_GLUE[live_runtime_owner..];
    let live_mcp = live_runtime_body
        .find("append_mcp_runtime_sections_for_turn(")
        .expect("RunnerPromptGlue MCP delegate");
    let live_remote = live_runtime_body
        .find("append_remote_runtime_sections_for_turn(")
        .expect("RunnerPromptGlue remote/computer delegate");
    assert!(live_mcp < live_remote);

    let mcp_owner = PROMPT_COLLECTOR_GLUE
        .find("pub fn append_mcp_runtime_sections_for_turn(")
        .expect("prompt collector MCP owner");
    let remote_owner = PROMPT_COLLECTOR_GLUE
        .find("pub fn append_remote_runtime_sections_for_turn(")
        .expect("prompt collector remote/computer owner");
    assert!(mcp_owner < remote_owner);

    let mcp_body = &PROMPT_COLLECTOR_GLUE[mcp_owner..remote_owner];
    assert!(mcp_body.contains("append_mcp_system_prompt_sections("));
    assert!(mcp_body.contains("!is_subagent_runner"));

    let remote_body = &PROMPT_COLLECTOR_GLUE[remote_owner..];
    let remote_delegate = remote_body
        .find("append_remote_box_system_prompt(messages, remote_box)")
        .expect("canonical remote-box prompt delegate");
    let computer_delegate = remote_body
        .find("append_computer_system_prompt(messages, computer)")
        .expect("canonical Computer prompt delegate");
    assert!(remote_delegate < computer_delegate);

    let remote_state_owner = SHIPPING_HOST_MAIN
        .find("let remote_for_turn = Arc::new(move || -> Result<RunnerPromptRemoteState, String> {")
        .expect("shipping RunnerPromptGlue remote-state owner");
    let remote_state_end = remote_state_owner
        + SHIPPING_HOST_MAIN[remote_state_owner..]
            .find("let prompt_automation_sessions =")
            .expect("remote-state owner must settle before automation prompt state");
    assert!(
        remote_state_owner < live_runtime,
        "shipping Host must bind the live remote/computer getter before consuming it",
    );
    let shipping_remote_owner = &SHIPPING_HOST_MAIN[remote_state_owner..remote_state_end];
    for production_binding in [
        "role: prompt_role",
        "available: box_available",
        "runtime_state: status.state.clone()",
        "desktop_capable",
        "desktop_ready",
        "box_available",
        "control_lease_active: lease.1",
        "human_takeover_pending",
        "window_index: lease.2",
        "get_status(&prompt_remote_agent_id)",
        ".is_available()",
        ".shared_desktop()",
        "prompt_remote_handoff",
        ".get(&prompt_remote_agent_id)",
    ] {
        assert!(
            shipping_remote_owner.contains(production_binding),
            "prompt collector does not receive live shipping state: {production_binding}"
        );
    }

    for production_binding in [
        "forever_box.box_().get_agent_window_index(&agent_id)",
        ".with_availability_check(Arc::new(move |args|",
        "ComputerToolExposure::ScreenshotOnly",
        "ComputerToolExposure::Full",
        "ComputerToolExposure::Disabled",
    ] {
        assert!(
            SHIPPING_HOST_MAIN.contains(production_binding),
            "missing shipping production binding: {production_binding}"
        );
    }

    assert_eq!(
        SHIPPING_HOST_MAIN.matches("append_mcp_system_prompt_sections(").count(),
        0,
        "shipping Host must delegate MCP prompt ownership through prompt collector"
    );
    assert_eq!(
        SHIPPING_HOST_MAIN.matches("append_remote_box_system_prompt(").count(),
        0,
        "shipping Host must delegate remote-box prompt ownership through prompt collector"
    );
    assert_eq!(
        SHIPPING_HOST_MAIN.matches("append_computer_system_prompt(").count(),
        0,
        "shipping Host must delegate Computer prompt ownership through prompt collector"
    );
}


#[test]
fn shipping_remote_box_and_computer_owners_are_consumed_not_test_only_helpers() {
    for binding in [
        "RemoteBoxResourceCoordinator",
        ".connect(false, move ||",
        ".box_()\n                    .ensure_ready(&agent_id)",
        ".computer_use_plan(&connection",
        ".clear_connection()",
    ] {
        assert!(
            SHIPPING_BOX_RUNNER.contains(binding),
            "shipping ForeverBox Runner port bypasses remote-box owner: {binding}"
        );
    }

    assert!(HOST_RUNNER_COMPOSITION.contains("computer_use: Arc<Mutex<ComputerUseCoordination>>"));
    assert!(HOST_RUNNER_COMPOSITION.contains("pub fn computer_use_coordination(&self)"));
    assert!(COMPUTER_USE_OWNER.contains("pub struct ComputerControlLease"));
    assert!(COMPUTER_USE_OWNER.contains("pub fn acquire_control_lease("));
    assert!(COMPUTER_USE_OWNER.contains("pub fn owns_control_lease("));
    assert!(COMPUTER_USE_OWNER.contains("pub fn release_control_lease("));

    for binding in [
        "let shipping_box_resources = Arc::new(",
        "ForeverBoxRunnerResourcePort::new(",
        ".with_background_shell_watches(Arc::clone(&background_shell_watches))",
        "host_runner_composition.begin_computer_use_preparation(&agent_id)",
        "host_runner_composition.mark_computer_use_preparation_ready(&agent_id)",
        "host_runner_composition.owns_computer_control_lease(lease)",
        "box-chrome --sand-prepare",
        "worker_host_runner_composition.finish_computer_use_turn(",
        "shipping_box_resources.clone()",
        "computer_control_box.box_().is_available()",
        "computer_control_box.box_().inner().shared_desktop().is_none()",
        "Computer input is unavailable because this Runner does not hold a ComputerControlLease.",
    ] {
        assert!(
            SHIPPING_HOST_MAIN.contains(binding),
            "shipping Host does not consume remote-box/computer owner: {binding}"
        );
    }

    for binding in [
        "owner.acquire_control_lease(agent_id)",
        "owner.begin_preparation(agent_id)",
        ".owns_control_lease(lease)",
        "owner.record_turn_ended(usage)",
        "owner.release_control_lease(lease)",
    ] {
        assert!(
            HOST_RUNNER_COMPOSITION.contains(binding),
            "HostRunnerComposition does not own ComputerUseCoordination delegation: {binding}"
        );
    }
}
