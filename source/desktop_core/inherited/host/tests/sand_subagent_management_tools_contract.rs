use std::sync::{Arc, Mutex};

use mahayana_host_runtime::runner::subagent_runtime::{
    SubagentRuntime, SubagentSessionSnapshot,
};
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_subagent_management_tools::{
    CHECK_SUBAGENT_TOOL_NAME, MESSAGE_SUBAGENT_TOOL_NAME, STOP_SUBAGENT_TOOL_NAME,
    SteerReview, SubagentManagementToolBridge, SubagentSteerReviewCallback,
    check_subagent, describe_running_subagent, elapsed_label, message_subagent,
    not_running_message, stop_subagent,
};
use serde_json::{Value, json};

#[test]
fn elapsed_labels_match_frozen_rounding_and_minute_format() {
    assert_eq!(elapsed_label(0), "0s");
    assert_eq!(elapsed_label(1_499), "1s");
    assert_eq!(elapsed_label(1_500), "2s");
    assert_eq!(elapsed_label(89_400), "89s");
    assert_eq!(elapsed_label(89_500), "1m 30s");
    assert_eq!(elapsed_label(120_000), "2m");
}

fn running_runtime() -> SubagentRuntime {
    let mut runtime = SubagentRuntime::default();
    runtime.register_session(
        "worker-a",
        SubagentSessionSnapshot {
            observed_tool_call_count: 3,
            recent_activity: vec!["Read file".into(), "Clicked button".into()],
            transcript_path: Some("/tmp/worker-a.jsonl".into()),
            ..SubagentSessionSnapshot::default()
        },
    );
    runtime.dispatch_background_subagent(
        "parent",
        "box",
        "worker-a",
        "computerUse",
        "call-a",
        "Inspect the UI",
        None,
        None,
        1_000,
    );
    runtime
}

#[test]
fn check_projection_lists_and_expands_live_runtime_without_second_registry() {
    let runtime = running_runtime();
    let list = check_subagent(&runtime, None, 62_000);
    assert!(list.starts_with("1 subagent(s) running:"));
    assert!(list.contains("worker-a [computerUse]"));
    assert!(list.contains("running for 61s"));
    assert!(list.contains("3 tool call(s)"));
    assert!(list.contains("Pass a subagent_id"));

    let detailed = check_subagent(&runtime, Some(" worker-a "), 62_000);
    assert!(detailed.contains("Recent activity (oldest → newest):"));
    assert!(detailed.contains("Read file"));
    assert!(detailed.contains("/tmp/worker-a.jsonl"));
}

#[test]
fn not_running_message_reports_current_ids_or_empty_registry() {
    let runtime = running_runtime();
    let running = runtime.list_running_subagents(2_000);
    assert!(
        not_running_message("missing", &running)
            .ends_with("Currently running: worker-a.")
    );
    assert!(
        not_running_message("missing", &[])
            .ends_with("No subagents are running right now.")
    );
}

#[test]
fn steer_review_denial_does_not_interrupt_and_allowed_message_uses_runtime() {
    let mut runtime = running_runtime();
    let denied = message_subagent(
        &mut runtime,
        "worker-a",
        "Try another button",
        Some(&SteerReview {
            allowed: false,
            reason: "review blocked".into(),
        }),
        2_000,
    );
    assert_eq!(denied, "review blocked");
    assert!(runtime.is_running("worker-a"));

    let delivered = message_subagent(
        &mut runtime,
        "worker-a",
        "Try another button",
        Some(&SteerReview {
            allowed: true,
            reason: String::new(),
        }),
        2_000,
    );
    assert!(delivered.starts_with("Message delivered to subagent worker-a."));
}

#[test]
fn stop_marks_runtime_aborting_and_missing_targets_get_frozen_guidance() {
    let mut runtime = running_runtime();
    let stopped = stop_subagent(&mut runtime, "worker-a", 2_000);
    assert_eq!(
        stopped,
        "Stopping subagent worker-a. It will be torn down and won't report back."
    );

    let missing = stop_subagent(&mut runtime, "missing", 2_000);
    assert!(missing.starts_with("No subagent \"missing\" is currently running."));
}

#[test]
fn detailed_description_without_activity_has_explicit_empty_state() {
    let info = mahayana_host_runtime::runner::subagent_runtime::RunningSubagentInfo {
        subagent_id: "id".into(),
        subagent_type: "generalPurpose".into(),
        title: "Title".into(),
        elapsed_ms: 4_000,
        tool_call_count: 0,
        recent_activity: vec![],
        transcript_path: None,
    };
    let text = describe_running_subagent(&info, true);
    assert!(text.contains("No tool activity recorded yet."));
}


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
        Err(ProviderSessionError::Tool("unexpected delegate call".into()))
    }
}

