use mahayana_host_runtime::extensions::cross_user_sharing::xuser_relay::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
struct RecordedRequest {
    method: String,
    url: String,
    body: Option<Value>,
}

struct FakeTransport {
    responses: Mutex<VecDeque<Result<Value, SandXuserRelayHttpError>>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl FakeTransport {
    fn new(responses: Vec<Result<Value, SandXuserRelayHttpError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl XuserRelayTransport for FakeTransport {
    fn request(
        &self,
        method: &str,
        url: &str,
        _token: &str,
        body: Option<&Value>,
    ) -> Result<Value, SandXuserRelayHttpError> {
        self.requests.lock().unwrap().push(RecordedRequest {
            method: method.into(),
            url: url.into(),
            body: body.cloned(),
        });
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(json!({ "events": [] })))
    }
}

fn runtime(
    transport: Arc<FakeTransport>,
    now: Arc<AtomicU64>,
    connected: Arc<AtomicBool>,
    safety_poll: Arc<AtomicBool>,
    handled: Arc<Mutex<Vec<String>>>,
) -> SandXuserRelayRuntime {
    let client = Arc::new(SandXuserRelayClient::new(
        "https://example.invalid".into(),
        Arc::new(|| Ok("token".into())),
        transport,
    ));
    SandXuserRelayRuntime::new(
        client,
        Arc::new(move |event| {
            let id = event
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            handled.lock().unwrap().push(id.clone());
            Ok(id == "a")
        }),
        Arc::new(move || now.load(Ordering::SeqCst)),
        Arc::new(move || connected.load(Ordering::SeqCst)),
        Arc::new(move || safety_poll.load(Ordering::SeqCst)),
    )
}

#[test]
fn relay_ack_lifecycle_and_notify_floor_match_frozen_contract() {
    let transport = Arc::new(FakeTransport::new(vec![
        Ok(json!({
            "events": [
                { "id": "a", "kind": "room-post" },
                { "id": "b", "kind": "room-post" }
            ]
        })),
        Ok(json!({
            "events": [
                { "id": "a", "kind": "room-post" },
                { "id": "b", "kind": "room-post" }
            ]
        })),
        Ok(json!({ "events": [] })),
        Ok(json!({ "events": [] })),
    ]));
    let now = Arc::new(AtomicU64::new(1_000));
    let connected = Arc::new(AtomicBool::new(true));
    let safety_poll = Arc::new(AtomicBool::new(true));
    let handled = Arc::new(Mutex::new(Vec::new()));
    let relay = runtime(
        Arc::clone(&transport),
        Arc::clone(&now),
        connected,
        safety_poll,
        Arc::clone(&handled),
    );

    relay.start();
    assert_eq!(relay.tick().unwrap(), true);
    assert_eq!(relay.pending_ack_ids(), vec!["a"]);

    // Owed acks force another drain. The already-acked event is not executed
    // again while an unhandled event remains eligible for redelivery.
    assert_eq!(relay.tick().unwrap(), true);
    assert_eq!(relay.pending_ack_ids(), vec!["a"]);
    assert_eq!(
        handled.lock().unwrap().as_slice(),
        &["a".to_string(), "b".to_string(), "b".to_string()]
    );

    // The backend retires an ack once it stops replaying the event.
    assert_eq!(relay.tick().unwrap(), true);
    assert!(relay.pending_ack_ids().is_empty());

    // Connected notify mode does not poll before the frozen 4 second floor.
    now.store(4_999, Ordering::SeqCst);
    assert_eq!(relay.request_drain().unwrap(), false);
    now.store(5_000, Ordering::SeqCst);
    assert_eq!(relay.tick().unwrap(), true);

    let requests = transport.requests.lock().unwrap();
    assert!(requests.iter().all(|request| request.method == "POST"));
    assert!(requests
        .iter()
        .all(|request| request.url.ends_with("/sand/xuser/poll")));
    assert_eq!(requests[0].body.as_ref().unwrap()["ackIds"], json!([]));
    assert_eq!(requests[1].body.as_ref().unwrap()["ackIds"], json!(["a"]));
}

#[test]
fn relay_error_backoff_matches_frozen_thirty_second_window() {
    let transport = Arc::new(FakeTransport::new(vec![
        Err(SandXuserRelayHttpError {
            status: 503,
            path: "/sand/xuser/poll".into(),
        }),
        Ok(json!({ "events": [] })),
    ]));
    let now = Arc::new(AtomicU64::new(1_000));
    let relay = runtime(
        transport,
        Arc::clone(&now),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
        Arc::new(Mutex::new(Vec::new())),
    );

    relay.start();
    assert!(relay.tick().is_err());
    assert_eq!(relay.backoff_until_ms(), 31_000);

    now.store(30_999, Ordering::SeqCst);
    assert_eq!(relay.tick().unwrap(), false);
    now.store(31_000, Ordering::SeqCst);
    assert_eq!(relay.tick().unwrap(), true);
}

#[test]
fn relay_uses_frozen_grok_endpoint_family() {
    let transport = Arc::new(FakeTransport::new(vec![
        Ok(json!({ "rooms": [] })),
        Ok(json!({ "timestampMs": 42 })),
        Ok(json!({
            "shareUrl": "https://share.example/room-a",
            "expiresAtMs": 12345,
            "room": { "roomId": "room-a" }
        })),
        Ok(json!({ "status": "pending", "roomName": "Shared" })),
    ]));
    let client = SandXuserRelayClient::new(
        "https://example.invalid".into(),
        Arc::new(|| Ok("token".into())),
        Arc::clone(&transport) as Arc<dyn XuserRelayTransport>,
    );

    client.fetch_share_state().unwrap();
    let sent = client.send(&json!({ "kind": "entry" })).unwrap();
    assert_eq!(sent["timestampMs"], 42.0);

    let created = client
        .create_room_from_agent(&json!({ "agentId": "agent-a" }))
        .unwrap();
    assert_eq!(created["status"], "ok");
    assert_eq!(created["roomId"], "room-a");
    assert_eq!(created["shareUrl"], "https://share.example/room-a");

    let joined = client.join_room("invite").unwrap();
    assert_eq!(joined["status"], "pending");
    assert_eq!(joined["roomName"], "Shared");

    let requests = transport.requests.lock().unwrap();
    assert!(requests[0].url.ends_with("/sand/share-state"));
    assert!(requests[1].url.ends_with("/sand/xuser/send"));
    assert!(requests[2].url.ends_with("/sand/share-rooms/from-agent"));
    assert!(requests[3].url.ends_with("/sand/share-rooms/join"));
}

#[test]
fn relay_driver_owns_polling_wakeup_and_clean_stop() {
    let transport = Arc::new(FakeTransport::new(vec![
        Ok(json!({ "events": [] })),
        Ok(json!({ "events": [] })),
    ]));
    let now = Arc::new(AtomicU64::new(1_000));
    let runtime = Arc::new(runtime(
        Arc::clone(&transport),
        now,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(true)),
        Arc::new(Mutex::new(Vec::new())),
    ));
    let driver = SandXuserRelayDriver::new(runtime);

    driver.start();
    for _ in 0..100 {
        if !transport.requests.lock().unwrap().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(driver.is_started());
    assert!(!transport.requests.lock().unwrap().is_empty());

    let before = transport.requests.lock().unwrap().len();
    driver.request_drain();
    for _ in 0..100 {
        if transport.requests.lock().unwrap().len() > before {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(transport.requests.lock().unwrap().len() > before);

    driver.stop();
    assert!(!driver.is_started());
}

#[test]
fn relay_projects_frozen_join_and_error_responses() {
    let transport = Arc::new(FakeTransport::new(vec![
        Ok(json!({
            "status": "already-member",
            "room": { "roomId": "room-a", "name": "Room A" }
        })),
        Ok(json!({ "status": "denied" })),
        Err(SandXuserRelayHttpError {
            status: 403,
            path: "/sand/share-rooms/from-agent".into(),
        }),
    ]));
    let client = SandXuserRelayClient::new(
        "https://example.invalid".into(),
        Arc::new(|| Ok("token".into())),
        Arc::clone(&transport) as Arc<dyn XuserRelayTransport>,
    );

    let member = client.join_room("invite-a").unwrap();
    assert_eq!(member["status"], "already-member");
    assert_eq!(member["roomName"], "Room A");
    assert_eq!(member["roomId"], "room-a");

    let denied = client.join_room("invite-b").unwrap();
    assert_eq!(denied, json!({ "status": "denied" }));

    let blocked = client
        .create_room_from_agent(&json!({ "agentId": "agent-a" }))
        .unwrap();
    assert_eq!(blocked["status"], "error");
    assert_eq!(
        blocked["message"],
        "Sharing isn't enabled for your account."
    );
}
