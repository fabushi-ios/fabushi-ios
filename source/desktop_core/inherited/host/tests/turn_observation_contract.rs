use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::transcript::async_task_union::AsyncTask;
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::turn_observation::{
    McpObservedRoutedToolBridge, ObservedRoutedToolBridge, RECENT_ACTIVITY_CAP, ToolActivity,
    ToolCallTelemetryEvent, TurnObservation, async_tasks_changed_event,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Delegate;

impl RoutedToolBridge for Delegate {
    fn list_tools(
        &self,
    ) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if args.get("fail").and_then(Value::as_bool) == Some(true) {
            Err(ProviderSessionError::Tool("boom".into()))
        } else {
            Ok(json!({"status":"done","value":"ok"}))
        }
    }
}

fn tool(name: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: "test".into(),
        tool_name: name.into(),
        description: None,
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn async_tasks_are_sorted_and_emit_the_frozen_owner_envelope() {
    let emitted = Arc::new(Mutex::new(Vec::<Value>::new()));
    let emitted_sink = Arc::clone(&emitted);
    let mut observation = TurnObservation::new("agent-async", None);
    observation.set_async_tasks_provider(Arc::new(|owner| {
        assert_eq!(owner, "agent-async");
        vec![
            AsyncTask {
                kind: "shell".into(),
                id: "shell-b".into(),
                label: "Later".into(),
                status: "running".into(),
                started_at_ms: 20.0,
                detail: None,
                subagent_type: None,
            },
            AsyncTask {
                kind: "subagent".into(),
                id: "sub-a".into(),
                label: "Earlier".into(),
                status: "running".into(),
                started_at_ms: 10.0,
                detail: Some("research".into()),
                subagent_type: Some("research".into()),
            },
        ]
    }));
    observation.set_async_tasks_event_handler(Arc::new(move |event| {
        emitted_sink.lock().expect("events").push(event);
    }));

    let tasks = observation
        .emit_async_tasks_changed()
        .expect("async tasks event");
    assert_eq!(
        tasks.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
        vec!["sub-a", "shell-b"]
    );
    let events = emitted.lock().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["parentAgentId"], "agent-async");
    assert_eq!(events[0]["tasks"][0]["id"], "sub-a");
    assert_eq!(events[0]["tasks"][0]["subagentType"], "research");
}

#[test]
fn async_task_event_rejects_an_unowned_conversation() {
    let error = async_tasks_changed_event("", &[])
        .expect_err("empty owner must fail closed");
    assert_eq!(error, "async-task owner conversation id is unavailable");
}

#[test]
fn activity_dedupes_caps_and_counts_terminal_tool_calls() {
    let mut observation = TurnObservation::new("agent-a", None);
    observation.record_tool_activity(ToolActivity {
        status: "pending".into(),
        name: "Read".into(),
        summary: None,
    });
    observation.record_tool_activity(ToolActivity {
        status: "pending".into(),
        name: "Read".into(),
        summary: None,
    });
    for index in 0..(RECENT_ACTIVITY_CAP + 5) {
        observation.record_tool_activity(ToolActivity {
            status: "done".into(),
            name: format!("Tool{index}"),
            summary: Some("ok".into()),
        });
    }
    assert_eq!(observation.recent_activity().len(), RECENT_ACTIVITY_CAP);
    assert_eq!(
        observation.observed_tool_call_count(),
        (RECENT_ACTIVITY_CAP + 5) as u64
    );
}

#[test]
fn first_token_is_one_shot_and_snapshot_uses_turn_start() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let mut observation = TurnObservation::new(
        "agent-a",
        Some(Arc::new(move |event| {
            sink_events.lock().unwrap().push(event);
        })),
    );
    let first_token_events = Arc::new(Mutex::new(Vec::new()));
    let first_token_sink_events = Arc::clone(&first_token_events);
    observation.set_first_token_handler(Arc::new(move |event| {
        first_token_sink_events.lock().unwrap().push(event);
    }));
    observation.turn_started(100);
    assert!(observation.observe_first_token(
        "text",
        Some(10.0),
        Some(25.0),
        Some("model-a"),
        false,
    ));
    assert!(!observation.observe_first_token(
        "text",
        Some(10.0),
        Some(30.0),
        Some("model-a"),
        false,
    ));
    assert_eq!(observation.snapshot(175).elapsed_ms, 75);
    let first_token_events = first_token_events.lock().unwrap();
    assert_eq!(first_token_events.len(), 1);
    assert_eq!(first_token_events[0]["chunkType"], "text");
    assert_eq!(first_token_events[0]["ttftMs"], 15.0);
    assert_eq!(first_token_events[0]["skew"], false);
    assert_eq!(first_token_events[0]["modelId"], "model-a");
    drop(first_token_events);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "first-token")
            .count(),
        1
    );
}

