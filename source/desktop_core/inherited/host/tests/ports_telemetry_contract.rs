use std::collections::BTreeMap;

use mahayana_host_runtime::ports::telemetry::{
    SAND_BOX_AUTH_ID_ENV, SAND_BOX_BOOT_STAGES, SAND_BOX_CLUSTER_ENV, SAND_BOX_STORE_ID_ENV,
    SAND_BOX_TENANT_ID_ENV, SAND_HOST_LIFECYCLE_PHASES, create_noop_sand_telemetry,
    resolve_sand_box_identity_tags_from, sand_error_detail, sand_error_detail_with_stack,
};

#[test]
fn identity_tags_trim_and_omit_empty_values_like_frozen_port() {
    let environment = BTreeMap::from([
        (SAND_BOX_AUTH_ID_ENV.to_string(), " auth ".to_string()),
        (SAND_BOX_TENANT_ID_ENV.to_string(), " ".to_string()),
        (SAND_BOX_STORE_ID_ENV.to_string(), "store".to_string()),
        (SAND_BOX_CLUSTER_ENV.to_string(), " cluster-a ".to_string()),
    ]);
    assert_eq!(
        resolve_sand_box_identity_tags_from(&environment),
        BTreeMap::from([
            ("auth_id".to_string(), "auth".to_string()),
            ("box_store_id".to_string(), "store".to_string()),
            ("cluster".to_string(), "cluster-a".to_string()),
        ])
    );
    assert_eq!(
        SAND_BOX_BOOT_STAGES,
        ["entrypoint_started", "daemon_listening", "desktop_up", "ready"]
    );
    assert_eq!(
        SAND_HOST_LIFECYCLE_PHASES,
        ["plugin_graph", "identity", "log_catchup", "transcript_read", "ready"]
    );
}

#[test]
fn error_detail_and_noop_surface_are_safe_to_call() {
    assert_eq!(sand_error_detail("boom").message, "boom");
    assert_eq!(
        sand_error_detail_with_stack("boom", Some("stack")),
        mahayana_host_runtime::ports::telemetry::SandErrorDetail {
            message: "boom".into(),
            stack: Some("stack".into()),
        }
    );

    let telemetry = create_noop_sand_telemetry();
    let turn = telemetry.start_turn("turn");
    turn.set_model("model");
    turn.set_request_id("request");
    turn.finalize("done");
    telemetry.report_tool_call_error("error");
    telemetry.report_queue_watchdog("watchdog");
    telemetry.report_automation_run("automation");
    telemetry.report_auto_review_expire_sweep_failed("sweep");
}
