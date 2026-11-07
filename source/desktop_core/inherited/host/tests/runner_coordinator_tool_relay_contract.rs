use mahayana_host_runtime::gateway_server::GatewayEventHub;
use mahayana_host_runtime::runner::coordinator_tool_relay::{
    CoordinatorToolRelay, CoordinatorToolRelayError, ROUTED_TOOL_EXECUTE_METHOD,
    ROUTED_TOOL_LIST_METHOD, RUNNER_TOOL_REQUEST_EVENT_CHANNEL,
};
use serde_json::json;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn runner_tool_request_round_trips_through_gateway_event_and_resolution() {
    let events = GatewayEventHub::default();
    let event_rx = events.subscribe();
    let relay = Arc::new(CoordinatorToolRelay::with_timeout(
        events,
        Duration::from_secs(1),
    ));
    let worker = {
        let relay = Arc::clone(&relay);
        thread::spawn(move || relay.request(ROUTED_TOOL_LIST_METHOD, json!({})))
    };

    let event = event_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("runner tool request event");
    assert_eq!(event["channel"], RUNNER_TOOL_REQUEST_EVENT_CHANNEL);
    assert_eq!(event["payload"]["method"], ROUTED_TOOL_LIST_METHOD);
    let request_id = event["payload"]["requestId"]
        .as_str()
        .expect("request id")
        .to_string();

    let ack = relay
        .resolve(&json!({
            "requestId": request_id,
            "ok": true,
            "result": [{"name":"github_search"}],
        }))
        .expect("resolve request");
    assert_eq!(ack["resolved"], true);
    assert_eq!(
        worker.join().expect("worker").expect("tool list"),
        json!([{"name":"github_search"}])
    );
    assert_eq!(relay.pending_count(), 0);
}

#[test]
fn runner_tool_request_propagates_remote_failure_and_rejects_unknown_methods() {
    let events = GatewayEventHub::default();
    let event_rx = events.subscribe();
    let relay = Arc::new(CoordinatorToolRelay::with_timeout(
        events,
        Duration::from_secs(1),
    ));
    let worker = {
        let relay = Arc::clone(&relay);
        thread::spawn(move || {
            relay.request(
                ROUTED_TOOL_EXECUTE_METHOD,
                json!({"toolName":"create_issue","args":{"title":"x"}}),
            )
        })
    };

    let event = event_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("runner tool request event");
    let request_id = event["payload"]["requestId"]
        .as_str()
        .expect("request id")
        .to_string();
    relay
        .resolve(&json!({
            "requestId": request_id,
            "ok": false,
            "error": "connector unavailable",
        }))
        .expect("resolve failure");
    assert!(matches!(
        worker.join().expect("worker"),
        Err(CoordinatorToolRelayError::Remote { message, .. })
            if message == "connector unavailable"
    ));

    assert!(matches!(
        relay.request("deleteEverything", json!({})),
        Err(CoordinatorToolRelayError::UnsupportedMethod(method))
            if method == "deleteEverything"
    ));
}

#[test]
fn runner_tool_request_timeout_and_shutdown_clear_waiters() {
    let relay = CoordinatorToolRelay::with_timeout(
        GatewayEventHub::default(),
        Duration::from_millis(20),
    );
    assert!(matches!(
        relay.request(ROUTED_TOOL_LIST_METHOD, json!({})),
        Err(CoordinatorToolRelayError::Timeout(_))
    ));
    assert_eq!(relay.pending_count(), 0);

    let events = GatewayEventHub::default();
    let event_rx = events.subscribe();
    let relay = Arc::new(CoordinatorToolRelay::with_timeout(
        events,
        Duration::from_secs(1),
    ));
    let worker = {
        let relay = Arc::clone(&relay);
        thread::spawn(move || relay.request(ROUTED_TOOL_LIST_METHOD, json!({})))
    };
    event_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pending request");
    relay.cancel_all("host shutdown");
    assert!(matches!(
        worker.join().expect("worker"),
        Err(CoordinatorToolRelayError::Closed(reason)) if reason == "host shutdown"
    ));
    assert_eq!(relay.pending_count(), 0);
}