#[test]
fn tool_call_can_be_the_frozen_first_output_and_keeps_dispatch_identity() {
    let first_token_events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&first_token_events);
    let observation = TurnObservation::shared("agent-tool-first", None);
    {
        let mut observation = observation.lock().unwrap();
        observation.set_first_token_handler(Arc::new(move |event| {
            sink_events.lock().unwrap().push(event);
        }));
        observation.turn_started(100);
        observation.observe_send_dispatch(
            0.0,
            10.0,
            None,
            100.0,
            true,
            "model-tool",
        );
    }

    let bridge = ObservedRoutedToolBridge::new(
        Arc::new(Delegate),
        Arc::clone(&observation),
    );
    bridge
        .call_tool(&tool("Read"), json!({}), "call-tool-first")
        .expect("tool call");

    let events = first_token_events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["chunkType"], "tool-call");
    assert_eq!(events[0]["modelId"], "model-tool");
    assert_eq!(events[0]["isFork"], true);
    assert!(events[0]["ttftMs"].as_f64().is_some());
}

#[test]
fn observed_bridge_wraps_success_failure_and_await_lifecycle() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let observation = TurnObservation::shared(
        "agent-a",
        Some(Arc::new(move |event| {
            sink_events.lock().unwrap().push(event);
        })),
    );
    let await_events = Arc::new(Mutex::new(Vec::new()));
    let await_sink_events = Arc::clone(&await_events);
    observation
        .lock()
        .unwrap()
        .set_turn_await_handler(Arc::new(move |event| {
            await_sink_events.lock().unwrap().push(event);
        }));
    let bridge = ObservedRoutedToolBridge::new(
        Arc::new(Delegate),
        Arc::clone(&observation),
    );

    bridge
        .call_tool(
            &tool("AwaitShell"),
            json!({"block_until_ms":123}),
            "call-1",
        )
        .expect("await tool");
    assert!(bridge
        .call_tool(&tool("Read"), json!({"fail":true}), "call-2")
        .is_err());

    let observation = observation.lock().unwrap();
    assert_eq!(observation.observed_tool_call_count(), 2);
    assert!(observation
        .recent_activity()
        .iter()
        .any(|line| line == "[failed] Read"));
    drop(observation);

    let events = events.lock().unwrap();
    assert!(events.iter().any(|event| event["type"] == "turn-await"));
    assert!(events.iter().any(|event| {
        event["type"] == "tool-completed" && event["failed"] == true
    }));
    drop(events);

    let await_events = await_events.lock().unwrap();
    assert_eq!(await_events.len(), 1);
    assert_eq!(await_events[0]["awaitIndex"], 1);
    assert_eq!(await_events[0]["blockUntilMs"], 123);
    assert_eq!(await_events[0]["outcome"], "completed");
}


#[test]
fn pending_awaits_flush_on_turn_unwind_with_frozen_clean_stop_and_abort_outcomes() {
    let emitted = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&emitted);
    let mut observation = TurnObservation::new("agent-await-unwind", None);
    observation.set_turn_await_handler(Arc::new(move |event| {
        sink.lock().unwrap().push(event);
    }));

    observation.observe_await_tool_call(
        true,
        "await-clean",
        101,
        None,
        false,
        false,
    );
    observation.flush_pending_awaits_on_unwind(false);

    observation.observe_await_tool_call(
        true,
        "await-abort",
        202,
        None,
        false,
        false,
    );
    observation.flush_pending_awaits_on_unwind(true);

    let emitted = emitted.lock().unwrap();
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0]["awaitIndex"], 1);
    assert_eq!(emitted[0]["blockUntilMs"], 101);
    assert_eq!(emitted[0]["outcome"], "clean_stop");
    assert_eq!(emitted[1]["awaitIndex"], 1);
    assert_eq!(emitted[1]["blockUntilMs"], 202);
    assert_eq!(emitted[1]["outcome"], "aborted");
}

