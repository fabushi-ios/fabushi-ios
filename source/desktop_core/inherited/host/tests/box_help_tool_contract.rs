use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge,
};
use mahayana_host_runtime::runner::tools::box_help_tool::{
    BoxHelpOutcome, BoxHelpRequest, BoxHelpToolBridge, SAND_REQUEST_BOX_HELP_TOOL_NAME,
    connector_card_emission_to_message, normalize_box_help_domain,
};
use mahayana_host_runtime::runner::tools::send_message_tool::{
    ResolvedAttachmentSource, SendMessageSink,
};
use serde_json::{Value, json};

struct EmptyBridge;
impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }
    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("no upstream tools".into()))
    }
}

struct BoxHelpSink {
    requests: Mutex<Vec<BoxHelpRequest>>,
    outcome: BoxHelpOutcome,
}

impl SendMessageSink for BoxHelpSink {
    fn resolve_attachment_source(
        &self,
        source_url: &str,
        _tool_call_id: &str,
    ) -> Result<ResolvedAttachmentSource, ProviderSessionError> {
        Ok(ResolvedAttachmentSource { url: source_url.to_string(), file_name: None })
    }
    fn send_message(
        &self,
        _message: Value,
        _timestamp_ms: u64,
        _tool_call_id: &str,
    ) -> Result<Option<String>, ProviderSessionError> {
        Err(ProviderSessionError::Tool("not used".into()))
    }
    fn request_box_help(
        &self,
        request: BoxHelpRequest,
        _timestamp_ms: u64,
        _tool_call_id: &str,
    ) -> Result<BoxHelpOutcome, ProviderSessionError> {
        self.requests.lock().expect("requests").push(request);
        Ok(self.outcome.clone())
    }
}

#[test]
fn frozen_box_help_normalizes_domains_and_connector_card_projection() {
    assert_eq!(
        normalize_box_help_domain(" https://WWW.Google.COM/login ").as_deref(),
        Some("google.com")
    );
    assert_eq!(
        normalize_box_help_domain("Salesforce.com/path").as_deref(),
        Some("salesforce.com")
    );
    assert_eq!(normalize_box_help_domain("   "), None);
    assert_eq!(
        connector_card_emission_to_message("slack", "server-1", "connect"),
        json!({"type":"connector","connector":"slack","serverId":"server-1","variant":"connect"})
    );
}

#[test]
fn request_box_help_is_first_party_and_cancels_provider_for_waiting_user() {
    let cancellation = RoutedProviderCancellation::default();
    let sink = Arc::new(BoxHelpSink {
        requests: Mutex::new(Vec::new()),
        outcome: BoxHelpOutcome::Started { request_id: "request-1".into() },
    });
    let bridge = BoxHelpToolBridge::new(Arc::new(EmptyBridge), sink.clone(), cancellation.clone());
    let tool = bridge.list_tools().expect("tools").into_iter()
        .find(|tool| tool.name == SAND_REQUEST_BOX_HELP_TOOL_NAME)
        .expect("request_box_help");
    let result = bridge.call_tool(
        &tool,
        json!({
            "instruction":" Sign in to Google ",
            "reason":"auth",
            "domain":"Example.com",
            "idp_domain":"accounts.google.com"
        }),
        "tool-1",
    ).expect("request help");
    assert!(result.as_str().is_some_and(|value| value.contains("Handed the box")));
    assert_eq!(
        sink.requests.lock().expect("requests").as_slice(),
        &[BoxHelpRequest {
            instruction: "Sign in to Google".into(),
            reason: Some("auth".into()),
            domain: Some("example.com".into()),
            idp_domain: Some("accounts.google.com".into()),
        }]
    );
    assert_eq!(
        cancellation.reason().as_deref(),
        Some("waiting-user:box-help:request-1")
    );
}

#[test]
fn already_pending_box_help_ends_turn_without_second_visible_request() {
    let cancellation = RoutedProviderCancellation::default();
    let sink = Arc::new(BoxHelpSink {
        requests: Mutex::new(Vec::new()),
        outcome: BoxHelpOutcome::AlreadyPending {
            request_id: "request-live".into(),
            instruction: "Approve 2FA".into(),
        },
    });
    let bridge = BoxHelpToolBridge::new(Arc::new(EmptyBridge), sink, cancellation.clone());
    let tool = bridge.list_tools().expect("tools").into_iter()
        .find(|tool| tool.name == SAND_REQUEST_BOX_HELP_TOOL_NAME)
        .expect("request_box_help");
    let result = bridge.call_tool(
        &tool,
        json!({"instruction":"Try another login","reason":"not-real"}),
        "tool-2",
    ).expect("already pending");
    assert!(result.as_str().is_some_and(|value| value.contains("still has the box")));
    assert_eq!(
        cancellation.reason().as_deref(),
        Some("waiting-user:box-help:request-live")
    );
}