#[test]
fn shipping_management_bridge_registers_all_frozen_tools_on_single_runtime() {
    let runtime = Arc::new(Mutex::new(running_runtime()));
    let review_calls = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let review_calls_sink = Arc::clone(&review_calls);
    let review: SubagentSteerReviewCallback = Arc::new(move |subagent_id, message, tool_call_id| {
        review_calls_sink.lock().unwrap().push((
            subagent_id.to_string(),
            message.to_string(),
            tool_call_id.to_string(),
        ));
        Ok(SteerReview {
            allowed: true,
            reason: String::new(),
        })
    });
    let bridge = SubagentManagementToolBridge::new(
        Arc::new(EmptyBridge),
        Arc::clone(&runtime),
        Some(review),
    );
    let tools = bridge.list_tools().expect("management tools");
    for name in [
        CHECK_SUBAGENT_TOOL_NAME,
        MESSAGE_SUBAGENT_TOOL_NAME,
        STOP_SUBAGENT_TOOL_NAME,
    ] {
        assert!(tools.iter().any(|tool| tool.name == name), "missing {name}");
    }

    let check = tools.iter().find(|tool| tool.name == CHECK_SUBAGENT_TOOL_NAME).unwrap();
    let checked = bridge
        .call_tool(check, json!({"subagent_id":"worker-a"}), "check-call")
        .expect("check");
    assert!(checked.as_str().is_some_and(|text| text.contains("worker-a")));

    let message = tools.iter().find(|tool| tool.name == MESSAGE_SUBAGENT_TOOL_NAME).unwrap();
    let steered = bridge
        .call_tool(
            message,
            json!({"subagent_id":"worker-a","message":"Try another button"}),
            "steer-call",
        )
        .expect("steer");
    assert!(steered.as_str().is_some_and(|text| text.starts_with("Message delivered")));
    assert_eq!(
        review_calls.lock().unwrap().as_slice(),
        &[(
            "worker-a".to_string(),
            "Try another button".to_string(),
            "steer-call".to_string(),
        )]
    );

    let stop = tools.iter().find(|tool| tool.name == STOP_SUBAGENT_TOOL_NAME).unwrap();
    let stopped = bridge
        .call_tool(stop, json!({"subagent_id":"worker-a"}), "stop-call")
        .expect("stop");
    assert!(stopped.as_str().is_some_and(|text| text.starts_with("Stopping subagent")));
}

#[test]
fn denied_live_steer_review_fences_runtime_mutation() {
    let runtime = Arc::new(Mutex::new(running_runtime()));
    let review: SubagentSteerReviewCallback = Arc::new(|_, _, _| {
        Ok(SteerReview {
            allowed: false,
            reason: "production review denied".into(),
        })
    });
    let bridge = SubagentManagementToolBridge::new(
        Arc::new(EmptyBridge),
        Arc::clone(&runtime),
        Some(review),
    );
    let tools = bridge.list_tools().expect("management tools");
    let message = tools.iter().find(|tool| tool.name == MESSAGE_SUBAGENT_TOOL_NAME).unwrap();
    let denied = bridge
        .call_tool(
            message,
            json!({"subagent_id":"worker-a","message":"redirect"}),
            "steer-denied",
        )
        .expect("review denial is a communicate result");
    assert_eq!(denied, Value::String("production review denied".into()));
    assert!(runtime.lock().unwrap().is_running("worker-a"));
}

#[test]
fn shipping_host_wires_management_tools_to_generated_runtime_and_auto_review() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = std::fs::read_to_string(root.join("app/src/main.rs")).expect("shipping host main");
    let host_runner_composition =
        std::fs::read_to_string(root.join("src/host_runner_composition.rs"))
            .expect("host runner composition");
    let production_bridge =
        std::fs::read_to_string(root.join("src/runner_production_bridge.rs"))
            .expect("production runner bridge");
    let composition = std::fs::read_to_string(root.join("src/runner/turn_agent_composition.rs"))
        .expect("turn composition");
    let toolset = std::fs::read_to_string(root.join("src/runner/tools/turn_toolset.rs"))
        .expect("turn toolset");
    for required in [
        "worker_subagent_management_runtime",
        "build_sand_subagent_steer_review_target",
        "review_sand_subagent_action(",
        "tool_call_id",
        "subagent_management_runtime: worker_subagent_management_runtime",
        "compose_production_turn(",
    ] {
        assert!(main.contains(required), "missing shipping management wiring: {required}");
    }
    assert!(
        !main.contains(".with_subagent_management("),
        "shipping main must delegate Runner decoration to HostRunnerComposition",
    );
    assert!(
        host_runner_composition.contains("create_production_runner_composition_with_hooks(input, hooks)"),
        "HostRunnerComposition must delegate generated-subagent management projection to the production bridge",
    );
    for required in [
        "hooks.subagent_management_runtime",
        ".with_subagent_management(",
    ] {
        assert!(
            production_bridge.contains(required),
            "missing canonical production Runner bridge management wiring: {required}",
        );
    }
    assert!(toolset.contains("SubagentManagementToolBridge"));
    assert!(composition.contains("subagent_steer_review"));
}