#[test]
fn shipping_host_removes_aborting_subagents_from_async_task_projection_immediately() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    for required in [
        ".set_abort_observer(Some(Arc::new(",
        "PendingWakeKind::Subagent",
        "pending_store.clear_if_matches(",
        "publish_async_tasks_changed",
    ] {
        assert!(
            main.contains(required),
            "missing abort-to-async-task production wiring: {required}"
        );
    }
}

#[test]
fn shipping_host_flushes_pending_awaits_at_the_real_turn_unwind_boundary() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    assert!(
        main.contains("observation.flush_pending_awaits_on_unwind(turn_interrupted);"),
        "shipping provider turn must settle pending awaits on unwind"
    );
    assert!(
        main.contains("RUN_DIRECT_USER_INTERRUPT_REASON | RUN_WATCHDOG_INTERRUPT_REASON"),
        "shipping unwind must derive interrupted outcome from canonical runner interrupt reasons"
    );
}

#[test]
fn send_dispatch_handler_receives_the_sanitized_shipping_payload() {
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&dispatched);
    let mut observation = TurnObservation::new("agent-dispatch", None);
    observation.set_send_dispatch_handler(Arc::new(move |event| {
        sink.lock().unwrap().push(event);
    }));
    observation.observe_send_dispatch(100.0, 115.4, Some(1_000.0), 1_025.2, false, "model-a");
    let dispatched = dispatched.lock().unwrap();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0]["type"], "send-dispatch");
    assert_eq!(dispatched[0]["agentId"], "agent-dispatch");
    assert_eq!(dispatched[0]["hostDispatchMs"], 15);
    assert_eq!(dispatched[0]["dispatchMs"], 25.0);
    assert_eq!(dispatched[0]["skew"], false);
    assert_eq!(dispatched[0]["isFork"], false);
    assert_eq!(dispatched[0]["modelId"], "model-a");
}


#[test]
fn observed_bridge_emits_frozen_started_and_error_tool_call_facts() {
    let observation = TurnObservation::shared("agent-tool", None);
    let telemetry = Arc::new(Mutex::new(Vec::<ToolCallTelemetryEvent>::new()));
    let telemetry_sink = Arc::clone(&telemetry);
    {
        let mut observation = observation.lock().unwrap();
        observation.set_request_id(Some("request-tool".into()));
        observation.set_tool_call_telemetry_handler(Arc::new(move |event| {
            telemetry_sink.lock().unwrap().push(event);
        }));
    }
    let bridge = ObservedRoutedToolBridge::new(Arc::new(Delegate), Arc::clone(&observation));

    bridge
        .call_tool(&tool("Shell"), json!({}), "call-started")
        .expect("dual-surface tool");
    assert!(
        bridge
            .call_tool(&tool("Read"), json!({"fail":true}), "call-failed")
            .is_err()
    );

    let telemetry = telemetry.lock().unwrap();
    assert!(telemetry.iter().any(|event| matches!(
        event,
        ToolCallTelemetryEvent::Started {
            conversation_id,
            request_id,
            tool_name,
            tool_call_id,
            surface,
        } if conversation_id == "agent-tool"
            && request_id.as_deref() == Some("request-tool")
            && tool_name == "Shell"
            && tool_call_id == "call-started"
            && surface == "box"
    )));
    assert!(telemetry.iter().any(|event| matches!(
        event,
        ToolCallTelemetryEvent::Error {
            conversation_id,
            request_id,
            tool_name,
            tool_call_id,
            error_class,
            connector,
            ..
        } if conversation_id == "agent-tool"
            && request_id.as_deref() == Some("request-tool")
            && tool_name == "Read"
            && tool_call_id == "call-failed"
            && error_class == "task_error_result"
            && connector == "unknown"
    )));
    assert_eq!(
        telemetry
            .iter()
            .filter(|event| matches!(event, ToolCallTelemetryEvent::Started { .. }))
            .count(),
        2,
        "both Shell and Read are frozen dual-surface box tools"
    );
}


