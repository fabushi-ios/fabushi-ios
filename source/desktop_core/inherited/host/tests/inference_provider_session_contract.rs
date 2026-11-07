use std::collections::VecDeque;
use std::fs;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::provider_session::{
    OpenRouterCheckpoint, OpenRouterTransport, ProviderMessage,
    ProviderSessionError, ProviderTokenUsage, RoutedProvider, RoutedProviderOptions,
    RoutedToolDefinition, configured_routed_provider, decode_sse_stream,
    run_openrouter_with_transport, run_openrouter_with_transport_reporting_usage,
    run_openrouter_with_transport_reporting_usage_and_partials, run_routed_provider_text,
};
use serde_json::{Value, json};

#[test]
fn provider_session_reads_router_settings_and_streams_sse_events() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "fabushi-provider-settings-{}-{suffix}.json",
        std::process::id()
    ));
    fs::write(&path, r#"{"router":{"provider":"codex"}}"#)
        .expect("write settings");
    assert_eq!(
        configured_routed_provider(&path),
        Some(RoutedProvider::Codex)
    );
    fs::remove_file(path).expect("remove settings");

    let stream = b"event: ignored\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\r\n\r\ndata: [DONE]\r\n\r\n";
    let mut events = Vec::new();
    decode_sse_stream(Cursor::new(stream.as_slice()), |event| {
        events.push(event);
        Ok(())
    })
    .expect("decode SSE");
    assert_eq!(
        events,
        vec![json!({
            "type":"response.output_text.delta",
            "delta":"hi"
        })]
    );
}

#[test]
fn provider_session_honors_runner_cancellation_before_provider_setup() {
    let root = std::env::temp_dir();
    let messages = [];
    let tools = [];
    let should_cancel = || true;
    let mut execute_tool = |_tool: &mahayana_host_runtime::extensions::inference::provider_session::RoutedToolDefinition,
                            _args: serde_json::Value,
                            _call_id: &str| -> Result<serde_json::Value, ProviderSessionError> {
        unreachable!("cancelled provider must not execute tools")
    };
    let mut on_text_delta = |_delta: &str, _accumulated: &str| {
        panic!("cancelled provider must not emit text")
    };
    let error = run_routed_provider_text(
        RoutedProvider::Cursor,
        &messages,
        &mut RoutedProviderOptions {
            data_dir: &root,
            typed_replay: None,
            cursor_auth: None,
            tools: &tools,
            mcp_server_url: None,
            execute_tool: &mut execute_tool,
            on_text_delta: &mut on_text_delta,
            on_partial_tool_call: None,
            should_cancel: &should_cancel,
        },
    )
    .expect_err("runner cancellation should win before provider setup");
    assert!(matches!(error, ProviderSessionError::Cancelled(_)));
}


struct FakeOpenRouterTransport {
    responses: VecDeque<Vec<Value>>,
    requests: Vec<Value>,
}

impl OpenRouterTransport for FakeOpenRouterTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), ProviderSessionError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), ProviderSessionError> {
        self.requests.push(request.clone());
        if should_cancel() {
            return Err(ProviderSessionError::Cancelled(
                "fake OpenRouter transport observed cancellation".into(),
            ));
        }
        for event in self.responses.pop_front().expect("fake response") {
            on_event(event)?;
        }
        Ok(())
    }
}

#[test]
fn openrouter_resumes_only_from_an_accepted_tool_boundary_checkpoint() {
    let mut first_transport = FakeOpenRouterTransport {
        responses: VecDeque::from([
            vec![json!({
                "choices":[{
                    "delta":{
                        "content":"Checking ",
                        "tool_calls":[{
                            "index":0,
                            "id":"call-resume",
                            "function":{
                                "name":"calendar_list",
                                "arguments":"{\"days\":1}"
                            }
                        }]
                    }
                }]
            })],
            vec![json!({
                "error":{"message":"transient upstream failure"}
            })],
        ]),
        requests: Vec::new(),
    };
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "calendar".into(),
    }];
    let tools = vec![RoutedToolDefinition {
        name: "calendar_list".into(),
        provider_identifier: "calendar".into(),
        tool_name: "list".into(),
        description: Some("List calendar entries".into()),
        input_schema: json!({"type":"object"}),
    }];

    let mut checkpoint: Option<OpenRouterCheckpoint> = None;
    let mut first_tool_calls = 0_usize;
    let mut first_deltas = Vec::new();
    let error = run_openrouter_with_transport(
        &mut first_transport,
        "openai/test",
        &messages,
        &tools,
        &mut |_tool, _args, _call_id| {
            first_tool_calls += 1;
            Ok(json!({"events":["daily"]}))
        },
        &mut |delta, accumulated| {
            first_deltas.push((delta.to_string(), accumulated.to_string()));
        },
        &|| false,
        None,
        &mut |accepted| {
            checkpoint = Some(accepted.clone());
            Ok(())
        },
    )
    .expect_err("second provider step should fail after checkpoint");
    assert!(error.to_string().contains("transient upstream failure"));
    assert_eq!(first_tool_calls, 1);
    assert_eq!(
        first_deltas,
        vec![("Checking ".to_string(), "Checking ".to_string())]
    );

    let checkpoint = checkpoint.expect("OpenRouter tool boundary checkpoint");
    assert_eq!(checkpoint.text, "Checking ");
    assert_eq!(checkpoint.completed_steps, 1);
    assert_eq!(checkpoint.tool_calls_completed, 1);
    assert!(checkpoint.conversation.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "call-resume"
    }));

    let mut resumed_transport = FakeOpenRouterTransport {
        responses: VecDeque::from([vec![json!({
            "choices":[{"delta":{"content":"done"}}]
        })]]),
        requests: Vec::new(),
    };
    let mut resumed_tool_calls = 0_usize;
    let mut resumed_deltas = Vec::new();
    let resumed = run_openrouter_with_transport(
        &mut resumed_transport,
        "openai/test",
        &messages,
        &tools,
        &mut |_tool, _args, _call_id| {
            resumed_tool_calls += 1;
            Ok(Value::Null)
        },
        &mut |delta, accumulated| {
            resumed_deltas.push((delta.to_string(), accumulated.to_string()));
        },
        &|| false,
        Some(&checkpoint),
        &mut |_accepted| Ok(()),
    )
    .expect("resume from accepted OpenRouter checkpoint");

    assert_eq!(
        resumed_tool_calls, 0,
        "accepted OpenRouter tool work must not repeat"
    );
    assert_eq!(resumed, "Checking done");
    assert_eq!(
        resumed_deltas,
        vec![("done".to_string(), "Checking done".to_string())]
    );
    assert_eq!(resumed_transport.requests.len(), 1);
    let resumed_messages = resumed_transport.requests[0]["messages"]
        .as_array()
        .expect("resumed messages");
    assert!(resumed_messages.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "call-resume"
    }));
}


