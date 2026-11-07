use std::collections::VecDeque;
use std::sync::Mutex;

use mahayana_host_runtime::extensions::inference::cursor_inference_transport::{
    CONNECT_END_STREAM_FLAG, CursorCheckpoint, CursorCheckpointMessage,
    CursorInferenceEvent, CursorInferenceStreamTransport, decode_cursor_connect_frames,
    encode_connect_envelope, encode_cursor_inference_request,
    encode_cursor_inference_request_with_model, encode_test_response,
    run_cursor_with_transport_reporting_usage,
    run_cursor_with_transport_reporting_usage_and_partials,
};
use mahayana_host_runtime::extensions::inference::cursor_session::RequestedModel;
use mahayana_host_runtime::extensions::inference::sand_model_experiment::SandAgentModelParameter;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderMessage, ProviderTokenUsage, RoutedToolDefinition,
};
use prost::Message;
use serde_json::json;

#[derive(Clone, PartialEq, Message)]
struct RequestedModelEnvelope {
    #[prost(message, optional, tag = "7")]
    requested_model: Option<RequestedModelProto>,
}

#[derive(Clone, PartialEq, Message)]
struct RequestedModelProto {
    #[prost(string, tag = "1")]
    model_id: String,
    #[prost(bool, tag = "2")]
    max_mode: bool,
    #[prost(message, repeated, tag = "3")]
    parameters: Vec<ModelParameterProto>,
}

#[derive(Clone, PartialEq, Message)]
struct ModelParameterProto {
    #[prost(string, tag = "1")]
    id: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[test]
fn cursor_connect_stream_decoder_preserves_text_thinking_tools_usage_and_finish() {
    let expected = vec![
        CursorInferenceEvent::TextDelta("hello".into()),
        CursorInferenceEvent::ThinkingDelta {
            text: "reason".into(),
            signature: Some("sig".into()),
        },
        CursorInferenceEvent::ToolCall {
            tool_call_id: "call-1".into(),
            tool_name: "calendar_list".into(),
            args: "{\"days\":1}".into(),
            complete: true,
            tool_index: Some(0),
        },
        CursorInferenceEvent::Usage(ProviderTokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: 3,
            cache_write_tokens: 2,
            reasoning_tokens: None,
        }),
        CursorInferenceEvent::Finished,
    ];
    let mut buffer = expected
        .iter()
        .cloned()
        .flat_map(encode_test_response)
        .collect::<Vec<_>>();
    let observed = decode_cursor_connect_frames(&mut buffer).expect("decode cursor stream");
    assert_eq!(observed, expected);
    assert!(buffer.is_empty());
}

#[test]
fn cursor_connect_decoder_waits_for_complete_frame_and_consumes_end_stream_metadata() {
    let frame = encode_test_response(CursorInferenceEvent::TextDelta("hello".into()));
    let split = frame.len() - 2;
    let mut buffer = frame[..split].to_vec();
    assert!(decode_cursor_connect_frames(&mut buffer)
        .expect("partial frame")
        .is_empty());
    buffer.extend_from_slice(&frame[split..]);
    assert_eq!(
        decode_cursor_connect_frames(&mut buffer).expect("complete frame"),
        vec![CursorInferenceEvent::TextDelta("hello".into())]
    );

    let metadata = br#"{}"#;
    let mut end = vec![CONNECT_END_STREAM_FLAG];
    end.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    end.extend_from_slice(metadata);
    assert!(decode_cursor_connect_frames(&mut end)
        .expect("end stream")
        .is_empty());
    assert!(end.is_empty());
}

#[test]
fn cursor_request_is_connect_framed_and_accepts_frozen_tool_schema() {
    let request = encode_cursor_inference_request(
        &[
            ProviderMessage {
                role: "system".into(),
                content: "system".into(),
            },
            ProviderMessage {
                role: "user".into(),
                content: "hello".into(),
            },
        ],
        &[RoutedToolDefinition {
            name: "calendar_list".into(),
            provider_identifier: "calendar".into(),
            tool_name: "list".into(),
            description: Some("List calendar entries".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "days": {"type": "integer"}
                }
            }),
        }],
    )
    .expect("encode request");

    assert_eq!(request[0], 0);
    let declared = u32::from_be_bytes([request[1], request[2], request[3], request[4]]) as usize;
    assert_eq!(declared, request.len() - 5);
    assert!(request.len() > 32);
}