#[test]
fn mcp_exec_observation_reports_stall_then_error_settlement() {
    let mut observation = TurnObservation::new("agent-mcp", None);
    observation.set_request_id(Some("request-mcp".into()));
    let telemetry = Arc::new(Mutex::new(Vec::<ToolCallTelemetryEvent>::new()));
    let telemetry_sink = Arc::clone(&telemetry);
    observation.set_tool_call_telemetry_handler(Arc::new(move |event| {
        telemetry_sink.lock().unwrap().push(event);
    }));

    let guard = observation
        .begin_mcp_exec_observation_with_threshold(
            "mcp-call-1",
            "github",
            Duration::from_millis(5),
        )
        .expect("mcp observation");
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let stalled = {
            let telemetry = telemetry.lock().unwrap();
            telemetry.iter().any(|event| matches!(
                event,
                ToolCallTelemetryEvent::Stalled {
                    conversation_id,
                    request_id,
                    tool_name,
                    tool_call_id,
                    connector,
                    ..
                } if conversation_id == "agent-mcp"
                    && request_id.as_deref() == Some("request-mcp")
                    && tool_name == "mcpToolCall"
                    && tool_call_id == "mcp-call-1"
                    && connector == "github"
            ))
        };
        if stalled {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "MCP stall telemetry was not emitted within the bounded contract window"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    guard.settle(Some("ConnectError.Unavailable"));
    let telemetry = telemetry.lock().unwrap();
    assert!(telemetry.iter().any(|event| matches!(
        event,
        ToolCallTelemetryEvent::Error {
            conversation_id,
            request_id,
            tool_name,
            tool_call_id,
            error_class,
            connector,
            ..
        } if conversation_id == "agent-mcp"
            && request_id.as_deref() == Some("request-mcp")
            && tool_name == "mcpToolCall"
            && tool_call_id == "mcp-call-1"
            && error_class == "ConnectError.Unavailable"
            && connector == "github"
    )));
}


#[test]
fn routed_mcp_failure_reports_one_specialized_error_not_generic_duplicate() {
    let observation = TurnObservation::shared("agent-mcp-dedupe", None);
    let telemetry = Arc::new(Mutex::new(Vec::<ToolCallTelemetryEvent>::new()));
    let telemetry_sink = Arc::clone(&telemetry);
    observation
        .lock()
        .unwrap()
        .set_tool_call_telemetry_handler(Arc::new(move |event| {
            telemetry_sink.lock().unwrap().push(event);
        }));

    let base: Arc<dyn RoutedToolBridge> = Arc::new(Delegate);
    let mcp: Arc<dyn RoutedToolBridge> =
        Arc::new(McpObservedRoutedToolBridge::new(base, Arc::clone(&observation)));
    let observed = ObservedRoutedToolBridge::new(mcp, Arc::clone(&observation));

    assert!(
        observed
            .call_tool(&tool("RemoteMcpTool"), json!({"fail":true}), "mcp-failed")
            .is_err()
    );

    let telemetry = telemetry.lock().unwrap();
    let errors = telemetry
        .iter()
        .filter(|event| matches!(event, ToolCallTelemetryEvent::Error { .. }))
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0],
        ToolCallTelemetryEvent::Error {
            tool_call_id,
            error_class,
            ..
        } if tool_call_id == "mcp-failed" && error_class == "mcp_error_result"
    ));
}


#[test]
fn shipping_host_routes_async_task_projection_through_turn_observation_owner() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = std::fs::read_to_string(root.join("app/src/main.rs")).expect("shipping host main");
    for required in [
        "async_tasks_changed_event(",
        "set_async_tasks_provider(",
        "set_async_tasks_event_handler(",
        "publish_async_tasks_changed(",
        "publish_async_tasks_changed_from_slots(",
        "collect_live_async_tasks(",
        "running_subagent_records_for_parent(agent_id)",
        ".async_task_snapshots(agent_id)",
        "get_async_tasks(owner_agent_id, &live_tasks)",
    ] {
        assert!(
            main.contains(required),
            "missing live-owner async-task observation wiring: {required}"
        );
    }
    assert!(
        main.matches("publish_async_tasks_changed(").count() >= 5,
        "gateway, pending-wake, subagent, cloud and shell task changes must share the canonical observation envelope"
    );
}



