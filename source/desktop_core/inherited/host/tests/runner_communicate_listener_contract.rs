const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const HOST_RUNNER_COMPOSITION: &str = include_str!("../src/host_runner_composition.rs");
const PRODUCTION_BRIDGE: &str = include_str!("../src/runner_production_bridge.rs");

use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::agent_v1_wire::{
    AgentToolCall, agent_tool_call, communicate_update_result,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::communicate_tool::{
    CommunicateActivity, CommunicateInteractionSink, CommunicateResult,
    CommunicateRoutedToolBridge, SAND_TOOL_MARKER, build_error_result,
    build_success_result, completed_tool_call, completed_tool_call_wire,
    empty_tool_call, encode_error, encode_sand_step, executing_tool_call,
    executing_tool_call_wire, render_result, run_communicate_execution,
    serialize_error, tool_call_wrapper,
};
use mahayana_host_runtime::runner::tools::listener_connect_cards::{
    surface_listener_connect_cards,
};
use prost::Message as _;
use serde_json::{Value, json};

#[test]
fn shipping_routine_write_surfaces_listener_connect_cards_and_arms_resume_watcher() {
    assert!(SHIPPING_HOST.contains("trigger_members(&target.spec.trigger)"));
    assert!(SHIPPING_HOST.contains("surface_listener_connect_cards("));
    assert!(SHIPPING_HOST.contains("lifecycle.is_platform_connected(platform)"));
    assert!(SHIPPING_HOST.contains("\"type\": card.message_type"));
    assert!(SHIPPING_HOST.contains("sink.send_message("));
    assert!(SHIPPING_HOST.contains("lifecycle.watch_listener_connection("));
    assert!(SHIPPING_HOST.contains("compose_production_turn("));
    assert!(SHIPPING_HOST.contains("routine_post_write,"));
    assert!(HOST_RUNNER_COMPOSITION.contains(
        "create_production_runner_composition_with_hooks(input, hooks)"
    ));
    assert!(PRODUCTION_BRIDGE.contains(
        "if let Some(routine_post_write) = hooks.routine_post_write {"
    ));
    assert!(PRODUCTION_BRIDGE.contains(
        "composition = composition.with_routine_post_write(routine_post_write);"
    ));
}

#[test]
fn listener_cards_are_fail_soft_and_preserve_frozen_instruction() {
    let platforms = vec!["slack".to_string(), "github".to_string(), "linear".to_string()];
    let surfaced = surface_listener_connect_cards(
        &platforms,
        Some(|platform: &str| -> Result<bool, String> {
            match platform {
                "slack" => Ok(false),
                "github" => Ok(true),
                _ => Err("lookup failed".into()),
            }
        }),
        |platform| (platform == "slack").then(|| "Slack".to_string()),
    );
    assert_eq!(surfaced.cards.len(), 1);
    assert_eq!(surfaced.cards[0].platform, "slack");
    assert_eq!(surfaced.cards[0].reason, "so this routine can fire");
    let reminder = surfaced.reminder.expect("reminder");
    assert!(reminder.starts_with("Slack isn't connected"));
    assert!(reminder.contains("don't paste a link"));
    assert!(reminder.contains("resumed automatically"));

    let absent = surface_listener_connect_cards(
        &platforms,
        None::<fn(&str) -> Result<bool, String>>,
        |_| None,
    );
    assert!(absent.cards.is_empty());
    assert!(absent.reminder.is_none());
}

#[test]
fn communicate_encoding_wraps_marker_activity_success_and_error() {
    let mut payload = serde_json::Map::new();
    payload.insert("phase".into(), Value::String("executing".into()));
    payload.insert("tool".into(), Value::String("demo".into()));
    let encoded = encode_sand_step(&payload);
    let parsed: Value = serde_json::from_str(&encoded).expect("sand json");
    assert_eq!(parsed[SAND_TOOL_MARKER], true);
    assert_eq!(parsed["tool"], "demo");

    let wrapped = tool_call_wrapper(&payload);
    assert_eq!(
        wrapped["tool"]["case"],
        Value::String("communicateUpdateToolCall".into())
    );
    assert!(empty_tool_call()["tool"]["value"]["args"].is_null());

    let activity = executing_tool_call(
        "UploadFile",
        Some(&CommunicateActivity {
            detail: Some("report.pdf".into()),
            target: Some("Mac".into()),
        }),
    );
    let current_step = activity["tool"]["value"]["args"]["currentStep"]
        .as_str()
        .expect("currentStep");
    let decoded: Value = serde_json::from_str(current_step).expect("decoded");
    assert_eq!(decoded["detail"], "report.pdf");
    assert_eq!(decoded["target"], "Mac");

    let success = build_success_result("done");
    assert_eq!(render_result(&success), "done");
    let completed = completed_tool_call(&success);
    assert_eq!(
        completed["tool"]["value"]["result"]["result"]["case"],
        "success"
    );

    let empty = build_success_result("");
    assert_eq!(render_result(&empty), "Tool completed.");

    let error = build_error_result("boom");
    assert_eq!(render_result(&error), "Error: boom");
    assert!(encode_error("boom").contains("boom"));
    assert_eq!(
        run_communicate_execution(|| -> Result<String, &str> { Err("failed") }),
        CommunicateResult::Error {
            error: "failed".into()
        }
    );
    assert_eq!(
        serialize_error("serialized")["tool"]["value"]["result"]["result"]["case"],
        json!("error")
    );
}


#[test]
fn communicate_wire_uses_frozen_agent_v1_tag_and_metadata() {
    let initial = executing_tool_call_wire(
        "CreateAgent",
        Some(&CommunicateActivity {
            detail: Some("researcher".into()),
            target: None,
        }),
        "call-48",
        100,
    );
    let bytes = initial.encode_to_vec();
    assert_eq!(
        &bytes[..2],
        &[0x82, 0x03],
        "agent.v1.ToolCall communicate_update_tool_call must use frozen tag 48"
    );
    let decoded = AgentToolCall::decode(bytes.as_slice()).expect("decode initial");
    assert_eq!(decoded.tool_call_id.as_deref(), Some("call-48"));
    assert_eq!(decoded.started_at_ms, Some(100));
    assert_eq!(decoded.completed_at_ms, None);
    match decoded.tool {
        Some(agent_tool_call::Tool::CommunicateUpdateToolCall(call)) => {
            let step = call
                .args
                .and_then(|args| args.current_step)
                .expect("current step");
            let step: Value = serde_json::from_str(&step).expect("step json");
            assert_eq!(step[SAND_TOOL_MARKER], true);
            assert_eq!(step["tool"], "CreateAgent");
            assert_eq!(step["detail"], "researcher");
            assert!(call.result.is_none());
        }
        _ => panic!("unexpected frozen ToolCall oneof"),
    }

    let completed = completed_tool_call_wire(
        &build_success_result("created"),
        "call-48",
        100,
        150,
    );
    let decoded = AgentToolCall::decode(completed.encode_to_vec().as_slice())
        .expect("decode completed");
    assert_eq!(decoded.completed_at_ms, Some(150));
    match decoded.tool {
        Some(agent_tool_call::Tool::CommunicateUpdateToolCall(call)) => {
            match call.result.and_then(|result| result.result) {
                Some(communicate_update_result::Result::Success(success)) => {
                    assert_eq!(success.current_step, "created");
                }
                _ => panic!("missing success result"),
            }
        }
        _ => panic!("unexpected completed ToolCall oneof"),
    }
}

#[derive(Default)]
struct FakeCommunicateDelegate;

impl RoutedToolBridge for FakeCommunicateDelegate {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![])
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if tool.tool_name == "CreateAgent" {
            Ok(Value::String("created".into()))
        } else if tool.tool_name == "UpdateAgent" {
            Err(ProviderSessionError::Tool("update failed".into()))
        } else {
            Ok(Value::String("base".into()))
        }
    }
}

