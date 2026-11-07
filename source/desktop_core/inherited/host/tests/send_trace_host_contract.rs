use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::send_trace_host::{
    BeginTurnTraceOptions, HostTrace, ParsedTraceparent, TraceContext, TraceFactory,
    TraceFactoryOptions, TraceSpan, TurnTraceOutcome, TurnTraceTypeOptions,
    add_turn_trace_event, adopt_remote_parent, begin_gateway_command_trace, begin_turn_trace,
    derive_child_traceparent, mark_turn_trace_error, mint_traceparent, parse_traceparent,
    resolve_turn_trace_outcome, resolve_turn_trace_sample_ratio, resolve_turn_trace_type,
    set_host_trace_factory, set_turn_trace_attributes, set_turn_trace_host_bundle_version,
    should_sample_send,
};
use serde_json::{Value, json};

#[derive(Default)]
struct RecordingSpan {
    attributes: Mutex<BTreeMap<String, Value>>,
    events: Mutex<Vec<(String, BTreeMap<String, Value>)>>,
    exceptions: Mutex<Vec<String>>,
    statuses: Mutex<Vec<u8>>,
    ends: Mutex<u64>,
}

impl TraceSpan for RecordingSpan {
    fn set_attribute(&self, key: &str, value: Value) {
        self.attributes.lock().unwrap().insert(key.into(), value);
    }
    fn add_event(&self, name: &str, attributes: &BTreeMap<String, Value>) {
        self.events
            .lock()
            .unwrap()
            .push((name.to_string(), attributes.clone()));
    }
    fn record_exception(&self, error: &str) {
        self.exceptions.lock().unwrap().push(error.into());
    }
    fn set_status(&self, code: u8) {
        self.statuses.lock().unwrap().push(code);
    }
    fn end(&self) {
        *self.ends.lock().unwrap() += 1;
    }
}

#[test]
fn traceparent_and_turn_resolution_match_the_frozen_contract() {
    let valid = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
    assert_eq!(
        parse_traceparent(valid),
        Some(ParsedTraceparent {
            trace_id: "0123456789abcdef0123456789abcdef".into(),
            span_id: "0123456789abcdef".into(),
            trace_flags: 1,
        })
    );
    assert!(parse_traceparent("00-00000000000000000000000000000000-0123456789abcdef-01").is_none());
    assert!(parse_traceparent("00-0123456789ABCDEF0123456789ABCDEF-0123456789abcdef-01").is_none());
    assert!(parse_traceparent("01-0123456789abcdef0123456789abcdef-0123456789abcdef-01").is_none());

    let minted = mint_traceparent(true);
    assert!(parse_traceparent(&minted.traceparent).is_some());
    let (child, child_span) = derive_child_traceparent(valid).expect("child");
    let parsed_child = parse_traceparent(&child).expect("parse child");
    assert_eq!(parsed_child.trace_id, "0123456789abcdef0123456789abcdef");
    assert_eq!(parsed_child.span_id, child_span);
    assert_eq!(parsed_child.trace_flags & 1, 1);

    assert!(!should_sample_send(0.0, 0.0));
    assert!(should_sample_send(1.0, 0.999));
    assert!(should_sample_send(0.5, 0.49));
    assert!(!should_sample_send(0.5, 0.5));
    assert_eq!(resolve_turn_trace_sample_ratio(None), 1.0);
    assert_eq!(resolve_turn_trace_sample_ratio(Some("0.25")), 0.25);
    assert_eq!(resolve_turn_trace_sample_ratio(Some("2")), 1.0);
    assert_eq!(resolve_turn_trace_sample_ratio(Some("NaN")), 1.0);

    let wake = json!({"id":"automation"});
    assert_eq!(
        resolve_turn_trace_type(TurnTraceTypeOptions {
            automation_wake: Some(&wake),
            request_source: Some("composer"),
            hidden: true,
        }),
        "automation"
    );
    assert_eq!(
        resolve_turn_trace_type(TurnTraceTypeOptions {
            automation_wake: None,
            request_source: Some("group-member"),
            hidden: true,
        }),
        "group-member"
    );
    assert_eq!(
        resolve_turn_trace_type(TurnTraceTypeOptions {
            automation_wake: None,
            request_source: Some("turn"),
            hidden: true,
        }),
        "hidden"
    );
    assert_eq!(
        resolve_turn_trace_outcome(&TurnTraceOutcome {
            quiesced_for_upgrade: true,
            aborted: true,
            awaiting_user_selection: true,
        }),
        "quiesced_for_upgrade"
    );
}

