use mahayana_host_runtime::runner::{
    agent_state::{state_write_failed, state_write_ok, StateWriteResult},
    clock_skew_guard::{
        bucket_clock_skew_delta_ms, sanitize_cross_clock_duration_ms, ClockSkewReason,
        SanitizedCrossClockDuration, SEND_DISPATCH_MAX_PLAUSIBLE_MS,
    },
    site_visit_tracking::visited_site_host,
    video_container::{bytes_look_like_video_container, has_ascii_at},
};
use mahayana_host_runtime::runner::tools::{
    mcp_server_resolution::{
        resolve_mcp_server_row_by_identifier_or_legacy_id,
        resolve_mcp_server_rows_by_identifier_or_legacy_id, McpInstalledServer,
    },
    sand_permission_request::summarize_permission_request,
    sand_secret_request::{
        build_secret_provided_ack, clamp_secret_description, clamp_secret_label,
        summarize_secret_request, SECRET_REQUEST_MAX_DESCRIPTION_LENGTH,
        SECRET_REQUEST_MAX_LABEL_LENGTH,
    },
    tool_input_error::SandToolInputError,
};

#[test]
fn agent_state_preserves_success_and_failure_payloads() {
    assert_eq!(state_write_ok(7), StateWriteResult::Ok { detail: 7 });
    assert_eq!(
        state_write_failed::<i32>("disk full"),
        StateWriteResult::Failed { reason: "disk full".into() }
    );
}

#[test]
fn clock_skew_guard_matches_grok_boundaries() {
    assert_eq!(
        sanitize_cross_clock_duration_ms(12.6, SEND_DISPATCH_MAX_PLAUSIBLE_MS as f64),
        SanitizedCrossClockDuration::Millis(13)
    );
    assert_eq!(
        sanitize_cross_clock_duration_ms(-0.1, 100.0),
        SanitizedCrossClockDuration::Skew(ClockSkewReason::Negative)
    );
    assert_eq!(
        sanitize_cross_clock_duration_ms(f64::NAN, 100.0),
        SanitizedCrossClockDuration::Skew(ClockSkewReason::TooLarge)
    );
    assert_eq!(bucket_clock_skew_delta_ms(-999.0), "neg_le_1s");
    assert_eq!(bucket_clock_skew_delta_ms(60_001.0), "le_5m");
    assert_eq!(bucket_clock_skew_delta_ms(f64::INFINITY), "nonfinite");
}

#[test]
fn site_visit_and_video_detection_match_reference_shapes() {
    assert_eq!(
        visited_site_host("https://www.example.com/a?q=1").as_deref(),
        Some("example.com")
    );
    assert_eq!(visited_site_host("not a url"), None);
    assert!(has_ascii_at(b"....ftyp", 4, "ftyp"));
    assert!(bytes_look_like_video_container(b"....ftypisom"));
    assert!(bytes_look_like_video_container(&[26, 69, 223, 163, 0]));
    assert!(!bytes_look_like_video_container(b"plain text"));
}

#[derive(Debug)]
struct Server {
    id: &'static str,
    identifier: &'static str,
}
impl McpInstalledServer for Server {
    fn id(&self) -> &str { self.id }
    fn server_identifier(&self) -> &str { self.identifier }
}

#[test]
fn mcp_resolution_prefers_server_identifier_then_legacy_id() {
    let rows = [
        Server { id: "legacy-a", identifier: "github" },
        Server { id: "github", identifier: "other" },
    ];
    let exact = resolve_mcp_server_rows_by_identifier_or_legacy_id(&rows, " github ");
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0].identifier, "github");
    let legacy = resolve_mcp_server_row_by_identifier_or_legacy_id(&rows, "legacy-a").unwrap();
    assert_eq!(legacy.identifier, "github");
    assert!(resolve_mcp_server_rows_by_identifier_or_legacy_id(&rows, " ").is_empty());
}

#[test]
fn permission_secret_and_tool_error_helpers_preserve_safety_text() {
    assert!(summarize_permission_request("Camera", "Needed").contains("no longer actionable"));
    assert_eq!(clamp_secret_label("  API\n  token "), "API token");
    assert_eq!(clamp_secret_label(&"x".repeat(200)).len(), SECRET_REQUEST_MAX_LABEL_LENGTH);
    assert_eq!(
        clamp_secret_description(&format!("  {}  ", "y".repeat(500))).len(),
        SECRET_REQUEST_MAX_DESCRIPTION_LENGTH
    );
    assert!(summarize_secret_request("API token").contains("securely"));
    let ack = build_secret_provided_ack("API token", "connector");
    assert!(ack.contains("you never see the value"));
    let error = SandToolInputError::new("bad input");
    assert_eq!(error.name(), "SandToolInputError");
    assert_eq!(error.to_string(), "bad input");
}