#[derive(Default)]
struct RecordingCommunicateSink {
    calls: Mutex<Vec<(&'static str, String, Vec<u8>)>>,
}

impl CommunicateInteractionSink for RecordingCommunicateSink {
    fn on_tool_call(
        &self,
        phase: &'static str,
        tool_name: &str,
        tool_call: &AgentToolCall,
    ) {
        self.calls.lock().unwrap().push((
            phase,
            tool_name.to_string(),
            tool_call.encode_to_vec(),
        ));
    }
}

fn routed_tool(name: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: "fabushi-runner".into(),
        tool_name: name.into(),
        description: None,
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn shipping_communicate_bridge_emits_initial_and_completed_wire_for_frozen_tools_only() {
    let sink = Arc::new(RecordingCommunicateSink::default());
    let bridge = CommunicateRoutedToolBridge::new(
        Arc::new(FakeCommunicateDelegate),
        Some(sink.clone()),
    );

    assert_eq!(
        bridge
            .call_tool(&routed_tool("CreateAgent"), json!({}), "create-1")
            .expect("create"),
        Value::String("created".into())
    );
    assert!(bridge
        .call_tool(&routed_tool("UpdateAgent"), json!({}), "update-1")
        .is_err());
    assert_eq!(
        bridge
            .call_tool(
                &routed_tool("update_state"),
                json!({"target":"memory","action":"write","scope":"user","fact":"x"}),
                "state-1",
            )
            .expect("state"),
        Value::String("base".into())
    );
    assert_eq!(
        bridge
            .call_tool(&routed_tool("Read"), json!({}), "read-1")
            .expect("base"),
        Value::String("base".into())
    );

    let calls = sink.calls.lock().unwrap();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[0].0, "initial");
    assert_eq!(calls[0].1, "CreateAgent");
    assert_eq!(calls[1].0, "completed");
    assert_eq!(calls[2].0, "initial");
    assert_eq!(calls[2].1, "UpdateAgent");
    assert_eq!(calls[3].0, "completed");
    assert_eq!(calls[4].0, "initial");
    assert_eq!(calls[4].1, "update_state");
    let state_initial = AgentToolCall::decode(calls[4].2.as_slice()).expect("state initial");
    match state_initial.tool {
        Some(agent_tool_call::Tool::CommunicateUpdateToolCall(call)) => {
            let step = call.args.and_then(|args| args.current_step).expect("state current step");
            let step: Value = serde_json::from_str(&step).expect("state step json");
            assert_eq!(step["detail"], "user memory");
        }
        _ => panic!("unexpected state communicate ToolCall"),
    }
    assert_eq!(calls[5].0, "completed");
    for (_, _, wire) in calls.iter() {
        let decoded = AgentToolCall::decode(wire.as_slice()).expect("canonical wire");
        assert!(matches!(
            decoded.tool,
            Some(agent_tool_call::Tool::CommunicateUpdateToolCall(_))
        ));
    }
}
