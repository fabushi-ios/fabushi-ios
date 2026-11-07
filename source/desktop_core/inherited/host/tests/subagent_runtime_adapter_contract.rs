use std::collections::HashMap;

use mahayana_host_runtime::runner::agent_adapters::{
    AgentUpdate, LaunchReview, SandRequestContextExecutor, SandSubagentHostAdapter,
    SubagentAdapterArgs, forward_agent_update, derive_sand_subagent_request_lineage,
    RequestContextInfo,
};
use mahayana_host_runtime::runner::conversation_outline::{
    OutlineToolCall, TaskToolCall,
};
use mahayana_host_runtime::runner::subagent_runtime::{
    ComputerUseUsageSnapshot, ControlResult, RunOutcome, SubagentLineage, SubagentRuntime,
    SubagentSessionSnapshot, SubagentStatus, compute_subagent_request_id,
    subagent_steer_run_options,
};
use mahayana_host_runtime::runner::{TurnEndedUsage, TurnUsage};
use serde_json::json;

#[test]
fn request_identity_and_steer_lineage_match_frozen_contract() {
    assert_eq!(compute_subagent_request_id("call-7"), "subagent:call-7");
    assert_eq!(compute_subagent_request_id(""), "subagent");

    let options = subagent_steer_run_options(
        &mahayana_host_runtime::runner::subagent_runtime::SubagentDispatchMeta {
            tool_call_id: "call-7".into(),
            lineage: Some(SubagentLineage {
                parent_request_id: Some("parent".into()),
                root_parent_request_id: Some("root".into()),
                parent_agent_tool_call_id: Some("stale".into()),
            }),
        },
    );
    assert_eq!(options.inference_request_id, "subagent:call-7");
    assert_eq!(
        options.lineage.unwrap().parent_agent_tool_call_id.as_deref(),
        Some("call-7")
    );
}

#[test]
fn runtime_steer_abort_settle_and_computer_usage_follow_frozen_lifecycle() {
    let mut runtime = SubagentRuntime::default();
    let mut action_counts = HashMap::new();
    action_counts.insert("screenshot".to_string(), 2);
    action_counts.insert("click".to_string(), 3);
    runtime.register_session(
        "worker",
        SubagentSessionSnapshot {
            resolved_outline: vec![json!({"kind":"tool-call"})],
            observed_tool_call_count: 5,
            recent_activity: vec!["Clicking".into()],
            transcript_path: Some("/tmp/worker.jsonl".into()),
            computer_use_usage: Some(ComputerUseUsageSnapshot {
                model_id: Some("model-x".into()),
                turn_ended_count: 2,
                usage: Some(TurnUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    reasoning_tokens: None,
                }),
            }),
            computer_use_action_counts: action_counts,
        },
    );
    let wake = runtime
        .dispatch_background_subagent(
            "parent-agent",
            "box-1",
            "worker",
            "computer-use",
            "call-9",
            "Inspect the trace",
            Some(SubagentLineage {
                parent_request_id: Some("req-parent".into()),
                root_parent_request_id: Some("req-root".into()),
                parent_agent_tool_call_id: None,
            }),
            Some("quiet:test"),
            100,
        )
        .expect("dispatch");
    assert_eq!(wake.title, "Inspect the trace");
    assert!(runtime.is_running("worker"));

    assert_eq!(
        runtime.steer_subagent("worker", "Check the network tab"),
        ControlResult::Ok {
            interrupt_reason: "Steering message from the parent agent."
        }
    );
    let continuation = runtime
        .settle_background_subagent_turn("worker", RunOutcome::Completed("old".into()), 140)
        .continuation
        .expect("steer continuation");
    assert!(continuation.prompt.contains("Check the network tab"));
    assert_eq!(continuation.options.inference_request_id, "subagent:call-9");
    assert!(runtime.is_running("worker"));

    assert_eq!(
        runtime.abort_subagent("worker"),
        ControlResult::Ok {
            interrupt_reason: "Stopped by the parent agent."
        }
    );
    let settled = runtime.settle_background_subagent_turn("worker", RunOutcome::Aborted, 180);
    assert!(settled.completion.is_none());
    assert_eq!(
        settled.pending_wake_disarmed,
        Some(("parent-agent".into(), "worker".into()))
    );
    let usage = settled.computer_use_usage.expect("computer usage");
    assert_eq!(usage.outcome, "aborted");
    assert_eq!(usage.tool_call_count, 5);
    assert_eq!(usage.turn_ended_count, 2);
    let audit = settled.computer_use_audit.expect("computer audit");
    assert_eq!(audit.action_count, 5);
    assert_eq!(audit.screenshot_count, 2);
    assert_eq!(runtime.list_subagents()[0].1.status, SubagentStatus::Aborted);
    assert_eq!(runtime.get_subagent_outline("worker").len(), 1);
}


