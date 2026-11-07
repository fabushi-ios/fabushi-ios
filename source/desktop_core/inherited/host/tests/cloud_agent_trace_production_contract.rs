use mahayana_host_runtime::cloud_agents::cloud_agent_transcript_dump::cloud_agent_transcript_dump_path;
use mahayana_host_runtime::production_binding_providers::production_cloud_agent_trace_converter;
use prost::{Message, Oneof};
use serde_json::json;

const PRODUCTION_BINDINGS: &str = include_str!("../src/production_binding_providers.rs");
const PRODUCTION_EXTENSIONS: &str = include_str!("../src/host_production_extensions.rs");
const CLOUD_AGENT_SERVICE: &str =
    include_str!("../src/extensions/cloud_agents/cloud_agents_service.rs");
const CLOUD_AGENT_TOOL: &str = include_str!("../src/cloud_agents/cloud_agent_tool.rs");
const SHIPPING_HOST_MAIN: &str = include_str!("../app/src/main.rs");

#[derive(Clone, PartialEq, Message)]
struct FixtureThinking {
    #[prost(string, tag = "1")]
    text: String,
}

#[derive(Clone, PartialEq, Message)]
struct FixtureBackgroundComposerFollowupResult {
    #[prost(string, tag = "1")]
    proposed_followup: String,
    #[prost(bool, tag = "2")]
    is_sent: bool,
}

#[derive(Clone, PartialEq, Message)]
struct FixtureClientSideToolV2Result {
    #[prost(oneof = "fixture_client_side_tool_v2_result::Result", tags = "33")]
    result: Option<fixture_client_side_tool_v2_result::Result>,
}

mod fixture_client_side_tool_v2_result {
    use super::*;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "33")]
        BackgroundComposerFollowupResult(FixtureBackgroundComposerFollowupResult),
    }
}

#[derive(Clone, PartialEq, Message)]
struct FixtureToolResult {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, tag = "2")]
    tool_name: String,
    #[prost(string, tag = "4")]
    args: String,
    #[prost(string, tag = "5")]
    raw_args: String,
    #[prost(string, optional, tag = "7")]
    content: Option<String>,
    #[prost(message, optional, tag = "8")]
    result: Option<FixtureClientSideToolV2Result>,
    #[prost(uint64, optional, tag = "13")]
    started_at_ms: Option<u64>,
    #[prost(uint64, optional, tag = "14")]
    completed_at_ms: Option<u64>,
}

#[derive(Clone, PartialEq, Message)]
struct FixtureConversationMessage {
    #[prost(string, tag = "1")]
    text: String,
    #[prost(int32, tag = "2")]
    r#type: i32,
    #[prost(message, repeated, tag = "18")]
    tool_results: Vec<FixtureToolResult>,
    #[prost(message, optional, tag = "45")]
    thinking: Option<FixtureThinking>,
}

fn encode(message: FixtureConversationMessage) -> Vec<u8> {
    message.encode_to_vec()
}

fn compact(source: &str) -> String {
    source.chars().filter(|value| !value.is_whitespace()).collect()
}

