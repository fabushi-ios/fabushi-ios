use mahayana_host_runtime::extensions::inference::provider_session::ProviderSessionError;
use mahayana_host_runtime::extensions::telemetry::agent_error_telemetry::{
    AGENT_ERROR_DETAIL_EVENT, AGENT_ERROR_EVENT, AgentErrorReport,
    MAX_ERROR_DETAIL_MESSAGE_LENGTH, agent_error_detail_telemetry, agent_error_telemetry,
};
use mahayana_host_runtime::extensions::transcript::turn_runtime::classify_agent_error;
use mahayana_host_runtime::ports::telemetry::sand_error_detail;

#[test]
fn frozen_agent_error_mapper_emits_summary_and_detail_records() {
    let error = ProviderSessionError::Transport(
        "Runner first-output watchdog timed out before provider output.".into(),
    );
    let report = AgentErrorReport {
        source: "ack_redrive".into(),
        conversation_id: "agent-a".into(),
        request_id: Some("request-a".into()),
        error: classify_agent_error(&error),
        detail: Some(sand_error_detail(&error)),
    };

    let summary = agent_error_telemetry(&report);
    assert_eq!(summary.level, Some("error"));
    assert_eq!(summary.event, Some(AGENT_ERROR_EVENT));
    assert_eq!(summary.metadata["source"], "ack_redrive");
    assert_eq!(summary.metadata["conversation_id"], "agent-a");
    assert_eq!(summary.metadata["request_id"], "request-a");
    assert_eq!(summary.metadata["error_code"], "SAND-E0402");
    assert_eq!(summary.metadata["error_domain"], "agent");
    assert_eq!(summary.metadata["error_retryable"], "true");

    let detail = agent_error_detail_telemetry(&report).expect("detail record");
    assert_eq!(detail.level, Some("error"));
    assert_eq!(detail.event, Some(AGENT_ERROR_DETAIL_EVENT));
    assert_eq!(detail.metadata["source"], "ack_redrive");
    assert_eq!(detail.metadata["conversation_id"], "agent-a");
    assert_eq!(detail.metadata["request_id"], "request-a");
    assert_eq!(detail.metadata["error_code"], "SAND-E0402");
    assert_eq!(
        detail.metadata["error_message"],
        "Runner first-output watchdog timed out before provider output."
    );
}

#[test]
fn error_classifier_preserves_frozen_agent_error_families() {
    let overloaded = classify_agent_error(&ProviderSessionError::Transport(
        "provider overloaded at capacity".into(),
    ));
    assert_eq!(overloaded.code, "SAND-E0401");

    let context = classify_agent_error(&ProviderSessionError::Protocol(
        "context window overflow".into(),
    ));
    assert_eq!(context.code, "SAND-E0404");

    let too_large = classify_agent_error(&ProviderSessionError::Tool(
        "conversation too large hard cap".into(),
    ));
    assert_eq!(too_large.code, "SAND-E0414");

    let retryable = classify_agent_error(&ProviderSessionError::Transport(
        "connection reset by peer".into(),
    ));
    assert_eq!(retryable.code, "SAND-E0406");

    let rejected = classify_agent_error(&ProviderSessionError::Authentication(
        "unauthorized".into(),
    ));
    assert_eq!(rejected.code, "SAND-E0405");

    let unclassified = classify_agent_error(&ProviderSessionError::Tool(
        "tool owner disappeared".into(),
    ));
    assert_eq!(unclassified.code, "SAND-E0407");
}

#[test]
fn agent_error_detail_message_is_bounded() {
    let error = ProviderSessionError::Tool("x".repeat(MAX_ERROR_DETAIL_MESSAGE_LENGTH + 25));
    let report = AgentErrorReport {
        source: "ack_redrive".into(),
        conversation_id: "agent-a".into(),
        request_id: None,
        error: classify_agent_error(&error),
        detail: Some(sand_error_detail(&error)),
    };
    let detail = agent_error_detail_telemetry(&report).expect("detail");
    assert_eq!(
        detail.metadata["error_message"].chars().count(),
        MAX_ERROR_DETAIL_MESSAGE_LENGTH
    );
    assert!(!detail.metadata.contains_key("request_id"));
}