#[test]
fn parent_delete_abort_targets_only_running_children_owned_by_that_parent() {
    let mut runtime = SubagentRuntime::default();
    for child in ["child-b", "child-a", "other-child"] {
        runtime.register_session(child, SubagentSessionSnapshot::default());
    }
    runtime.dispatch_background_subagent(
        "parent-a",
        "box",
        "child-a",
        "generalPurpose",
        "call-a",
        "first",
        None,
        None,
        10,
    );
    runtime.dispatch_background_subagent(
        "parent-a",
        "box",
        "child-b",
        "generalPurpose",
        "call-b",
        "second",
        None,
        None,
        11,
    );
    runtime.dispatch_background_subagent(
        "parent-b",
        "box",
        "other-child",
        "generalPurpose",
        "call-c",
        "third",
        None,
        None,
        12,
    );

    assert_eq!(
        runtime.running_subagent_ids_for_parent("parent-a"),
        vec!["child-a".to_string(), "child-b".to_string()]
    );
    assert_eq!(
        runtime
            .running_subagent_records_for_parent("parent-a")
            .into_iter()
            .map(|(id, record)| (id, record.title, record.started_at_ms))
            .collect::<Vec<_>>(),
        vec![
            ("child-a".to_string(), "first".to_string(), 10),
            ("child-b".to_string(), "second".to_string(), 11),
        ]
    );
    assert_eq!(
        runtime.abort_running_subagents_for_parent("parent-a"),
        vec!["child-a".to_string(), "child-b".to_string()]
    );
    assert!(
        runtime
            .running_subagent_records_for_parent("parent-a")
            .is_empty(),
        "aborting children must leave the frozen async-task projection immediately"
    );

    for child in ["child-a", "child-b"] {
        let settled =
            runtime.settle_background_subagent_turn(child, RunOutcome::Completed("late".into()), 20);
        assert!(settled.completion.is_none());
    }
    assert!(runtime.running_subagent_ids_for_parent("parent-a").is_empty());
    assert_eq!(
        runtime.running_subagent_ids_for_parent("parent-b"),
        vec!["other-child".to_string()]
    );
    assert!(runtime.is_running("other-child"));
}

#[test]
fn completed_background_task_preserves_empty_output_fallback() {
    let mut runtime = SubagentRuntime::default();
    runtime.register_session("worker", SubagentSessionSnapshot::default());
    runtime.dispatch_background_subagent(
        "parent",
        "box",
        "worker",
        "generalPurpose",
        "",
        "Long running task",
        None,
        None,
        10,
    );
    let settled =
        runtime.settle_background_subagent_turn("worker", RunOutcome::Completed("   ".into()), 20);
    let completion = settled.completion.expect("completion");
    assert_eq!(completion.status, "completed");
    assert_eq!(
        completion.result,
        "(the task finished without producing any text output)"
    );
}

