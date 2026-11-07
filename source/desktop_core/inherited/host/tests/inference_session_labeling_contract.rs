use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::cursor_session::{
    RequestLineage, RequestedModel, ResolveRequestedModelInputs, SandSessionOptions,
    parse_sand_mock_script, resolve_sand_requested_model, sand_default_model_selection,
};
use mahayana_host_runtime::extensions::inference::generated_inference_codec::{
    AgentFollowupCategorizationRequest, AgentPostTurnLabelingRequest, InferenceMessageRole,
    InferenceReason, core_message_to_proto,
};
use mahayana_host_runtime::extensions::inference::inference_service::{
    HostInferenceService, InferenceRoute, InferenceSettings, InferenceUsage,
    authorize_routed_provider_request, resolve_inference_route, usage_from_extended_fields,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, RoutedProvider,
};
use prost::Message;
use mahayana_host_runtime::extensions::inference::sand_labeling::{
    FollowupLabelingTracker, LabelMessage, SAND_AGENT_MODE, SAND_SUMMARIZATION_REASON,
    prepare_post_turn_labeling,
};
use mahayana_host_runtime::extensions::inference::sand_model_experiment::{
    SandAgentModelParameter, SandAgentModelSelection,
};

fn selection(model_id: &str, max_mode: bool) -> SandAgentModelSelection {
    SandAgentModelSelection {
        model_id: model_id.into(),
        max_mode,
        parameters: vec![SandAgentModelParameter {
            id: "effort".into(),
            value: "medium".into(),
        }],
    }
}

fn requested(
    session_options: Option<&SandSessionOptions>,
    env_model_override: Option<&str>,
    stored_default_model: Option<&SandAgentModelSelection>,
    stored_computer_use_model: Option<&SandAgentModelSelection>,
    stored_browser_use_model: Option<&SandAgentModelSelection>,
    experiment_model_override: Option<&SandAgentModelSelection>,
) -> RequestedModel {
    resolve_sand_requested_model(ResolveRequestedModelInputs {
        session_options,
        env_model_override,
        stored_default_model,
        stored_computer_use_model,
        stored_browser_use_model,
        experiment_model_override,
    })
}

#[test]
fn cursor_model_resolution_preserves_frozen_precedence() {
    let stored = selection("stored", false);
    let experiment = selection("experiment", true);
    assert_eq!(
        requested(None, None, Some(&stored), None, None, Some(&experiment)).model_id,
        "experiment"
    );

    let env = requested(None, Some("env-model"), Some(&stored), None, None, Some(&experiment));
    assert_eq!(env.model_id, "env-model");
    assert_eq!(env.max_mode, None);

    let summarization = SandSessionOptions {
        model_id: Some("summary-model".into()),
        is_summarization_session: true,
        ..Default::default()
    };
    let resolved = requested(Some(&summarization), None, Some(&stored), None, None, None);
    assert_eq!(resolved.model_id, "summary-model");
    assert_eq!(resolved.max_mode, Some(true));

    let session_override = SandSessionOptions {
        model_id: Some("session-model".into()),
        session_model_override: true,
        thinking_level: Some("high".into()),
        ..Default::default()
    };
    let resolved = requested(
        Some(&session_override),
        None,
        Some(&stored),
        None,
        None,
        Some(&experiment),
    );
    assert_eq!(resolved.model_id, "session-model");
    assert_eq!(resolved.parameters[0].id, "thinking");
    assert_eq!(resolved.parameters[0].value, "high");
    assert_eq!(stored.model_id, "stored");

    let computer = SandSessionOptions {
        is_computer_use_subagent: true,
        ..Default::default()
    };
    let resolved = requested(Some(&computer), None, None, None, None, None);
    assert_eq!(resolved.model_id, "claude-opus-4-8");
    assert_eq!(resolved.max_mode, Some(false));

    let browser_model = selection("browser", false);
    let browser = SandSessionOptions {
        is_browser_use_subagent: true,
        ..Default::default()
    };
    assert_eq!(
        requested(Some(&browser), None, None, None, Some(&browser_model), None).model_id,
        "browser"
    );

    assert_eq!(
        requested(None, None, None, None, None, None),
        RequestedModel {
            model_id: sand_default_model_selection().model_id,
            max_mode: Some(true),
            parameters: sand_default_model_selection().parameters,
        }
    );
}

