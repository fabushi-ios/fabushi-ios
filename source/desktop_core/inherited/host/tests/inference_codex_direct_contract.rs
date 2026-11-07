use std::collections::VecDeque;

use mahayana_host_runtime::extensions::inference::codex_direct_responses::{
    CodexDirectCheckpoint, CodexDirectError, CodexDirectOptions, CodexDirectTool,
    CodexDirectTransport, run_codex_direct_responses,
    run_codex_direct_responses_with_cancel,
    run_codex_direct_responses_with_lifecycle,
};
use serde_json::{Value, json};

struct FakeTransport {
    responses: VecDeque<Vec<Value>>,
    requests: Vec<Value>,
}

impl CodexDirectTransport for FakeTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), CodexDirectError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), CodexDirectError> {
        self.requests.push(request.clone());
        if should_cancel() {
            return Err(CodexDirectError::Cancelled(
                "fake transport observed Runner cancellation".into(),
            ));
        }
        for event in self.responses.pop_front().expect("fake response") {
            on_event(event)?;
        }
        Ok(())
    }
}

#[test]
fn codex_direct_streams_text_executes_tools_and_continues_the_same_turn() {
    let mut transport = FakeTransport {
        responses: VecDeque::from([
            vec![
                json!({"type":"response.output_text.delta","delta":"Checking "}),
                json!({"type":"response.output_item.done","item":{
                    "type":"function_call",
                    "name":"calendar_list",
                    "call_id":"call-1",
                    "arguments":"{\"days\":1}"
                }}),
                json!({"type":"response.completed","response":{
                    "id":"resp-1",
                    "usage":{"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":3}},
                    "output":[{
                        "type":"function_call",
                        "name":"calendar_list",
                        "call_id":"call-1",
                        "arguments":"{\"days\":1}"
                    }]
                }})
            ],
            vec![
                json!({"type":"response.output_text.delta","delta":"done"}),
                json!({"type":"response.completed","response":{
                    "id":"resp-2",
                    "usage":{"input_tokens":6,"output_tokens":1,"input_tokens_details":{"cached_tokens":0}},
                    "output":[]
                }})
            ],
        ]),
        requests: Vec::new(),
    };
    let mut options = CodexDirectOptions::new(
        "gpt-test",
        "system",
        vec![json!({"role":"user","content":"calendar"})],
    );
    options.tools = vec![CodexDirectTool {
        name: "calendar_list".into(),
        description: Some("List calendar entries".into()),
        parameters: json!({"type":"object"}),
        source: json!({"providerIdentifier":"calendar","toolName":"list"}),
    }];

    let mut calls: Vec<(String, Value, String)> = Vec::new();
    let mut deltas: Vec<(String, String)> = Vec::new();
    let result = run_codex_direct_responses(
        &mut transport,
        &options,
        &mut |tool, args, call_id| {
            calls.push((tool.name.clone(), args, call_id.to_string()));
            Ok(json!({"events":["daily"]}))
        },
        &mut |delta, accumulated| {
            deltas.push((delta.to_string(), accumulated.to_string()));
        },
    )
    .expect("provider turn");

    assert_eq!(result.text, "Checking done");
    assert_eq!(result.response_id, "resp-2");
    assert_eq!(result.usage.input_tokens, 16);
    assert_eq!(result.usage.output_tokens, 3);
    assert_eq!(result.usage.cache_read_tokens, 3);
    assert_eq!(
        calls,
        vec![(
            "calendar_list".to_string(),
            json!({"days":1}),
            "call-1".to_string()
        )]
    );
    assert_eq!(
        deltas,
        vec![
            ("Checking ".to_string(), "Checking ".to_string()),
            ("done".to_string(), "Checking done".to_string())
        ]
    );
    assert_eq!(transport.requests.len(), 2);
    let second_input = transport.requests[1]["input"].as_array().expect("second input");
    assert!(second_input.iter().any(|item| item["type"] == "function_call_output"));
}

#[test]
fn codex_direct_rejects_a_stream_without_terminal_completion() {
    let mut transport = FakeTransport {
        responses: VecDeque::from([vec![json!({
            "type":"response.output_text.delta",
            "delta":"partial"
        })]]),
        requests: Vec::new(),
    };
    let options = CodexDirectOptions::new(
        "gpt-test",
        "system",
        vec![json!({"role":"user","content":"hello"})],
    );
    let error = run_codex_direct_responses(
        &mut transport,
        &options,
        &mut |_tool, _args, _call_id| Ok(Value::Null),
        &mut |_delta, _accumulated| {},
    )
    .expect_err("missing completion should fail");
    assert!(error.to_string().contains("response.completed"));
}

