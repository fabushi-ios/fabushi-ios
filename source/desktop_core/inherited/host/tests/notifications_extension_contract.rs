use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::Duration;

use prost::Message;
use serde_json::json;

use mahayana_host_runtime::cursor_backend::clear_sand_privacy_mode_cache_for_testing;
use mahayana_host_runtime::extensions::notifications::extension::{
    NOTIFICATIONS_DEPENDENCIES, NOTIFICATIONS_EXTENSION_ID,
    start_notifications_extension_with_sender,
};
use mahayana_host_runtime::extensions::notifications::mobile_push_notifier::{
    CursorMobilePushOptions, MobilePushAuth, MobilePushInput, NotificationAgent,
    NOTIFY_SAND_AGENT_TURN_FINISHED_PATH, create_cursor_mobile_push_sender,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::host_event_bus::SandHostEventBus;

#[derive(Clone)]
struct TestAuth;

impl MobilePushAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("mobile-push-token".into())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        Ok("mobile-push-machine".into())
    }
}

#[derive(Clone, PartialEq, Message)]
struct NotifyRequest {
    #[prost(string, tag = "1")]
    agent_id: String,
    #[prost(string, tag = "2")]
    agent_name: String,
    #[prost(string, tag = "3")]
    message_preview: String,
    #[prost(string, tag = "4")]
    last_message_id: String,
    #[prost(bool, tag = "5")]
    awaiting_user_response: bool,
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    loop {
        let count = stream.read(&mut buffer).expect("read request");
        assert!(count > 0, "client closed before request completed");
        request.extend_from_slice(&buffer[..count]);
        if expected.is_none() {
            if let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..header_end]).expect("headers");
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("content length"))
                    })
                    .unwrap_or(0);
                expected = Some(header_end + 4 + length);
            }
        }
        if expected.is_some_and(|value| request.len() >= value) {
            return request;
        }
    }
}

fn serve(listener: &TcpListener, body: &[u8]) -> Vec<u8> {
    let (mut stream, _) = listener.accept().expect("accept request");
    let request = read_request(&mut stream);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write response");
    stream.write_all(body).expect("response body");
    stream.flush().expect("flush response");
    request
}

fn split_request(request: &[u8]) -> (&str, &[u8]) {
    let header_end = request
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header terminator");
    (
        std::str::from_utf8(&request[..header_end]).expect("headers"),
        &request[header_end + 4..],
    )
}

fn notification_agent(
    running: bool,
    awaiting_reason: Option<&str>,
    message_id: &str,
) -> NotificationAgent {
    NotificationAgent {
        id: "agent-a".into(),
        name: "Alpha".into(),
        is_running: running,
        awaiting_reason: awaiting_reason.map(str::to_string),
        notify_enabled: true,
        is_hidden_from_sidebar: false,
        last_message_id: Some(message_id.into()),
        last_message_preview: Some(format!("preview-{message_id}")),
    }
}

fn agent_json(running: bool, awaiting_reason: Option<&str>, message_id: &str) -> serde_json::Value {
    json!({
        "id": "agent-a",
        "name": "Alpha",
        "isRunning": running,
        "awaitingUserResponse": awaiting_reason.map(|reason| json!({ "reason": reason })),
        "notifyOnUpdatesEnabled": true,
        "isHiddenFromSidebar": false,
        "lastMessageId": message_id,
        "lastMessagePreview": format!("preview-{message_id}")
    })
}

#[test]
fn notifications_extension_declares_frozen_identity_and_auth_dependency() {
    assert_eq!(NOTIFICATIONS_EXTENSION_ID, HostExtensionId::Notifications);
    assert_eq!(NOTIFICATIONS_DEPENDENCIES, &[HostExtensionId::Auth]);
}

