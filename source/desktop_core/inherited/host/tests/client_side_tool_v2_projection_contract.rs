use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_producer::{
    ClientSideToolV2Producer, ClientSideToolV2TransportKind,
};
use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_projection::{
    ProjectedClientSideToolV2, ToolProjectionPhase, project_generated_tool_case,
    project_routed_tool_call,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::turn_observation::{
    ObservedRoutedToolBridge, TurnObservation,
};
use serde_json::{Value, json};

fn tool(name: &str, provider: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: provider.into(),
        tool_name: name.into(),
        description: None,
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn frozen_shell_projection_preserves_typed_params_partial_and_terminal_semantics() {
    let args = json!({
        "command": "pwd",
        "workingDirectory": "/workspace",
        "is_background": false
    });
    let partial = project_routed_tool_call(
        ToolProjectionPhase::Partial,
        &tool("Shell", "mahayana-box"),
        &args,
        "call-shell",
        "model-1",
        None,
    )
    .expect("partial shell projection");
    match partial {
        ProjectedClientSideToolV2::Call {
            is_streaming,
            message,
            protobuf_bytes,
            ..
        } => {
            assert!(is_streaming);
            assert_eq!(message["tool"], 15);
            assert_eq!(message["modelCallId"], "model-1");
            assert_eq!(message["runTerminalCommandV2Params"]["command"], "pwd");
            assert_eq!(message["runTerminalCommandV2Params"]["cwd"], "/workspace");
            assert!(!protobuf_bytes.is_empty());
        }
        other => panic!("unexpected shell projection {other:?}"),
    }

    let completed = project_routed_tool_call(
        ToolProjectionPhase::Completed,
        &tool("Shell", "mahayana-box"),
        &args,
        "call-shell",
        "",
        Some(Ok(json!({
            "kind": "success",
            "stdout": "/workspace\n",
            "stderr": "",
            "exitCode": 0,
            "workingDirectory": "/workspace"
        }))),
    )
    .expect("completed shell projection");
    match completed {
        ProjectedClientSideToolV2::Result {
            message,
            protobuf_bytes,
            ..
        } => {
            assert_eq!(message["tool"], 15);
            assert_eq!(message["runTerminalCommandV2Result"]["endedReason"], 1);
            assert_eq!(message["runTerminalCommandV2Result"]["exitCodeV2"], 0);
            assert!(!protobuf_bytes.is_empty());
        }
        other => panic!("unexpected shell result {other:?}"),
    }
}

#[test]
fn read_and_mcp_projection_keep_frozen_fail_closed_and_struct_semantics() {
    let read = tool("Read", "mahayana-box");
    let non_text = project_routed_tool_call(
        ToolProjectionPhase::Completed,
        &read,
        &json!({"path":"/workspace/image.png"}),
        "read-1",
        "",
        Some(Ok(json!({
            "kind":"success",
            "output":{"kind":"data","data":[1,2,3]},
            "totalLines":0
        }))),
    )
    .expect("read projection");
    assert_eq!(
        non_text.message()["error"]["clientVisibleErrorMessage"],
        "Read completed with non-text content; bytes remain available in ordinary transcript transport"
    );

    let mcp = RoutedToolDefinition {
        name: "github.get_issue".into(),
        provider_identifier: "github".into(),
        tool_name: "get_issue".into(),
        description: None,
        input_schema: json!({"type":"object"}),
    };
    let call = project_routed_tool_call(
        ToolProjectionPhase::Started,
        &mcp,
        &json!({"issue_number":20}),
        "mcp-1",
        "",
        None,
    )
    .expect("mcp call projection");
    assert_eq!(call.message()["tool"], 49);
    assert_eq!(call.message()["callMcpToolParams"]["server"], "github");
    assert_eq!(call.message()["callMcpToolParams"]["toolName"], "get_issue");
    assert!(
        call.message()["callMcpToolParams"]["toolArgs"]["fields"]["issue_number"].is_object()
    );

    let result = project_routed_tool_call(
        ToolProjectionPhase::Completed,
        &mcp,
        &json!({"issue_number":20}),
        "mcp-1",
        "",
        Some(Ok(json!({"title":"parity","open":true}))),
    )
    .expect("mcp result projection");
    assert_eq!(result.message()["tool"], 49);
    assert!(
        result.message()["callMcpToolResult"]["result"]["fields"]["title"].is_object()
    );
}

#[test]
fn frozen_generated_cases_are_schema_backed_and_unknown_cases_fail_closed() {
    let web = project_generated_tool_case(
        ToolProjectionPhase::Started,
        "webSearchToolCall",
        "web-1",
        &json!({"searchTerm":"Grok 0.18"}),
        None,
        "",
        "",
    )
    .expect("web projection");
    assert_eq!(web.message()["tool"], 18);
    assert_eq!(web.message()["webSearchParams"]["searchTerm"], "Grok 0.18");
    assert!(!web.protobuf_bytes().is_empty());

    assert!(
        project_generated_tool_case(
            ToolProjectionPhase::Started,
            "sendMessageToolCall",
            "send-1",
            &json!({"message":"hello"}),
            None,
            "",
            "",
        )
        .is_none()
    );
}

struct FakeShell;

impl RoutedToolBridge for FakeShell {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![tool("Shell", "mahayana-box")])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({
            "kind":"success",
            "stdout":"ok\n",
            "stderr":"",
            "exitCode":0,
            "workingDirectory":"/workspace"
        }))
    }
}