#[test]
fn openrouter_reports_server_usage_without_estimating_tokens() {
    let mut transport = FakeOpenRouterTransport {
        responses: VecDeque::from([vec![
            json!({"choices":[{"delta":{"content":"hello"}}]}),
            json!({
                "choices":[],
                "usage":{
                    "prompt_tokens":11,
                    "completion_tokens":7,
                    "prompt_tokens_details":{"cached_tokens":3},
                    "completion_tokens_details":{"reasoning_tokens":2}
                }
            }),
        ]]),
        requests: Vec::new(),
    };
    let messages = vec![ProviderMessage {
        role: "user".into(),
        content: "hello".into(),
    }];
    let mut observed_usage = Vec::<ProviderTokenUsage>::new();
    let output = run_openrouter_with_transport_reporting_usage(
        &mut transport,
        "openai/test",
        &messages,
        &[],
        &mut |_tool, _args, _call_id| Ok(Value::Null),
        &mut |_delta, _accumulated| {},
        &|| false,
        None,
        &mut |_accepted| Ok(()),
        &mut |usage| observed_usage.push(usage),
    )
    .expect("OpenRouter usage-reporting turn");

    assert_eq!(output, "hello");
    assert_eq!(transport.requests.len(), 1);
    assert_eq!(transport.requests[0]["usage"]["include"], true);
    assert_eq!(
        observed_usage,
        vec![ProviderTokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            cache_read_tokens: 3,
            cache_write_tokens: 0,
            reasoning_tokens: Some(2),
        }]
    );
}
#[test]
fn openrouter_partial_tool_callback_reports_real_accumulated_state_and_model_call_identity() {
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
    let mut transport = FakeOpenRouterTransport {
        responses: VecDeque::from([
            vec![
                json!({
                    "id":"or-model-call-1",
                    "choices":[{"delta":{"tool_calls":[{
                        "index":0,"id":"or-call-1","function":{"name":"Shell","arguments":r#"{"command":"pri"#}
                    }]}}]
                }),
                json!({
                    "id":"or-model-call-1",
                    "choices":[{"delta":{"tool_calls":[{
                        "index":0,"function":{"arguments":r#"ntf ok"}"#}
                    }]}}]
                }),
            ],
            vec![json!({
                "id":"or-model-call-2",
                "choices":[{"delta":{"content":"done"}}]
            })],
        ]),
        requests: Vec::new(),
    };
    let mut partials = Vec::new();
    let mut calls = 0usize;
    let output = run_openrouter_with_transport_reporting_usage_and_partials(
        &mut transport,
        "test-model",
        &messages,
        &tools,
        &mut |_tool, args, call_id| {
            calls += 1;
            assert_eq!(call_id, "or-call-1");
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
    .expect("OpenRouter partial lifecycle");

    assert_eq!(output, "done");
    assert_eq!(calls, 1);
    assert_eq!(partials.len(), 2);
    assert_eq!(partials[0].tool_call_id, "or-call-1");
    assert_eq!(partials[0].tool.tool_name, "Shell");
    assert_eq!(partials[0].raw_arguments, r#"{"command":"pri"#);
    assert_eq!(
        partials[0].model_call_id.as_deref(),
        Some("or-model-call-1")
    );
    assert_eq!(partials[1].raw_arguments, r#"{"command":"printf ok"}"#);
    assert_eq!(
        partials[1].model_call_id.as_deref(),
        Some("or-model-call-1")
    );
}