#[test]
fn codex_direct_stops_on_runner_cancellation_before_processing_more_stream_events() {
    use std::cell::Cell;

    let mut transport = FakeTransport {
        responses: VecDeque::from([vec![
            json!({"type":"response.output_text.delta","delta":"should-not-emit"}),
            json!({"type":"response.completed","response":{"id":"resp-cancel","output":[]}})
        ]]),
        requests: Vec::new(),
    };
    let options = CodexDirectOptions::new(
        "gpt-test",
        "system",
        vec![json!({"role":"user","content":"cancel me"})],
    );
    let checks = Cell::new(0_u32);
    let should_cancel = || {
        let seen = checks.get();
        checks.set(seen + 1);
        seen >= 1
    };
    let mut deltas = Vec::new();
    let error = run_codex_direct_responses_with_cancel(
        &mut transport,
        &options,
        &mut |_tool, _args, _call_id| Ok(Value::Null),
        &mut |delta, _accumulated| deltas.push(delta.to_string()),
        &should_cancel,
    )
    .expect_err("runner cancellation should stop the stream");
    assert!(matches!(error, CodexDirectError::Cancelled(_)));
    assert!(deltas.is_empty(), "cancelled stream must not emit text");
}


#[test]
fn codex_direct_resumes_only_from_an_accepted_tool_boundary_checkpoint() {
    let first_round = vec![
        json!({"type":"response.output_text.delta","delta":"Checking "}),
        json!({"type":"response.output_item.done","item":{
            "type":"function_call",
            "name":"calendar_list",
            "call_id":"call-resume",
            "arguments":"{\"days\":1}"
        }}),
        json!({"type":"response.completed","response":{
            "id":"resp-before-resume",
            "usage":{"input_tokens":10,"output_tokens":2},
            "output":[{
                "type":"function_call",
                "name":"calendar_list",
                "call_id":"call-resume",
                "arguments":"{\"days\":1}"
            }]
        }})
    ];
    let failure_round = vec![json!({
        "type":"response.failed",
        "response":{"error":{"message":"transient upstream failure"}}
    })];
    let mut first_transport = FakeTransport {
        responses: VecDeque::from([first_round, failure_round]),
        requests: Vec::new(),
    };
    let mut options = CodexDirectOptions::new(
        "gpt-test",
        "system",
        vec![json!({"role":"user","content":"calendar"})],
    );
    options.tools = vec![CodexDirectTool {
        name: "calendar_list".into(),
        description: Some("List calendar entries".into()),
        parameters: json!({"type":"object"}),
        source: json!({"providerIdentifier":"calendar","toolName":"list"}),
    }];

    let mut checkpoint: Option<CodexDirectCheckpoint> = None;
    let mut first_tool_calls = 0_usize;
    let mut first_deltas = Vec::new();
    let error = run_codex_direct_responses_with_lifecycle(
        &mut first_transport,
        &options,
        None,
        &mut |_tool, _args, _call_id| {
            first_tool_calls += 1;
            Ok(json!({"events":["daily"]}))
        },
        &mut |delta, accumulated| {
            first_deltas.push((delta.to_string(), accumulated.to_string()));
        },
        &mut |accepted| {
            checkpoint = Some(accepted.clone());
            Ok(())
        },
        &|| false,
    )
    .expect_err("second provider step should fail after checkpoint");
    assert!(error.to_string().contains("transient upstream failure"));
    assert_eq!(first_tool_calls, 1);
    assert_eq!(first_deltas[0].0, "Checking ");

    let checkpoint = checkpoint.expect("tool boundary checkpoint");
    assert_eq!(checkpoint.text, "Checking ");
    assert_eq!(checkpoint.completed_steps, 1);
    assert_eq!(checkpoint.tool_calls_completed, 1);
    assert!(checkpoint
        .input
        .iter()
        .any(|item| item["type"] == "function_call_output"));

    let mut resumed_transport = FakeTransport {
        responses: VecDeque::from([vec![
            json!({"type":"response.output_text.delta","delta":"done"}),
            json!({"type":"response.completed","response":{
                "id":"resp-after-resume",
                "usage":{"input_tokens":6,"output_tokens":1},
                "output":[]
            }})
        ]]),
        requests: Vec::new(),
    };
    let mut resumed_tool_calls = 0_usize;
    let mut resumed_deltas = Vec::new();
    let resumed = run_codex_direct_responses_with_lifecycle(
        &mut resumed_transport,
        &options,
        Some(&checkpoint),
        &mut |_tool, _args, _call_id| {
            resumed_tool_calls += 1;
            Ok(Value::Null)
        },
        &mut |delta, accumulated| {
            resumed_deltas.push((delta.to_string(), accumulated.to_string()));
        },
        &mut |_accepted| Ok(()),
        &|| false,
    )
    .expect("resume from accepted checkpoint");

    assert_eq!(resumed_tool_calls, 0, "accepted tool work must not repeat");
    assert_eq!(resumed.text, "Checking done");
    assert_eq!(resumed.response_id, "resp-after-resume");
    assert_eq!(
        resumed_deltas,
        vec![("done".to_string(), "Checking done".to_string())]
    );
    assert_eq!(resumed_transport.requests.len(), 1);
    let resumed_input = resumed_transport.requests[0]["input"]
        .as_array()
        .expect("resumed input");
    assert!(resumed_input
        .iter()
        .any(|item| item["type"] == "function_call_output"));
}