#[test]
fn cursor_mobile_push_sender_uses_grokbot_proto_and_authenticated_cursor_headers() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("address").port();
    let server = thread::spawn(move || {
        let privacy = serve(&listener, &[0x08, 0x03]);
        let notify = serve(&listener, &[]);
        (privacy, notify)
    });

    let sender = create_cursor_mobile_push_sender(CursorMobilePushOptions {
        auth: Arc::new(TestAuth),
        backend_url: format!("http://127.0.0.1:{port}"),
    });
    sender(MobilePushInput {
        agent_id: "agent-a".into(),
        agent_name: "Alpha".into(),
        message_preview: "finished".into(),
        last_message_id: "message-2".into(),
        awaiting_user_response: true,
    })
    .expect("mobile push");

    let (privacy, notify) = server.join().expect("server");
    let (privacy_headers, _) = split_request(&privacy);
    assert!(
        privacy_headers
            .to_ascii_lowercase()
            .contains("x-ghost-mode: true\r\n")
    );

    let (headers, body) = split_request(&notify);
    assert!(headers.starts_with(&format!(
        "POST {NOTIFY_SAND_AGENT_TURN_FINISHED_PATH} HTTP/1.1\r\n"
    )));
    let lower = headers.to_ascii_lowercase();
    assert!(lower.contains("authorization: bearer mobile-push-token\r\n"));
    assert!(lower.contains("connect-protocol-version: 1\r\n"));
    assert!(lower.contains("x-ghost-mode: false\r\n"));
    assert!(lower.contains("x-cursor-client-type:"));
    assert!(lower.contains("x-cursor-checksum:"));
    assert!(lower.contains("x-request-id:"));

    let decoded = NotifyRequest::decode(body).expect("notify protobuf");
    assert_eq!(decoded.agent_id, "agent-a");
    assert_eq!(decoded.agent_name, "Alpha");
    assert_eq!(decoded.message_preview, "finished");
    assert_eq!(decoded.last_message_id, "message-2");
    assert!(decoded.awaiting_user_response);
}

#[test]
fn extension_uses_authoritative_roster_events_focus_and_forget_lifecycle() {
    let bus = SandHostEventBus::default();
    let sent = Arc::new(Mutex::new(Vec::<MobilePushInput>::new()));
    let capture = Arc::clone(&sent);
    let notify = Arc::new(move |input: MobilePushInput| {
        capture.lock().expect("sent").push(input);
        Ok(())
    });

    let now = Arc::new(AtomicU64::new(10_000));
    let now_source = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let focused_at = Arc::new(Mutex::new(None::<u64>));
    let focus_source = {
        let focused_at = Arc::clone(&focused_at);
        Arc::new(move || *focused_at.lock().expect("focus"))
    };

    let extension = start_notifications_extension_with_sender(
        bus.clone(),
        Some(vec![notification_agent(true, None, "m1")]),
        focus_source,
        now_source,
        notify,
    );

    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(false, None, "m2") }
    }));
    {
        let sent = sent.lock().expect("sent");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].last_message_id, "m2");
        assert!(!sent[0].awaiting_user_response);
    }

    now.store(20_001, Ordering::SeqCst);
    *focused_at.lock().expect("focus") = Some(20_000);
    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(true, None, "m2") }
    }));
    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(false, None, "m3") }
    }));
    assert_eq!(sent.lock().expect("sent").len(), 1);

    *focused_at.lock().expect("focus") = None;
    now.store(30_000, Ordering::SeqCst);
    bus.publish(json!({
        "channel": "agents",
        "payload": { "agents": [] }
    }));
    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(false, None, "m4") }
    }));
    assert_eq!(
        sent.lock().expect("sent").len(),
        1,
        "forgotten agent must be re-observed before another transition can notify"
    );

    drop(extension);
    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(true, Some("approve"), "m5") }
    }));
    assert_eq!(sent.lock().expect("sent").len(), 1);
}

#[test]
fn extension_buffers_upsert_until_first_full_roster_when_startup_baseline_is_unavailable() {
    let bus = SandHostEventBus::default();
    let sent = Arc::new(Mutex::new(Vec::<MobilePushInput>::new()));
    let capture = Arc::clone(&sent);
    let extension = start_notifications_extension_with_sender(
        bus.clone(),
        None,
        Arc::new(|| None),
        Arc::new(|| 10_000),
        Arc::new(move |input| {
            capture.lock().expect("sent").push(input);
            Ok(())
        }),
    );

    bus.publish(json!({
        "channel": "agent-upserted",
        "payload": { "agent": agent_json(true, Some("approve"), "m2") }
    }));
    assert!(sent.lock().expect("sent").is_empty());

    bus.publish(json!({
        "channel": "agents",
        "payload": { "agents": [agent_json(true, None, "m1")] }
    }));
    let sent = sent.lock().expect("sent");
    assert_eq!(sent.len(), 1);
    assert!(sent[0].awaiting_user_response);
    assert_eq!(sent[0].last_message_id, "m2");
    drop(sent);
    drop(extension);
}