#[test]
fn shipping_observed_bridge_emits_call_then_result_into_host_producer_contract() {
    let seen = Arc::new(Mutex::new(Vec::<ProjectedClientSideToolV2>::new()));
    let observation = TurnObservation::shared("agent-1", None);
    {
        let seen = Arc::clone(&seen);
        observation
            .lock()
            .expect("observation")
            .set_client_side_tool_v2_handler(Arc::new(move |event| {
                seen.lock().expect("seen").push(event);
            }));
    }
    let bridge = ObservedRoutedToolBridge::new(Arc::new(FakeShell), observation);
    bridge
        .call_tool(
            &tool("Shell", "mahayana-box"),
            json!({"command":"printf ok","workingDirectory":"/workspace"}),
            "shipping-call-1",
        )
        .expect("shipping tool call");

    let events = seen.lock().expect("seen");
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], ProjectedClientSideToolV2::Call { .. }));
    assert!(matches!(events[1], ProjectedClientSideToolV2::Result { .. }));

    let mut producer = ClientSideToolV2Producer::with_epoch("contract-epoch");
    let first = producer
        .publish("agent-1", events[0].clone().into_produced_value())
        .expect("producer call");
    let second = producer
        .publish("agent-1", events[1].clone().into_produced_value())
        .expect("producer result");
    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(first.kind, ClientSideToolV2TransportKind::Call);
    assert_eq!(second.kind, ClientSideToolV2TransportKind::Result);
    assert_eq!(
        first.message.expect("call bytes").message_type,
        "aiserver.v1.ClientSideToolV2Call"
    );
    assert_eq!(
        second.message.expect("result bytes").message_type,
        "aiserver.v1.ClientSideToolV2Result"
    );
}
#[test]
fn frozen_remaining_oneofs_preserve_case_specific_params_and_results() {
    let cases = [
        (
            "listMcpResourcesToolCall",
            json!({"server":"github"}),
            "listMcpResourcesParams",
        ),
        (
            "readMcpResourceToolCall",
            json!({"server":"github","uri":"repo://20","downloadPath":"/tmp/r"}),
            "readMcpResourceParams",
        ),
        (
            "askQuestionToolCall",
            json!({"title":"Choose","questions":[{"id":"q1","prompt":"Pick","allowMultiple":true,"options":[{"id":"a","label":"A"}]}],"runAsync":true}),
            "askQuestionParams",
        ),
        (
            "mcpAuthToolCall",
            json!({"serverIdentifier":"github"}),
            "mcpAuthParams",
        ),
        (
            "webSearchToolCall",
            json!({"searchTerm":"Grok 0.18"}),
            "webSearchParams",
        ),
        (
            "webFetchToolCall",
            json!({"url":"https://example.com"}),
            "webFetchParams",
        ),
        (
            "computerUseToolCall",
            json!({"actions":[]}),
            "computerUseParams",
        ),
        (
            "recordScreenToolCall",
            json!({"durationMs":1200}),
            "recordScreenParams",
        ),
        (
            "getMcpToolsToolCall",
            json!({"server":"github","toolName":"get_issue","pattern":"issue"}),
            "getMcpToolsParams",
        ),
    ];
    for (case, args, params_case) in cases {
        let call = project_generated_tool_case(
            ToolProjectionPhase::Started,
            case,
            &format!("call-{case}"),
            &args,
            None,
            "model-case",
            "",
        )
        .unwrap_or_else(|| panic!("{case} call projection"));
        assert!(
            call.message().get(params_case).is_some(),
            "missing {params_case} for {case}"
        );
        assert_eq!(call.message()["modelCallId"], "model-case");
        assert!(
            !call.protobuf_bytes().is_empty(),
            "empty protobuf for {case}"
        );
    }

    let edit_call = project_generated_tool_case(
        ToolProjectionPhase::Partial,
        "editToolCall",
        "edit-1",
        &json!({"path":"src/lib.rs","streamContent":"fn main() {}"}),
        None,
        "model-edit",
        "",
    )
    .expect("edit call");
    assert_eq!(
        edit_call.message()["editFileV2Params"]["relativeWorkspacePath"],
        "src/lib.rs"
    );
    assert_eq!(
        edit_call.message()["editFileV2Params"]["streamingContent"],
        "fn main() {}"
    );
    assert_eq!(edit_call.message()["rawArgs"], r#"{"path":"src/lib.rs"}"#);
    assert!(matches!(
        edit_call,
        ProjectedClientSideToolV2::Call {
            is_streaming: true,
            ..
        }
    ));

    let edit_result = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "editToolCall",
        "edit-1",
        &json!({"path":"src/lib.rs"}),
        Some(Ok(json!({"case":"success","value":{
            "beforeFullFileContent":"old","afterFullFileContent":"new",
            "diffString":"-old\n+new","message":"edited"
        }}))),
        "",
        "",
    )
    .expect("edit result");
    assert_eq!(
        edit_result.message()["editFileV2Result"]["fileWasCreated"],
        false
    );
    assert_eq!(
        edit_result.message()["editFileV2Result"]["diff"]["chunks"][0]["diffString"],
        "-old\n+new"
    );
    assert_eq!(
        edit_result.message()["editFileV2Result"]["contentsBeforeEdit"],
        "old"
    );
    assert_eq!(
        edit_result.message()["editFileV2Result"]["contentsAfterEdit"],
        "new"
    );
    assert_eq!(
        edit_result.message()["editFileV2Result"]["resultForModel"],
        "edited"
    );

    let ask_async = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "askQuestionToolCall",
        "ask-1",
        &json!({}),
        Some(Ok(json!({"case":"async","value":{"answers":[{
            "questionId":"q1","selectedOptionIds":["a"],"freeformText":"note"
        }]}}))),
        "",
        "",
    )
    .expect("ask async result");
    assert_eq!(ask_async.message()["askQuestionResult"]["isAsync"], true);
    assert_eq!(
        ask_async.message()["askQuestionResult"]["answers"][0]["questionId"],
        "q1"
    );

    let auth = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "mcpAuthToolCall",
        "auth-1",
        &json!({}),
        Some(Ok(
            json!({"case":"success","value":{"serverIdentifier":"github"}}),
        )),
        "",
        "",
    )
    .expect("mcp auth result");
    assert_eq!(auth.message()["mcpAuthResult"]["success"], true);
    assert_eq!(auth.message()["mcpAuthResult"]["message"], "github");

    let search = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "webSearchToolCall",
        "search-1",
        &json!({}),
        Some(Ok(json!({"case":"rejected","value":{"references":[{"title":"r","url":"https://example.com"}]}}))),
        "",
        "",
    )
    .expect("web search result");
    assert_eq!(search.message()["webSearchResult"]["rejected"], true);
    assert_eq!(search.message()["webSearchResult"]["isFinal"], true);
    assert_eq!(
        search.message()["webSearchResult"]["references"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let fetch = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "webFetchToolCall",
        "fetch-1",
        &json!({"url":"https://example.com"}),
        Some(Ok(json!({"case":"error","value":{"message":"blocked"}}))),
        "",
        "",
    )
    .expect("web fetch result");
    assert_eq!(
        fetch.message()["webFetchResult"]["url"],
        "https://example.com"
    );
    assert_eq!(fetch.message()["webFetchResult"]["error"], "blocked");

    let list = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "listMcpResourcesToolCall",
        "list-1",
        &json!({}),
        Some(Ok(
            json!({"case":"success","value":{"resources":[{"uri":"repo://20","name":"repo"}]}}),
        )),
        "",
        "",
    )
    .expect("list resources result");
    assert_eq!(
        list.message()["listMcpResourcesResult"]["resources"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let get_tools = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "getMcpToolsToolCall",
        "tools-1",
        &json!({}),
        Some(Ok(
            json!({"case":"success","value":{"content":"tools","outputFilePath":"/tmp/tools"}}),
        )),
        "",
        "",
    )
    .expect("get tools result");
    assert_eq!(get_tools.message()["getMcpToolsResult"]["content"], "tools");
    assert_eq!(
        get_tools.message()["getMcpToolsResult"]["outputFilePath"],
        "/tmp/tools"
    );

    let generated = project_generated_tool_case(
        ToolProjectionPhase::Started,
        "generateImageToolCall",
        "image-1",
        &json!({"description":"lotus"}),
        None,
        "",
        "",
    )
    .expect("generate image call");
    assert!(generated.message().get("generateImageParams").is_none());
    assert!(!generated.protobuf_bytes().is_empty());
}

