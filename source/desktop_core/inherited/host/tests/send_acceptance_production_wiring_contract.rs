const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const SESSION_GATEWAY: &str = include_str!("../src/extensions/session/gateway.rs");
const TRANSCRIPT_RUNTIME: &str = include_str!("../src/extensions/transcript/production_runtime.rs");
const SEND_ACCEPTANCE: &str = include_str!("../src/extensions/transcript/send_acceptance.rs");

#[test]
fn shipping_send_acceptance_projects_durable_echoes_after_unlock_and_before_dispatch() {
    for needle in [
        "accepted_echoes: staged",
        "acceptance_effects_applied: true",
        "accepted_durably: true",
        "needs_roster_refresh",
    ] {
        assert!(SESSION_GATEWAY.contains(needle), "missing durable acceptance projection: {needle}");
    }

    let unlock = TRANSCRIPT_RUNTIME.find("drop(state);\n        on_accepted(&persisted_send_context);")
        .expect("acceptance observer must run after releasing Transcript runtime state");
    let dispatch = TRANSCRIPT_RUNTIME.find("dispatch()")
        .expect("shipping provider dispatch");
    assert!(unlock < dispatch, "durable acceptance projection must precede provider dispatch");

    for needle in [
        "execute_send_with_acceptance_observer(",
        "mark_accepted_echoes_on_active_transcript",
        "emit_persisted_send_acceptance(",
        "self.trays.clear_for_agent(agent_id)",
        "emit_accepted_send_echoes(",
        "roster_emit: Arc<ProductionRosterEmit>",
    ] {
        assert!(SHIPPING_HOST.contains(needle), "shipping Host missing send-acceptance owner wiring: {needle}");
    }

    for needle in [
        "\"channel\": \"transcript\"",
        "\"type\": \"appended\"",
        "direct_addressed_acceptance",
        "roster.emit_agent_update(agent_id)",
        "has_offscreen_entries",
    ] {
        assert!(SEND_ACCEPTANCE.contains(needle), "missing frozen accepted-echo behavior: {needle}");
    }
}


#[test]
fn shipping_send_acceptance_uses_gateway_context_for_frozen_completed_spans() {
    const TRACE_HOST: &str = include_str!("../src/send_trace_host.rs");
    const OTLP: &str = include_str!("../src/extensions/telemetry/host_tracing.rs");

    for needle in [
        "call_accept_routed_prompt(args, Some(context))",
        "call_send_prompt(args, Some(context))",
        "begin_send_trace(",
        "context.traceparent.as_deref()",
        "context.dispatch_started.elapsed().as_secs_f64() * 1_000.0",
        "record_send_acceptance_tracing(",
        "\"durable-append\"",
        "\"send-ack-emit\"",
        "\"sand.durable_append_ms\"",
        "\"sand.durable\"",
        "\"sand.conversation_id\"",
        "\"sand.client_nonce\"",
        "\"sand.ack_emit_host_ms\"",
    ] {
        assert!(SHIPPING_HOST.contains(needle), "missing frozen send trace wiring: {needle}");
    }
    assert!(!SHIPPING_HOST.contains("\"traceparent\": context.traceparent"));
    assert!(TRACE_HOST.contains("record_completed_trace_span"));
    assert!(TRACE_HOST.contains("end_at(end_time_ms.max(start_time_ms).max(0.0))"));
    assert!(OTLP.contains("fn end_at(&self, end_time_ms: f64)"));
    assert!(OTLP.contains("(end_time_ms * 1_000_000.0).round() as u128"));
}