#[test]
fn codex_direct_normalizes_strict_dsml_send_message_into_the_declared_tool() {
    let dsml = concat!(
        "<｜｜DSML｜｜ calls>\n",
        "<｜｜DSML｜｜ invoke name=\"SendMessage\">\n",
        "<｜｜DSML｜｜ parameter name=\"message\" string=\"true\">",
        "FABUSHI-RESEARCH-ONLY-7421",
        "</｜｜DSML｜｜ parameter>\n",
        "</｜｜DSML｜｜ invoke>\n",
        "</｜｜DSML｜｜ calls>"
    );
    let mut transport = FakeTransport {
        responses: VecDeque::from([
            vec![
                json!({"type":"response.output_text.delta","delta":dsml}),
                json!({"type":"response.completed","response":{
                    "id":"resp-dsml-1",
                    "usage":{"input_tokens":10,"output_tokens":4},
                    "output":[]
                }})
            ],
            vec![
                json!({"type":"response.output_text.delta","delta":"done"}),
                json!({"type":"response.completed","response":{
                    "id":"resp-dsml-2",
                    "usage":{"input_tokens":6,"output_tokens":1},
                    "output":[]
                }})
            ],
        ]),
        requests: Vec::new(),
    };
    let mut options = CodexDirectOptions::new(
        "deepseek-chat",
        "system",
        vec![json!({"role":"user","content":"include the marker"})],
    );
    options.tools = vec![CodexDirectTool {
        name: "SendMessage".into(),
        description: Some("Send a user-visible message".into()),
        parameters: json!({
            "type":"object",
            "required":["type"],
            "properties":{
                "type":{"type":"string"},
                "content":{"type":"string"}
            }
        }),
        source: json!({"providerIdentifier":"fabushi-runner","toolName":"SendMessage"}),
    }];

    let mut calls = Vec::new();
    let result = run_codex_direct_responses(
        &mut transport,
        &options,
        &mut |tool, args, call_id| {
            calls.push((tool.name.clone(), args, call_id.to_string()));
            Ok(json!({"sent":true}))
        },
        &mut |_delta, _accumulated| {},
    )
    .expect("DSML compatibility call should execute through the declared tool");

    assert_eq!(
        calls,
        vec![(
            "SendMessage".to_string(),
            json!({"type":"text","content":"FABUSHI-RESEARCH-ONLY-7421"}),
            "dsml-step-0-call-0".to_string(),
        )]
    );
    assert_eq!(result.text, "done");
    assert_eq!(transport.requests.len(), 2);
    let second_input = transport.requests[1]["input"].as_array().expect("second input");
    assert!(second_input.iter().any(|item| {
        item["type"] == "function_call" && item["name"] == "SendMessage"
    }));
    assert!(second_input.iter().any(|item| item["type"] == "function_call_output"));
}

#[test]
fn codex_direct_does_not_execute_mixed_or_undeclared_dsml_text() {
    for text in [
        "prefix <｜｜DSML｜｜ calls><｜｜DSML｜｜ invoke name=\"SendMessage\"></｜｜DSML｜｜ invoke></｜｜DSML｜｜ calls>",
        "<｜｜DSML｜｜ calls><｜｜DSML｜｜ invoke name=\"UnknownTool\"></｜｜DSML｜｜ invoke></｜｜DSML｜｜ calls>",
    ] {
        let mut transport = FakeTransport {
            responses: VecDeque::from([vec![
                json!({"type":"response.output_text.delta","delta":text}),
                json!({"type":"response.completed","response":{
                    "id":"resp-dsml-fail-closed",
                    "output":[]
                }})
            ]]),
            requests: Vec::new(),
        };
        let mut options = CodexDirectOptions::new(
            "deepseek-chat",
            "system",
            vec![json!({"role":"user","content":"hello"})],
        );
        options.tools = vec![CodexDirectTool {
            name: "SendMessage".into(),
            description: None,
            parameters: json!({"type":"object"}),
            source: json!({}),
        }];
        let mut calls = 0_usize;
        let result = run_codex_direct_responses(
            &mut transport,
            &options,
            &mut |_tool, _args, _call_id| {
                calls += 1;
                Ok(Value::Null)
            },
            &mut |_delta, _accumulated| {},
        )
        .expect("non-canonical DSML must stay ordinary text");
        assert_eq!(calls, 0);
        assert_eq!(result.text, text);
        assert_eq!(transport.requests.len(), 1);
    }
}