#[test]
fn cursor_request_uses_explicit_requested_model_instead_of_hardcoded_default() {
    let request = encode_cursor_inference_request_with_model(
        &[ProviderMessage {
            role: "user".into(),
            content: "hello".into(),
        }],
        &[],
        &RequestedModel {
            model_id: "session-model".into(),
            max_mode: Some(false),
            parameters: vec![SandAgentModelParameter {
                id: "effort".into(),
                value: "medium".into(),
            }],
        },
    )
    .expect("encode request with model");

    let payload = &request[5..];
    let decoded = RequestedModelEnvelope::decode(payload).expect("decode inference request");
    let model = decoded.requested_model.expect("requested model");
    assert_eq!(model.model_id, "session-model");
    assert!(!model.max_mode);
    assert_eq!(model.parameters.len(), 1);
    assert_eq!(model.parameters[0].id, "effort");
    assert_eq!(model.parameters[0].value, "medium");
}

#[test]
fn cursor_connect_decoder_rejects_server_end_stream_error() {
    let metadata = br#"{"error":{"code":"unavailable","message":"down"}}"#;
    let mut frame = vec![CONNECT_END_STREAM_FLAG];
    frame.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    frame.extend_from_slice(metadata);
    let error = decode_cursor_connect_frames(&mut frame).expect_err("server error");
    assert!(error.to_string().contains("unavailable"));
}


struct FakeCursorTransport {
    responses: Mutex<VecDeque<Vec<CursorInferenceEvent>>>,
}

impl CursorInferenceStreamTransport for FakeCursorTransport {
    fn stream_conversation(
        &self,
        _conversation: &[CursorCheckpointMessage],
        _tools: &[RoutedToolDefinition],
        should_cancel: &dyn Fn() -> bool,
        on_event: &mut dyn FnMut(CursorInferenceEvent) -> Result<(), mahayana_host_runtime::extensions::inference::provider_session::ProviderSessionError>,
    ) -> Result<(), mahayana_host_runtime::extensions::inference::provider_session::ProviderSessionError> {
        if should_cancel() {
            return Err(mahayana_host_runtime::extensions::inference::provider_session::ProviderSessionError::Cancelled(
                "fake cursor transport observed cancellation".into(),
            ));
        }
        let events = self
            .responses
            .lock()
            .expect("fake responses")
            .pop_front()
            .expect("fake cursor response");
        for event in events {
            on_event(event)?;
        }
        Ok(())
    }
}