#[test]
fn host_factory_adopts_remote_parents_and_turn_spans_are_fail_closed() {
    let calls = Arc::new(Mutex::new(Vec::<(String, Option<String>, bool)>::new()));
    let spans = Arc::new(Mutex::new(Vec::<Arc<RecordingSpan>>::new()));
    let calls_for_factory = Arc::clone(&calls);
    let spans_for_factory = Arc::clone(&spans);
    let factory: TraceFactory = Arc::new(move |options: TraceFactoryOptions| {
        calls_for_factory.lock().unwrap().push((
            options.name,
            options.traceparent,
            options.parent_ctx.is_some(),
        ));
        let span = Arc::new(RecordingSpan::default());
        spans_for_factory.lock().unwrap().push(Arc::clone(&span));
        let context: TraceContext = Arc::new(String::from("ctx")) as Arc<dyn Any + Send + Sync>;
        Some(HostTrace {
            span,
            context: Some(context),
            trace_id: Some("0123456789abcdef0123456789abcdef".into()),
            span_id: Some("0123456789abcdef".into()),
        })
    });
    set_host_trace_factory(factory);
    set_turn_trace_host_bundle_version(Some(" 1.2.75 "));

    assert!(adopt_remote_parent(Some("garbage"), "bad").is_none());
    let remote = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
    assert!(begin_gateway_command_trace(Some(remote), "sendPrompt").is_some());

    let parent: TraceContext = Arc::new(String::from("remote-context")) as Arc<dyn Any + Send + Sync>;
    let trace = begin_turn_trace(BeginTurnTraceOptions {
        conversation_id: "agent-1".into(),
        turn_type: "user".into(),
        traceparent: None,
        parent_ctx: Some(parent),
        start_time: Some(42.0),
        sample_ratio: Some(0.0),
        attributes: BTreeMap::from([("sand.custom".into(), json!("value"))]),
    })
    .expect("parent context bypasses root sampling");

    set_turn_trace_attributes(
        Some(&trace),
        &BTreeMap::from([("sand.extra".into(), json!(7))]),
    );
    add_turn_trace_event(
        Some(&trace),
        "first_token",
        &BTreeMap::from([("chunk_type".into(), json!("text"))]),
    );
    mark_turn_trace_error(Some(&trace), "boom");

    let recorded = spans.lock().unwrap();
    let turn = recorded.last().unwrap();
    let attrs = turn.attributes.lock().unwrap();
    assert_eq!(attrs["sand.conversation_id"], "agent-1");
    assert_eq!(attrs["sand.turn_type"], "user");
    assert_eq!(attrs["sand.host_bundle_version"], "1.2.75");
    assert_eq!(attrs["sand.custom"], "value");
    assert_eq!(attrs["sand.extra"], 7);
    assert_eq!(attrs["sand.outcome"], "error");
    drop(attrs);
    assert_eq!(turn.exceptions.lock().unwrap().as_slice(), &["boom"]);
    assert_eq!(turn.statuses.lock().unwrap().as_slice(), &[2]);
    assert_eq!(
        turn.events.lock().unwrap().as_slice(),
        &[(
            "first_token".to_string(),
            BTreeMap::from([("chunk_type".into(), json!("text"))]),
        )]
    );
    drop(recorded);

    let remote_turn = begin_turn_trace(BeginTurnTraceOptions {
        conversation_id: "agent-remote".into(),
        turn_type: "user".into(),
        traceparent: Some(remote.into()),
        parent_ctx: None,
        start_time: None,
        sample_ratio: Some(0.0),
        attributes: BTreeMap::new(),
    })
    .expect("valid remote traceparent bypasses local root sampling");
    assert_eq!(remote_turn.trace_id.as_deref(), Some("0123456789abcdef0123456789abcdef"));

    let calls = calls.lock().unwrap();
    assert!(calls.iter().any(|(name, traceparent, _)| {
        name == "sand.gateway.sendPrompt" && traceparent.as_deref() == Some(remote)
    }));
    assert!(calls.iter().any(|(name, _, has_parent)| {
        name == "sand.turn.run" && *has_parent
    }));
}


#[test]
fn completed_span_api_preserves_explicit_start_and_end_contract() {
    const SOURCE: &str = include_str!("../src/send_trace_host.rs");
    assert!(SOURCE.contains("fn end_at(&self, _end_time_ms: f64)"));
    assert!(SOURCE.contains("pub fn record_completed_trace_span("));
    assert!(SOURCE.contains("start_time: Some(start_time_ms.max(0.0))"));
    assert!(SOURCE.contains("child.span.end_at(end_time_ms.max(start_time_ms).max(0.0))"));
}
