use mahayana_host_runtime::extensions::cloud_agents::extension::{
    CLOUD_AGENTS_DEPENDENCIES, CLOUD_AGENTS_EXTENSION_ID,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;

const EXTENSION: &str = include_str!("../src/extensions/cloud_agents/extension.rs");
const SERVICE: &str = include_str!("../src/extensions/cloud_agents/cloud_agents_service.rs");
const POLL_LOOP: &str = include_str!("../src/extensions/cloud_agents/cloud_agent_poll_loop.rs");
const PRODUCTION_OWNER: &str = include_str!("../src/host_production_extensions.rs");
const REGISTRY: &str = include_str!("../src/extensions/registry.rs");
const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const TRACE_CONTRACT: &str = include_str!("cloud_agent_trace_production_contract.rs");

#[test]
fn cloud_agents_keeps_the_frozen_id_and_auth_dependency() {
    assert_eq!(CLOUD_AGENTS_EXTENSION_ID, HostExtensionId::CloudAgents);
    assert_eq!(CLOUD_AGENTS_DEPENDENCIES, &[HostExtensionId::Auth]);
    assert!(REGISTRY.contains("HostExtensionId::CloudAgents"));
}

#[test]
fn production_start_uses_auth_host_converter_real_polling_and_policy_prefetch() {
    for needle in [
        "let cloud_agents = start_cloud_agents_extension(",
        "backend_url.clone()",
        "Arc::clone(&auth)",
        "production_cloud_agent_trace_converter()",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "shipping production composition must preserve CloudAgents wiring: {needle}"
        );
    }

    for needle in [
        "let started = Instant::now();",
        "started.elapsed().as_millis()",
        "Arc::new(std::thread::sleep)",
        "CloudAgentCompletionPoller::new(",
    ] {
        assert!(
            SERVICE.contains(needle),
            "production CloudAgents must construct the real clock/sleep-backed poller: {needle}"
        );
    }

    for needle in [
        "pub const CLOUD_AGENT_POLL_INTERVAL_MS: u64 = 10_000;",
        "poll_interval_ms: CLOUD_AGENT_POLL_INTERVAL_MS",
    ] {
        assert!(
            POLL_LOOP.contains(needle),
            "the production poller must retain the frozen polling interval: {needle}"
        );
    }

    assert!(EXTENSION.contains("service.prefetch_team_admin_policy();"));
    assert!(
        TRACE_CONTRACT.contains("production_cloud_agent_trace_converter"),
        "generated ConversationMessage -> trace adapter already has production contract evidence"
    );
}

#[test]
fn startup_failure_after_cloud_agents_preserves_reverse_unwind_before_auth() {
    let auth_start = PRODUCTION_OWNER
        .find("let auth = Arc::new(")
        .expect("shipping Host must construct Auth before CloudAgents");
    let cloud_start = PRODUCTION_OWNER
        .find("let cloud_agents = start_cloud_agents_extension(")
        .expect("shipping Host must start CloudAgents");
    let later_fallible_start = PRODUCTION_OWNER
        .find("let notify_bus = start_notify_bus_extension(")
        .expect("shipping Host must have a later fallible extension start");
    let later_failure_boundary = PRODUCTION_OWNER[later_fallible_start..]
        .find(".map_err(|error| error.to_string())?;")
        .map(|offset| later_fallible_start + offset)
        .expect("later extension startup must propagate failure");

    assert!(
        auth_start < cloud_start && cloud_start < later_fallible_start,
        "CloudAgents must start only after Auth and before the later fallible extension"
    );
    assert!(
        later_fallible_start < later_failure_boundary,
        "the later startup must retain its real failure boundary"
    );
    assert!(
        EXTENSION.contains("impl Drop for CloudAgentsExtensionInner")
            && EXTENSION.contains("self.stop();"),
        "Rust unwind must settle the already-started CloudAgents owner exactly once"
    );
}

#[test]
fn cloud_agents_on_stop_is_explicit_idempotent_and_not_owned_by_the_order_table() {
    for needle in [
        "stopped: AtomicBool",
        "if !self.stopped.swap(true, Ordering::AcqRel)",
        "self.service.dispose();",
        "pub fn stop(&self)",
        "self.inner.stop();",
        "impl Drop for CloudAgentsExtensionInner",
    ] {
        assert!(
            EXTENSION.contains(needle),
            "CloudAgents extension must own exactly-once frozen onStop semantics: {needle}"
        );
    }

    for needle in [
        "pub fn stop_cloud_agents(&self)",
        "impl Drop for ProductionHostExtensions",
        "self.cloud_agents.stop();",
    ] {
        assert!(
            PRODUCTION_OWNER.contains(needle),
            "production Host must settle CloudAgents on normal and exceptional teardown: {needle}"
        );
    }

    assert!(
        !REGISTRY.contains("CloudAgentsExtension"),
        "the 36-slot registry order table must not become a second CloudAgents lifecycle owner"
    );
}

#[test]
fn shipping_shutdown_stops_cloud_agents_after_later_stages_and_before_restart_boundary() {
    let box_stage_stop = SHIPPING_HOST
        .find("box_extensions.stop();")
        .expect("shipping Host must stop the later Box extension stage");
    let cloud_agents_stop = SHIPPING_HOST
        .find("production_extensions.stop_cloud_agents();")
        .expect("shipping Host must run the frozen CloudAgents onStop callback");
    let daemon_stop = SHIPPING_HOST
        .find("daemon.close()")
        .expect("shipping Host must settle the earlier box-exec daemon");
    let discovery_clear = SHIPPING_HOST
        .find("clear_gateway_discovery(&gateway_discovery_path)")
        .expect("shipping Host must clear discovery before restart");
    let host_lock_release = SHIPPING_HOST
        .find("host_lock.release()")
        .expect("shipping Host must release the singleton Host lock");

    assert!(
        box_stage_stop < cloud_agents_stop,
        "later production stages must settle before the first-stage CloudAgents extension"
    );
    assert!(
        cloud_agents_stop < daemon_stop,
        "CloudAgents must settle before the earlier daemon owner"
    );
    assert!(
        cloud_agents_stop < discovery_clear && cloud_agents_stop < host_lock_release,
        "CloudAgents must settle before a replacement Host can cross the restart boundary"
    );
}