#[test]
fn adapter_enforces_resume_and_single_computer_session_rules() {
    let mut adapter = SandSubagentHostAdapter::default();
    let first = adapter
        .create_or_resume_session(&SubagentAdapterArgs {
            subagent_type: "computerUse".into(),
            tool_call_id: "call-1".into(),
            prompt: "Inspect".into(),
            ..SubagentAdapterArgs::default()
        })
        .expect("first computer session");
    let second = adapter.create_or_resume_session(&SubagentAdapterArgs {
        subagent_type: "computer_use".into(),
        prompt: "Inspect another".into(),
        ..SubagentAdapterArgs::default()
    });
    assert!(second.is_err());

    let dispatch = adapter
        .run_session(
            &first,
            &SubagentAdapterArgs {
                subagent_type: String::new(),
                tool_call_id: "call-1".into(),
                prompt: "Inspect".into(),
                selected_videos: vec![json!({"id":"video"})],
                ..SubagentAdapterArgs::default()
            },
            Some("req-parent"),
            Some("req-root"),
            None,
        )
        .expect("dispatch");
    assert_eq!(dispatch.subagent_type, "generalPurpose");
    assert_eq!(dispatch.inference_request_id.as_deref(), Some("subagent:call-1"));
    assert_eq!(
        dispatch.lineage.unwrap().root_parent_request_id.as_deref(),
        Some("req-root")
    );

    let running_resume = adapter.create_or_resume_session(&SubagentAdapterArgs {
        resume_agent_id: Some(first.clone()),
        subagent_type: "computerUse".into(),
        ..SubagentAdapterArgs::default()
    });
    assert!(running_resume.is_err());
    adapter.mark_settled(&first);
    adapter.release_session(&first);
    assert!(!adapter.has_session(&first));
}

#[test]
fn denied_launch_releases_session_and_request_context_is_fail_closed() {
    let mut adapter = SandSubagentHostAdapter::default();
    let id = adapter
        .create_or_resume_session(&SubagentAdapterArgs {
            subagent_type: "generalPurpose".into(),
            ..SubagentAdapterArgs::default()
        })
        .expect("session");
    let denied = adapter.run_session(
        &id,
        &SubagentAdapterArgs::default(),
        None,
        None,
        Some(&LaunchReview {
            allowed: false,
            reason: "blocked".into(),
        }),
    );
    assert_eq!(denied.unwrap_err(), "blocked");
    assert!(!adapter.has_session(&id));

    let executor = SandRequestContextExecutor {
        include_transcripts: false,
        auto_review_enforce_enabled: true,
    };
    let projection = executor.execute(
        RequestContextInfo {
            os_version: Some("macOS".into()),
            shell: Some("zsh".into()),
            time_zone: Some("America/Los_Angeles".into()),
            transcripts_folder: Some("/secret/transcripts".into()),
        },
        None,
        vec!["skill-a".into()],
    );
    assert_eq!(projection.agent_transcripts_folder, None);
    assert!(!projection.rules_info_complete);
    assert!(projection.smart_mode_classifier_auto_mode_enabled);
    assert_eq!(projection.agent_skills, vec!["skill-a"]);
}

#[test]
fn forwarding_adapter_projects_deltas_usage_tools_and_summary_fence() {
    let usage = forward_agent_update(
        false,
        AgentUpdate::TurnEnded(TurnEndedUsage {
            input_tokens: Some(5),
            output_tokens: Some(2),
            ..TurnEndedUsage::default()
        }),
        None,
    );
    assert_eq!(usage.updates.len(), 1);

    let pending = forward_agent_update(
        false,
        AgentUpdate::ToolCall {
            phase: "toolCallStarted".into(),
            call_id: "tool-1".into(),
            tool_call: Some(OutlineToolCall {
                case: Some("shellToolCall".into()),
                task: Some(TaskToolCall::default()),
                activity_args: Some(json!({"command":"pwd"})),
                ..OutlineToolCall::default()
            }),
            model_call_id: None,
        },
        None,
    );
    assert!(pending.surface_unresolved_pending.is_some());

    let completed = forward_agent_update(
        false,
        AgentUpdate::ToolCall {
            phase: "toolCallCompleted".into(),
            call_id: "tool-1".into(),
            tool_call: Some(OutlineToolCall {
                case: Some("taskToolCall".into()),
                task: Some(TaskToolCall {
                    description: Some("Done".into()),
                    ..TaskToolCall::default()
                }),
                ..OutlineToolCall::default()
            }),
            model_call_id: None,
        },
        Some("ResolvedTool"),
    );
    assert!(completed.surface_unresolved_pending.is_none());

    assert!(
        !forward_agent_update(true, AgentUpdate::SummaryStarted, None).summary_lifecycle
    );
    assert!(
        forward_agent_update(false, AgentUpdate::SummaryCompleted, None).summary_lifecycle
    );

    let lineage =
        derive_sand_subagent_request_lineage(Some("parent"), None, "call").expect("lineage");
    assert_eq!(lineage.root_parent_request_id.as_deref(), Some("parent"));
}
