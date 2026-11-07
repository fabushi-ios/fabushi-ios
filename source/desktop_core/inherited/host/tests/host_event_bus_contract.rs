use std::sync::{Arc, Mutex};
use std::time::Duration;

use mahayana_host_runtime::host_event_bus::{
    HostEventFailureMode, SandHostEventBus, pin_host_event_failure_reporter,
};
use serde_json::{Value, json};

#[test]
fn shipping_stream_and_callback_fanout_match_frozen_event_bus_contract() {
    let bus = SandHostEventBus::default();
    let receiver = bus.subscribe();
    let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&seen);
    let subscription = bus.subscribe_listener(move |event| {
        sink.lock().expect("listener sink").push(event.clone());
    });

    let event = json!({"channel":"runner","payload":{"type":"delta"}});
    bus.publish(event.clone());

    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(1)).expect("stream event"),
        event
    );
    assert_eq!(
        seen.lock().expect("callback values").as_slice(),
        &[event.clone()]
    );
    assert_eq!(bus.subscriber_count(), 1);

    subscription.unsubscribe();
    bus.publish(json!({"channel":"runner","payload":{"type":"done"}}));
    assert_eq!(seen.lock().expect("callback values").len(), 1);
}

#[test]
fn listener_and_topic_failures_are_isolated_reported_and_rejectable() {
    let failures = Arc::new(Mutex::new(Vec::<Value>::new()));
    let failure_sink = Arc::clone(&failures);
    let bus = SandHostEventBus::with_failure_reporter(Arc::new(move |failure| {
        failure_sink
            .lock()
            .expect("failure sink")
            .push(failure.clone());
    }));

    let _panic = bus.subscribe_listener(|_| panic!("listener boom"));
    let healthy = Arc::new(Mutex::new(0usize));
    let healthy_sink = Arc::clone(&healthy);
    let _healthy = bus.subscribe_listener(move |_| {
        *healthy_sink.lock().expect("healthy listener") += 1;
    });
    bus.publish(json!({"type":"event"}));
    assert_eq!(*healthy.lock().expect("healthy listener"), 1);

    let handled = Arc::new(Mutex::new(0usize));
    let handled_sink = Arc::clone(&handled);
    let _ok = bus.on("capability", move |_| {
        *handled_sink.lock().expect("topic count") += 1;
        Ok(())
    });
    let _failed = bus.on("capability", |_| Err("denied".into()));

    assert!(bus
        .emit_topic(
            "capability",
            &json!({"request":1}),
            HostEventFailureMode::Continue,
        )
        .is_ok());
    assert_eq!(*handled.lock().expect("topic count"), 1);

    let error = bus
        .emit_topic(
            "capability",
            &json!({"request":2}),
            HostEventFailureMode::Reject,
        )
        .expect_err("reject mode");
    assert_eq!(error.topic, "capability");
    assert_eq!(error.message, "denied");

    let failures = failures.lock().expect("failures");
    assert!(failures.iter().any(|failure| {
        failure.get("kind").and_then(Value::as_str) == Some("listener_failed")
    }));
    assert!(failures.iter().any(|failure| {
        failure.get("kind").and_then(Value::as_str) == Some("subscriber_failed")
            && failure.get("topic").and_then(Value::as_str) == Some("capability")
    }));
}


#[test]
fn default_shipping_bus_forwards_failures_through_the_pinned_reporter() {
    let failures = Arc::new(Mutex::new(Vec::<Value>::new()));
    let failure_sink = Arc::clone(&failures);
    pin_host_event_failure_reporter(Some(Arc::new(move |failure| {
        failure_sink
            .lock()
            .expect("pinned failure sink")
            .push(failure.clone());
    })));

    let bus = SandHostEventBus::default();
    let _panic = bus.subscribe_listener(|_| panic!("listener boom"));
    bus.publish(json!({"type":"event"}));
    let _failed = bus.on("capability", |_| Err("denied".into()));
    bus.emit_topic(
        "capability",
        &json!({"request":1}),
        HostEventFailureMode::Continue,
    )
    .expect("continue mode");

    pin_host_event_failure_reporter(None);
    let failures = failures.lock().expect("pinned failures");
    assert!(failures.iter().any(|failure| {
        failure.get("kind").and_then(Value::as_str) == Some("listener_failed")
            && failure.get("topic").is_none()
            && failure.get("errorClass").and_then(Value::as_str) == Some("panic")
    }));
    assert!(failures.iter().any(|failure| {
        failure.get("kind").and_then(Value::as_str) == Some("subscriber_failed")
            && failure.get("topic").and_then(Value::as_str) == Some("capability")
            && failure.get("errorClass").and_then(Value::as_str) == Some("Error")
    }));
}