#[test]
fn provider_partial_lifecycle_emits_call_partial_then_result_without_execution_reopen() {
    use mahayana_host_runtime::extensions::inference::provider_session::ProviderPartialToolCall;

    let seen = Arc::new(Mutex::new(Vec::<ProjectedClientSideToolV2>::new()));
    let observation = TurnObservation::shared("agent-partial", None);
    {
        let seen = Arc::clone(&seen);
        observation
            .lock()
            .expect("observation")
            .set_client_side_tool_v2_handler(Arc::new(move |event| {
                seen.lock().expect("seen").push(event);
            }));
    }
    let bridge = ObservedRoutedToolBridge::new(Arc::new(FakeShell), observation);
    let shell = tool("Shell", "mahayana-box");
    bridge
        .observe_partial_tool_call(&ProviderPartialToolCall {
            tool: shell.clone(),
            tool_call_id: "stream-call-1".into(),
            raw_arguments: r#"{"command":"pri"#.into(),
            model_call_id: Some("model-stream-1".into()),
        })
        .expect("first provider partial");
    bridge
        .observe_partial_tool_call(&ProviderPartialToolCall {
            tool: shell.clone(),
            tool_call_id: "stream-call-1".into(),
            raw_arguments: r#"{"command":"printf ok","workingDirectory":"/workspace"}"#.into(),
            model_call_id: Some("model-stream-1".into()),
        })
        .expect("second provider partial");
    bridge
        .call_tool(
            &shell,
            json!({"command":"printf ok","workingDirectory":"/workspace"}),
            "stream-call-1",
        )
        .expect("execution result");

    let events = seen.lock().expect("seen");
    assert_eq!(
        events.len(),
        3,
        "execution must not fabricate a second Started call"
    );
    match &events[0] {
        ProjectedClientSideToolV2::Call {
            raw_args,
            model_call_id,
            is_streaming,
            ..
        } => {
            assert_eq!(raw_args, r#"{"command":"pri"#);
            assert_eq!(model_call_id.as_deref(), Some("model-stream-1"));
            assert!(!is_streaming);
        }
        other => panic!("expected first Call, got {other:?}"),
    }
    match &events[1] {
        ProjectedClientSideToolV2::Call {
            raw_args,
            is_streaming,
            ..
        } => {
            assert_eq!(
                raw_args,
                r#"{"command":"printf ok","workingDirectory":"/workspace"}"#
            );
            assert!(*is_streaming);
        }
        other => panic!("expected Partial call, got {other:?}"),
    }
    assert!(matches!(
        events[2],
        ProjectedClientSideToolV2::Result { .. }
    ));

    let mut producer = ClientSideToolV2Producer::with_epoch("partial-epoch");
    let produced = events
        .iter()
        .cloned()
        .map(|event| {
            producer
                .publish("agent-partial", event.into_produced_value())
                .expect("produced")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        produced
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(produced[0].kind, ClientSideToolV2TransportKind::Call);
    assert_eq!(produced[1].kind, ClientSideToolV2TransportKind::Call);
    assert_eq!(produced[2].kind, ClientSideToolV2TransportKind::Result);
}

#[test]
fn frozen_task_and_direct_terminal_oneofs_are_schema_backed() {
    let task_call = project_generated_tool_case(
        ToolProjectionPhase::Started,
        "taskToolCall",
        "task-1",
        &json!({"description":"inspect","prompt":"check parity","subagentType":"explore","model":"gpt","mode":2}),
        None,
        "model-task",
        "",
    ).expect("task call");
    assert_eq!(task_call.message()["tool"], 48);
    assert_eq!(
        task_call.message()["taskV2Params"]["prompt"],
        "check parity"
    );
    assert!(!task_call.protobuf_bytes().is_empty());
    let task_result = project_generated_tool_case(
        ToolProjectionPhase::Completed,
        "taskToolCall",
        "task-1",
        &json!({}),
        Some(Ok(
            json!({"agentId":"agent-sub","isBackground":false,"cloudAgentBcId":"cloud-1"}),
        )),
        "",
        "",
    )
    .expect("task result");
    assert_eq!(
        task_result.message()["taskV2Result"]["agentId"],
        "agent-sub"
    );
    assert_eq!(task_result.message()["taskV2Result"]["isBackground"], false);

    let direct = [
        (
            "readMcpResourceToolCall",
            "readMcpResourceResult",
            json!({"contents":[{"uri":"repo://20","text":"ok"}]}),
        ),
        (
            "computerUseToolCall",
            "computerUseResult",
            json!({"output":"done"}),
        ),
        (
            "generateImageToolCall",
            "generateImageResult",
            json!({"imageUrl":"file:///tmp/image.png"}),
        ),
        (
            "recordScreenToolCall",
            "recordScreenResult",
            json!({"outputPath":"/tmp/screen.mp4"}),
        ),
    ];
    for (case, result_case, value) in direct {
        let result = project_generated_tool_case(
            ToolProjectionPhase::Completed,
            case,
            &format!("result-{case}"),
            &json!({}),
            Some(Ok(if case == "readMcpResourceToolCall" {
                json!({"case":"success","value":value})
            } else {
                value
            })),
            "",
            "",
        )
        .unwrap_or_else(|| panic!("{case} result projection"));
        assert!(
            result.message().get(result_case).is_some(),
            "missing {result_case}"
        );
        assert!(
            !result.protobuf_bytes().is_empty(),
            "empty result protobuf for {case}"
        );
    }
}
