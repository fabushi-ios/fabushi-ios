const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const INFERENCE_PRODUCTION: &str = include_str!("../src/extensions/inference/production.rs");
const TRANSCRIPT_RUNTIME: &str = include_str!("../src/extensions/transcript/production_runtime.rs");
const TURN_OBSERVATION: &str = include_str!("../src/runner/turn_observation.rs");

#[test]
fn shipping_host_wires_frozen_turn_telemetry_sources() {
    for needle in [
        "let fields = UserMessageReceivedFields {",
        ".report_user_message_received(&fields)",
        "send_was_in_flight",
        "self.transcript_runtime.is_agent_running(agent_id)",
        "let fields = TurnInterruptFields {",
        ".report_turn_interrupt(&fields)",
        "reason: \"watchdog\".into()",
        "watchdog_transcript_runtime.is_agent_running(&event.agent_id)",
        "interrupt_wedged_run_for_watchdog(&event.agent_id)",
        "worker_transcript_runtime.settle_turn_usage(",
        "let usage_fields = TurnUsageFields {",
        ".report_turn_usage(&usage_fields)",
        "ProviderTokenUsage",
        "merge_provider_token_usage",
        "usage_sink: Some(usage_sink)",
        "worker_provider_usage",
        "let fields = ComputerUseUsageFields {",
        ".report_computer_use_usage(&fields)",
        "settled.computer_use_usage",
        "usage_inference.record_usage(",
        "InferenceUsage {",
        "usage_provider",
        "TelemetryTokenUsage",
        "GatewayCommandContext",
        "call_with_context(",
        "let turn_trace = begin_turn_trace(BeginTurnTraceOptions {",
        "traceparent: worker_gateway_context",
        ".and_then(|context| context.traceparent.clone())",
        "let ttft_trace_id = turn_trace",
        "let ttft_span_id = turn_trace",
        "trace_id: ttft_trace_id.clone()",
        "span_id: ttft_span_id.clone()",
        "dispatch_started.elapsed().as_secs_f64()",
        "observation.set_first_token_handler(",
        "let fields = TtftFields {",
        ".report_ttft(&fields)",
        "reason: \"agent_deleted\".into()",
        "let was_in_flight = transcript_runtime.is_agent_running(agent_id)",
        "runner_registry.cancel_agent(agent_id, \"agent deleted\") > 0",
        "self.runner_registry.agent_id_for_stream(stream_id)",
        "reason.to_ascii_lowercase().contains(\"superseded\")",
        "reason: \"superseded\".into()",
        "had_active_run: cancelled",
    ] {
        assert!(
            SHIPPING_HOST.contains(needle),
            "shipping Host must preserve frozen turn telemetry wiring through the typed Host facade: {needle}"
        );
    }

    for needle in [
        "self.settings.record_inference_usage(",
        "provider.as_str()",
        "inference_usage_persist_failed",
    ] {
        assert!(
            INFERENCE_PRODUCTION.contains(needle),
            "Host Inference owner must preserve routed-provider usage persistence: {needle}"
        );
    }

    assert!(
        TRANSCRIPT_RUNTIME.contains("state.lifecycle.track_turn_request_id(agent_id, operation_id)"),
        "Transcript runtime must own per-turn request-id tracking"
    );
}

#[test]
fn shipping_turn_telemetry_uses_typed_host_facade_and_closing_send_delivery_owner() {
    const OWNER: &str =
        include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    const SEND_TOOL: &str =
        include_str!("../src/runner/tools/send_message_tool.rs");
    const COORDINATOR: &str =
        include_str!("../../node-agent-coordinator/src/main.rs");
    const ELECTRON: &str =
        include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");

    for method in [
        "pub fn report_turn_interrupt(",
        "pub fn report_turn_await(",
        "pub fn report_turn_retry(",
        "pub fn report_user_message_received(",
        "pub fn report_closing_send_nudge(",
        "pub fn report_ttft(",
        "pub fn report_turn_usage(",
        "pub fn report_computer_use_usage(",
    ] {
        assert!(OWNER.contains(method), "typed Host owner missing {method}");
    }

    for call in [
        ".report_turn_interrupt(&fields)",
        ".report_turn_await(&fields)",
        ".report_turn_retry(&fields)",
        ".report_user_message_received(&fields)",
        ".report_closing_send_nudge(&closing_send_fields)",
        ".report_ttft(&fields)",
        ".report_turn_usage(&usage_fields)",
        ".report_computer_use_usage(&fields)",
    ] {
        assert!(SHIPPING_HOST.contains(call), "shipping producer missing typed call {call}");
    }

    assert!(SEND_TOOL.contains("if result.is_ok()"));
    assert!(SEND_TOOL.contains("self.counter.record_success()"));
    assert!(SHIPPING_HOST.contains("let sent_message_count = send_message_delivery_counter.count()"));
    assert!(SHIPPING_HOST.contains("let reacted = reaction_delivery_counter.count() > 0"));
    assert!(SHIPPING_HOST.contains("let terminal_projection = project_turn_terminal("));
    assert!(SHIPPING_HOST.contains("sent_message_count,"));
    assert!(SHIPPING_HOST.contains("reacted,"));
    assert!(SHIPPING_HOST.contains("delivered: !is_delivery_owed("));
    assert!(SHIPPING_HOST.contains("reaction_delivery_counter.count() > 0"));
    assert!(SHIPPING_HOST.contains("aborted: worker_cancellation.is_cancelled()"));

    for event in [
        "sand.turn.interrupt",
        "sand.turn.await",
        "sand.turn.retry",
        "sand.user_message.received",
        "sand.turn.closing_send_nudge",
        "sand.ttft",
        "sand.turn.usage",
    ] {
        assert!(!COORDINATOR.contains(event), "Coordinator must not own {event}");
        assert!(!ELECTRON.contains(event), "Electron must not own {event}");
    }
}

#[test]
fn shipping_ttft_and_usage_provenance_are_live_not_synthetic() {
    for needle in [
        "let turn_trace = begin_turn_trace(BeginTurnTraceOptions {",
        "traceparent: worker_gateway_context",
        ".and_then(|context| context.traceparent.clone())",
        "let ttft_trace_id = turn_trace",
        "let ttft_span_id = turn_trace",
        "trace_id: ttft_trace_id.clone()",
        "span_id: ttft_span_id.clone()",
        "let dispatch_perf_ms = worker_gateway_context",
        ".map(|context| context.dispatch_started.elapsed().as_secs_f64() * 1_000.0)",
        "observation.observe_send_dispatch(",
        "observation.set_first_token_handler(",
        "worker_provider_usage = Arc::new(Mutex::new(None::<ProviderTokenUsage>))",
        "usage_sink: Some(usage_sink)",
        "*stored = Some(merge_provider_token_usage(stored.take(), usage))",
        "worker_transcript_runtime.settle_turn_usage(",
        "usage: worker_provider_usage",
    ] {
        assert!(SHIPPING_HOST.contains(needle), "missing live provenance: {needle}");
    }

    for needle in [
        "self.first_output_dispatch_started = Some(Instant::now())",
        "pub fn observe_stream_output(",
        ".map(|started| started.elapsed().as_secs_f64() * 1_000.0)",
        "self.observe_first_token(",
    ] {
        assert!(
            TURN_OBSERVATION.contains(needle),
            "TurnObservation must own live dispatch-to-first-output TTFT provenance: {needle}"
        );
    }
}