#[test]
fn production_converter_matches_frozen_no_preamble_roles_args_results_and_timestamps() {
    let human = encode(FixtureConversationMessage {
        text: "hello".into(),
        r#type: 1,
        tool_results: Vec::new(),
        thinking: None,
    });
    let assistant = encode(FixtureConversationMessage {
        text: "done".into(),
        r#type: 2,
        thinking: Some(FixtureThinking {
            text: "reasoning".into(),
        }),
        tool_results: vec![
            FixtureToolResult {
                tool_call_id: "call-content".into(),
                tool_name: "Shell".into(),
                args: r#"{"source":"args"}"#.into(),
                raw_args: r#"{"source":"raw"}"#.into(),
                content: Some(r#"{"stdout":"ok"}"#.into()),
                result: Some(FixtureClientSideToolV2Result {
                    result: Some(
                        fixture_client_side_tool_v2_result::Result::BackgroundComposerFollowupResult(
                            FixtureBackgroundComposerFollowupResult {
                                proposed_followup: "ignored because content wins".into(),
                                is_sent: false,
                            },
                        ),
                    ),
                }),
                started_at_ms: Some(100),
                completed_at_ms: Some(160),
            },
            FixtureToolResult {
                tool_call_id: "call-result".into(),
                tool_name: "Followup".into(),
                args: r#"{"fallback":true}"#.into(),
                raw_args: "not-json".into(),
                content: None,
                result: Some(FixtureClientSideToolV2Result {
                    result: Some(
                        fixture_client_side_tool_v2_result::Result::BackgroundComposerFollowupResult(
                            FixtureBackgroundComposerFollowupResult {
                                proposed_followup: "next".into(),
                                is_sent: true,
                            },
                        ),
                    ),
                }),
                started_at_ms: Some(200),
                completed_at_ms: Some(250),
            },
            FixtureToolResult {
                tool_call_id: "call-args".into(),
                tool_name: "ArgsFallback".into(),
                args: "[1,2]".into(),
                raw_args: String::new(),
                content: None,
                result: None,
                started_at_ms: None,
                completed_at_ms: None,
            },
            FixtureToolResult {
                tool_call_id: " ".into(),
                tool_name: "\t".into(),
                args: r#"{"fallback":"must-not-win"}"#.into(),
                raw_args: "   ".into(),
                content: Some("   ".into()),
                result: None,
                started_at_ms: Some(300),
                completed_at_ms: Some(250),
            },
        ],
    });

    let converter = production_cloud_agent_trace_converter();
    let rows = converter(&[human, assistant]).expect("canonical generated converter");
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0], json!({"role":"user","text":"hello"}));
    assert_eq!(
        rows[1],
        json!({
            "role":"assistant",
            "text":"done",
            "thinking":"reasoning",
            "tool_calls":[
                {
                    "tool_call_id":"call-content",
                    "tool_name":"Shell",
                    "tool_args":{"source":"raw"},
                    "started_at_ms":100,
                    "completed_at_ms":160,
                    "duration_ms":60
                },
                {
                    "tool_call_id":"call-result",
                    "tool_name":"Followup",
                    "tool_args":"not-json",
                    "started_at_ms":200,
                    "completed_at_ms":250,
                    "duration_ms":50
                },
                {
                    "tool_call_id":"call-args",
                    "tool_name":"ArgsFallback",
                    "tool_args":[1,2]
                },
                {
                    "tool_call_id":" ",
                    "tool_name":"\t",
                    "tool_args":"   ",
                    "started_at_ms":300,
                    "completed_at_ms":250,
                    "duration_ms":-50
                }
            ]
        })
    );
    assert_eq!(
        rows[2],
        json!({
            "role":"tool",
            "tool_call_id":"call-content",
            "tool_name":"Shell",
            "tool_args":{"source":"raw"},
            "tool_result":{"stdout":"ok"},
            "started_at_ms":100,
            "completed_at_ms":160,
            "duration_ms":60
        })
    );
    assert_eq!(
        rows[3],
        json!({
            "role":"tool",
            "tool_call_id":"call-result",
            "tool_name":"Followup",
            "tool_args":"not-json",
            "tool_result":{
                "resultType":"backgroundComposerFollowupResult",
                "value":{
                    "proposedFollowup":"next",
                    "isSent":true
                }
            },
            "started_at_ms":200,
            "completed_at_ms":250,
            "duration_ms":50
        })
    );
    assert_eq!(
        rows[4],
        json!({
            "role":"tool",
            "tool_call_id":"call-args",
            "tool_name":"ArgsFallback",
            "tool_args":[1,2]
        })
    );
    assert_eq!(
        rows[5],
        json!({
            "role":"tool",
            "tool_call_id":" ",
            "tool_name":"\t",
            "tool_args":"   ",
            "tool_result":"   ",
            "started_at_ms":300,
            "completed_at_ms":250,
            "duration_ms":-50
        })
    );
}

#[test]
fn shipping_dump_path_consumes_the_production_converter_without_a_json_fallback() {
    let bindings = compact(PRODUCTION_BINDINGS);
    assert!(bindings.contains(
        "convert_generated_conversation_messages_to_no_preamble_trace(conversation)"
    ));
    assert!(!bindings.contains(
        "CloudAgenttranscriptdumpisunavailableuntilcanonicalgeneratedConversationMessage"
    ));

    let extensions = compact(PRODUCTION_EXTENSIONS);
    assert!(extensions.contains(
        "start_cloud_agents_extension(backend_url.clone(),Arc::clone(&auth),production_cloud_agent_trace_converter(),)"
    ));

    let service = compact(CLOUD_AGENT_SERVICE);
    assert!(service.contains(
        "(self.convert_conversation)(&conversation.conversation)"
    ));
    assert!(service.contains("serde_json::to_string(&message)"));

    let tool = compact(CLOUD_AGENT_TOOL);
    assert!(tool.contains("deps.manager.get_transcript_dump(id)"));
    assert!(tool.contains("deps.box_resources.execute_write(RunnerBoxWriteRequest{"));
    assert!(tool.contains("data:dump.jsonl.as_bytes().to_vec()"));

    let shipping_host = compact(SHIPPING_HOST_MAIN);
    assert!(shipping_host.contains(
        "letshipping_box_resources=Arc::new(ForeverBoxRunnerResourcePort::new("
    ));
    assert!(shipping_host.contains(
        "letbox_resources:Arc<dynRunnerBoxResourcePort>=shipping_box_resources.clone();"
    ));
    assert!(shipping_host.contains("manager:Arc::clone(&worker_cloud_agents)"));
    assert!(shipping_host.contains("box_resources:Arc::clone(&box_resources)"));

    assert_eq!(
        cloud_agent_transcript_dump_path("bc-production"),
        "cloud-agent-transcripts/bc-production.jsonl"
    );
}