#[test]
fn cursor_mock_script_accepts_send_message_and_strict_tool_calls() {
    let send = parse_sand_mock_script(r#"{"sendMessage":"hello"}"#).expect("send message");
    assert_eq!(send.tool_calls.len(), 1);
    assert_eq!(send.tool_calls[0].tool_name, "SendMessage");
    assert_eq!(send.tool_calls[0].args["content"], "hello");

    let tools = parse_sand_mock_script(
        r#"{"toolCalls":[{"toolCallId":"call-1","toolName":"Read","args":{"path":"a"}}]}"#,
    )
    .expect("tool calls");
    assert_eq!(tools.tool_calls[0].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(tools.tool_calls[0].tool_name, "Read");
    assert_eq!(tools.tool_calls[0].args["path"], "a");

    assert!(parse_sand_mock_script("{}").is_none());
    assert!(parse_sand_mock_script(r#"{"toolCalls":[]}"#).is_none());
    assert!(parse_sand_mock_script(r#"{"toolCalls":[{"toolName":"","args":{}}]}"#).is_none());
}

#[derive(Clone, Default)]
struct RecordingSettings {
    provider: Arc<Mutex<Option<RoutedProvider>>>,
    usage: Arc<Mutex<Vec<(RoutedProvider, InferenceUsage)>>>,
}

impl InferenceSettings for RecordingSettings {
    fn inference_provider(&self) -> RoutedProvider {
        self.provider.lock().unwrap().unwrap_or(RoutedProvider::Cursor)
    }

    fn record_inference_usage(&self, provider: RoutedProvider, usage: InferenceUsage) {
        self.usage.lock().unwrap().push((provider, usage));
    }
}

#[test]
fn inference_service_authorizes_cursor_only_through_the_cursor_shipping_route_and_records_usage() {
    assert_eq!(resolve_inference_route(RoutedProvider::Cursor), InferenceRoute::Cursor);
    assert_eq!(
        resolve_inference_route(RoutedProvider::Codex),
        InferenceRoute::Routed(RoutedProvider::Codex)
    );
    assert_eq!(
        authorize_routed_provider_request(
            InferenceRoute::Routed(RoutedProvider::OpenRouter),
            RoutedProvider::OpenRouter,
        )
        .unwrap(),
        RoutedProvider::OpenRouter
    );
    assert!(
        authorize_routed_provider_request(
            InferenceRoute::Routed(RoutedProvider::Codex),
            RoutedProvider::OpenRouter,
        )
        .unwrap_err()
        .contains("does not match Host inference settings")
    );
    assert_eq!(
        authorize_routed_provider_request(
            InferenceRoute::Cursor,
            RoutedProvider::Cursor,
        )
        .unwrap(),
        RoutedProvider::Cursor
    );
    assert!(
        authorize_routed_provider_request(
            InferenceRoute::Cursor,
            RoutedProvider::Codex,
        )
        .unwrap_err()
        .contains("does not match Host inference settings")
    );

    let settings = RecordingSettings::default();
    *settings.provider.lock().unwrap() = Some(RoutedProvider::OpenRouter);
    let service = HostInferenceService::new(settings.clone());
    assert_eq!(
        service.route(),
        InferenceRoute::Routed(RoutedProvider::OpenRouter)
    );

    let usage = usage_from_extended_fields(Some(10), Some(20), Some(3), Some(-1));
    assert_eq!(usage.cache_write_tokens, None);
    service.record_usage(RoutedProvider::OpenRouter, usage);
    assert_eq!(settings.usage.lock().unwrap().as_slice(), &[(RoutedProvider::OpenRouter, usage)]);
}

#[test]
fn labeling_tracks_previous_request_once_and_skips_summarization() {
    let mut tracker = FollowupLabelingTracker::default();
    let normal = [LabelMessage::default()];
    assert!(tracker.observe_stream("conv", "req-1", "model", &normal).is_none());
    let followup = tracker
        .observe_stream("conv", "req-2", "model", &normal)
        .expect("followup");
    assert_eq!(followup.replying_to_request_id, "req-1");
    assert_eq!(followup.request_id, "req-2");
    assert_eq!(followup.agent_mode, SAND_AGENT_MODE);
    assert!(tracker.observe_stream("conv", "req-2", "model", &normal).is_none());

    let summarization = [LabelMessage {
        inference_reason: Some(SAND_SUMMARIZATION_REASON.into()),
    }];
    assert!(
        tracker
            .observe_stream("conv", "req-3", "model", &summarization)
            .is_none()
    );
    assert_eq!(tracker.last_request_id("conv"), Some("req-2"));

    assert!(prepare_post_turn_labeling("", "req", "model", &normal).is_none());
    let post = prepare_post_turn_labeling("conv", "req-4", "model", &normal).expect("post");
    assert_eq!(post.agent_mode, SAND_AGENT_MODE);
}


#[test]
fn generated_labeling_codec_matches_frozen_field_contract_and_core_message_projection() {
    let messages = vec![
        ProviderMessage { role: "user".into(), content: "hello".into() },
        ProviderMessage { role: "assistant".into(), content: "world".into() },
    ];
    let projected = messages.iter().map(core_message_to_proto).collect::<Vec<_>>();
    assert_eq!(projected[0].role, InferenceMessageRole::User as i32);
    assert_eq!(projected[1].role, InferenceMessageRole::Assistant as i32);

    let followup = AgentFollowupCategorizationRequest {
        request_id: "req-2".into(),
        replying_to_request_id: "req-1".into(),
        messages: projected.clone(),
        conversation_id: Some("conv-1".into()),
        agent_mode: Some(SAND_AGENT_MODE.into()),
        model_name: Some("grok-4.5".into()),
    };
    let encoded = followup.encode_to_vec();
    let decoded = AgentFollowupCategorizationRequest::decode(encoded.as_slice()).expect("followup codec");
    assert_eq!(decoded.request_id, "req-2");
    assert_eq!(decoded.replying_to_request_id, "req-1");
    assert_eq!(decoded.conversation_id.as_deref(), Some("conv-1"));
    assert_eq!(decoded.messages.len(), 2);

    let post = AgentPostTurnLabelingRequest {
        request_id: "req-2".into(),
        messages: projected,
        conversation_id: Some("conv-1".into()),
        agent_mode: Some(SAND_AGENT_MODE.into()),
        model_name: Some("grok-4.5".into()),
    };
    let encoded = post.encode_to_vec();
    let decoded = AgentPostTurnLabelingRequest::decode(encoded.as_slice()).expect("post-turn codec");
    assert_eq!(decoded.request_id, "req-2");
    assert_eq!(decoded.conversation_id.as_deref(), Some("conv-1"));
}

#[test]
fn cursor_session_lineage_sanitizes_headers_and_preserves_inference_reason() {
    let lineage = RequestLineage {
        parent_request_id: "parent\r\nunsafe".into(),
        root_parent_request_id: "root\nunsafe".into(),
        parent_agent_tool_call_id: Some("tool\runsafe".into()),
    };
    assert_eq!(
        lineage.sanitized_headers(),
        vec![
            ("x-parent-request-id".into(), "parentunsafe".into()),
            ("x-root-parent-request-id".into(), "rootunsafe".into()),
            ("x-parent-agent-tool-call-id".into(), "toolunsafe".into()),
        ]
    );

    let options = SandSessionOptions {
        conversation_id: Some("conv-1".into()),
        inference_reason: Some(InferenceReason::GeminiVideoSubagent),
        lineage: Some(lineage),
        ..Default::default()
    };
    assert_eq!(options.conversation_id.as_deref(), Some("conv-1"));
    assert_eq!(options.inference_reason, Some(InferenceReason::GeminiVideoSubagent));
    assert!(options.lineage.is_some());
}