#[test]
fn cursor_provider_loop_executes_tools_once_and_resumes_from_accepted_checkpoint() {
    let tools = vec![RoutedToolDefinition {
        name: "calendar_list".into(),
        provider_identifier: "calendar".into(),
        tool_name: "list".into(),
        description: Some("List calendar entries".into()),
        input_schema: json!({"type":"object"}),
    }];
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "calendar".into(),
    }];
    let transport = FakeCursorTransport {
        responses: Mutex::new(VecDeque::from([
            vec![
                CursorInferenceEvent::TextDelta("Checking ".into()),
                CursorInferenceEvent::ToolCall {
                    tool_call_id: "call-1".into(),
                    tool_name: "calendar_list".into(),
                    args: "{\"days\":1}".into(),
                    complete: true,
                    tool_index: Some(0),
                },
            ],
            vec![CursorInferenceEvent::TextDelta("done".into()), CursorInferenceEvent::Finished],
        ])),
    };
    let mut checkpoints = Vec::<CursorCheckpoint>::new();
    let mut tool_calls = 0usize;
    let mut deltas = Vec::new();
    let mut usage = Vec::new();
    let output = run_cursor_with_transport_reporting_usage(
        &transport,
        &messages,
        &tools,
        &mut |_tool, args, call_id| {
            tool_calls += 1;
            assert_eq!(call_id, "call-1");
            assert_eq!(args, json!({"days":1}));
            Ok(json!({"events":["daily"]}))
        },
        &mut |delta, accumulated| {
            deltas.push((delta.to_string(), accumulated.to_string()));
        },
        &|| false,
        None,
        &mut |checkpoint| {
            checkpoints.push(checkpoint.clone());
            Ok(())
        },
        &mut |observed| usage.push(observed),
    )
    .expect("cursor provider loop");

    assert_eq!(tool_calls, 1);
    assert_eq!(output, "Checking done");
    assert_eq!(
        deltas,
        vec![
            ("Checking ".to_string(), "Checking ".to_string()),
            ("done".to_string(), "Checking done".to_string()),
        ]
    );
    let accepted = checkpoints.last().expect("accepted checkpoint");
    assert_eq!(accepted.completed_steps, 1);
    assert_eq!(accepted.tool_calls_completed, 1);
    assert!(accepted.conversation.iter().any(|message| matches!(
        message,
        CursorCheckpointMessage::ToolResult { tool_call_id, .. } if tool_call_id == "call-1"
    )));
    assert!(usage.is_empty());

    let resumed_transport = FakeCursorTransport {
        responses: Mutex::new(VecDeque::from([vec![
            CursorInferenceEvent::TextDelta(" resumed".into()),
            CursorInferenceEvent::Finished,
        ]])),
    };
    let mut resumed_tool_calls = 0usize;
    let resumed = run_cursor_with_transport_reporting_usage(
        &resumed_transport,
        &messages,
        &tools,
        &mut |_tool, _args, _call_id| {
            resumed_tool_calls += 1;
            Ok(json!(null))
        },
        &mut |_delta, _accumulated| {},
        &|| false,
        Some(accepted),
        &mut |_checkpoint| Ok(()),
        &mut |_usage| {},
    )
    .expect("resume cursor provider loop");
    assert_eq!(resumed_tool_calls, 0);
    assert_eq!(resumed, "Checking  resumed");
}
#[test]
fn cursor_partial_tool_callback_reports_real_accumulated_state_and_model_call_identity() {
    let tools = vec![RoutedToolDefinition {
        name: "Shell".into(),
        provider_identifier: "mahayana-box".into(),
        tool_name: "Shell".into(),
        description: Some("shell".into()),
        input_schema: json!({"type":"object"}),
    }];
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "run".into(),
    }];
    let transport = FakeCursorTransport {
        responses: Mutex::new(VecDeque::from([
            vec![
                CursorInferenceEvent::ModelCallIdentity("cursor-request-1".into()),
                CursorInferenceEvent::ToolCall {
                    tool_call_id: "cursor-call-1".into(),
                    tool_name: "Shell".into(),
                    args: r#"{"command":"pri"#.into(),
                    complete: false,
                    tool_index: Some(0),
                },
                CursorInferenceEvent::ToolCall {
                    tool_call_id: "cursor-call-1".into(),
                    tool_name: "Shell".into(),
                    args: r#"{"command":"printf ok"}"#.into(),
                    complete: true,
                    tool_index: Some(0),
                },
            ],
            vec![
                CursorInferenceEvent::ModelCallIdentity("cursor-request-2".into()),
                CursorInferenceEvent::Finished,
            ],
        ])),
    };
    let mut partials = Vec::new();
    let mut calls = 0usize;
    let output = run_cursor_with_transport_reporting_usage_and_partials(
        &transport,
        &messages,
        &tools,
        &mut |_tool, args, call_id| {
            calls += 1;
            assert_eq!(call_id, "cursor-call-1");
            assert_eq!(args, json!({"command":"printf ok"}));
            Ok(json!({"kind":"success","stdout":"ok","exitCode":0}))
        },
        &mut |_delta, _accumulated| {},
        &|| false,
        None,
        &mut |_checkpoint| Ok(()),
        &mut |_usage| {},
        &mut |partial| {
            partials.push(partial);
            Ok(())
        },
    )
    .expect("cursor partial lifecycle");

    assert_eq!(output, "");
    assert_eq!(calls, 1);
    assert_eq!(partials.len(), 2);
    assert_eq!(partials[0].tool_call_id, "cursor-call-1");
    assert_eq!(partials[0].tool.tool_name, "Shell");
    assert_eq!(partials[0].raw_arguments, r#"{"command":"pri"#);
    assert_eq!(
        partials[0].model_call_id.as_deref(),
        Some("cursor-request-1")
    );
    assert_eq!(partials[1].raw_arguments, r#"{"command":"printf ok"}"#);
    assert_eq!(
        partials[1].model_call_id.as_deref(),
        Some("cursor-request-1")
    );
}