#[test]
fn shipping_background_task_settlement_clears_durable_projection_before_change_event() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    for required in [
        "cloud_watch_settled",
        "PendingWakeKind::CloudAgent",
        "shell_watch_settled",
        "PendingWakeKind::Shell",
        "PendingWakeKind::Subagent",
    ] {
        assert!(
            main.contains(required),
            "missing production background-task settlement projection: {required}"
        );
    }

    let background_work = fs::read_to_string("src/runner/background_work.rs")
        .expect("background work owner");
    let cloud_settled = background_work
        .find("callback(CloudAgentBackgroundCompletion")
        .expect("cloud settlement callback");
    let cloud_changed = background_work[cloud_settled..]
        .find("callback(&parent_agent_id);")
        .expect("cloud terminal change callback")
        + cloud_settled;
    assert!(cloud_settled < cloud_changed);

    let shell_settled = background_work
        .find("callback(BackgroundShellBackgroundCompletion")
        .expect("shell settlement callback");
    let shell_changed = background_work[shell_settled..]
        .find("callback(&parent_agent_id);")
        .expect("shell terminal change callback")
        + shell_settled;
    assert!(shell_settled < shell_changed);
}

#[test]
fn shipping_first_output_is_armed_at_provider_dispatch_and_covers_tool_calls() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    let observation =
        fs::read_to_string("src/runner/turn_observation.rs").expect("turn observation owner");
    assert!(main.contains("observation.observe_send_dispatch("));
    assert!(main.contains("observation.observe_stream_output(\"text\")"));
    assert!(observation.contains("self.first_output_dispatch_started = Some(Instant::now())"));
    assert!(observation.contains("observation.observe_stream_output(\"tool-call\")"));
    assert!(
        !main.contains("let ttft_dispatch_started = worker_gateway_context"),
        "first-output timing must be armed at the real provider dispatch boundary"
    );
}

#[test]
fn shipping_host_owns_real_turn_trace_for_dispatch_ttft_and_first_token() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    for required in [
        "begin_turn_trace(BeginTurnTraceOptions",
        "traceparent: worker_gateway_context",
        "let send_dispatch_trace_id = turn_trace",
        "let send_dispatch_span_id = turn_trace",
        "trace_id: send_dispatch_trace_id.clone()",
        "span_id: send_dispatch_span_id.clone()",
        "\"sand.send_dispatch_ms\"",
        "\"sand.send_dispatch_skew_reason\"",
        "\"sand.send_dispatch_host_ms\"",
        "let ttft_trace_id = turn_trace",
        "let ttft_span_id = turn_trace",
        "trace_id: ttft_trace_id.clone()",
        "span_id: ttft_span_id.clone()",
        "\"sand.ttft_ms\"",
        "\"sand.ttft_skew_reason\"",
        "add_turn_trace_event(",
        "\"first_token\"",
        "\"chunk_type\"",
        "trace.span.end()",
    ] {
        assert!(
            main.contains(required),
            "missing frozen dispatch/TTFT turn-trace production wiring: {required}"
        );
    }
    assert!(
        !main.contains("let send_dispatch_trace_id = worker_gateway_context"),
        "dispatch telemetry must bind to the turn span, not merely echo the gateway parent span"
    );
}

#[test]
fn shipping_host_emits_send_dispatch_at_the_real_provider_dispatch_boundary() {
    let main = fs::read_to_string("app/src/main.rs").expect("shipping host main");
    let observation = main
        .find("observation.observe_send_dispatch(")
        .expect("send-dispatch observation producer");
    let provider_dispatch = main
        .find("runner.run_routed_provider_with_projected_messages(")
        .expect("real provider dispatch");
    assert!(
        observation < provider_dispatch,
        "send-dispatch observation must be emitted immediately before the shipping provider call"
    );
    for required in [
        "worker_enter_epoch_ms",
        "context.dispatch_started.elapsed().as_secs_f64() * 1_000.0",
        "turn_input.options.is_fork",
        "provider.as_str()",
    ] {
        assert!(
            main.contains(required),
            "missing frozen send-dispatch production input: {required}"
        );
    }
}
