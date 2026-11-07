use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use mahayana_node_agent_coordinator::gateway::gateway_client::stream_http_events;
use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::dispatch_http_json;
use mahayana_node_agent_coordinator::gateway::host_supervisor::{
    GatewayConnection, fetch_health,
};
use mahayana_node_agent_coordinator::gateway::http_transport::GATEWAY_TLS_CERT_ENV;
use rcgen::generate_simple_self_signed;
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use uuid::Uuid;

static TLS_ENV_LOCK: Mutex<()> = Mutex::new(());

fn read_request(stream: &mut StreamOwned<ServerConnection, TcpStream>) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected_len = None;
    loop {
        let count = stream.read(&mut chunk).expect("read TLS gateway request");
        assert!(count > 0, "TLS request ended before completion");
        bytes.extend_from_slice(&chunk[..count]);
        if expected_len.is_none() {
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = index + 4;
                let headers = String::from_utf8(bytes[..header_end].to_vec())
                    .expect("TLS request headers utf8");
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                expected_len = Some((header_end, content_length));
            }
        }
        if let Some((header_end, content_length)) = expected_len {
            if bytes.len() >= header_end + content_length {
                return String::from_utf8(bytes).expect("TLS request utf8");
            }
        }
    }
}

fn write_json(
    stream: &mut StreamOwned<ServerConnection, TcpStream>,
    value: &Value,
) {
    let body = serde_json::to_vec(value).expect("serialize TLS response");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write TLS response headers");
    stream.write_all(&body).expect("write TLS response body");
    stream.flush().expect("flush TLS response");
}

#[test]
fn command_health_and_sse_share_real_https_transport() {
    let _env_guard = TLS_ENV_LOCK.lock().expect("TLS env lock");

    let certified = generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("generate TLS certificate");
    let cert_path = std::env::temp_dir().join(format!(
        "fabushi-coordinator-gateway-{}.pem",
        Uuid::new_v4()
    ));
    std::fs::write(&cert_path, certified.cert.pem()).expect("write TLS trust certificate");
    unsafe {
        std::env::set_var(GATEWAY_TLS_CERT_ENV, &cert_path);
    }

    let key = PrivatePkcs8KeyDer::from(certified.key_pair.serialize_der());
    let server_config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certified.cert.der().clone()], key.into())
            .expect("build TLS server config"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind TLS gateway");
    let port = listener.local_addr().expect("TLS gateway address").port();

    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            let (socket, _) = listener.accept().expect("accept TLS gateway connection");
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("TLS server read timeout");
            let connection =
                ServerConnection::new(Arc::clone(&server_config)).expect("TLS server connection");
            let mut stream = StreamOwned::new(connection, socket);
            let request = read_request(&mut stream);
            let first_line = request.lines().next().unwrap_or_default();

            if first_line.starts_with("POST /api/getTranscript ") {
                write_json(&mut stream, &json!({"value":7}));
            } else if first_line.starts_with("GET /health ") {
                write_json(&mut stream, &json!({"ok":true,"generation":9}));
            } else if first_line.starts_with("GET /events ") {
                let body =
                    "retry: 1000\n\ndata: {\"channel\":\"runtime\",\"payload\":{\"value\":9}}\n\n";
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n{body}"
                )
                .expect("write TLS SSE response");
                stream.flush().expect("flush TLS SSE response");
            } else {
                panic!("unexpected TLS gateway request: {first_line}");
            }
        }
    });

    let connection = GatewayConnection {
        base_url: format!("https://localhost:{port}"),
        headers: BTreeMap::new(),
    };

    let value = dispatch_http_json(&connection, "getTranscript", json!({"value":7}))
        .expect("HTTPS command dispatch");
    assert_eq!(value, json!({"value":7}));

    let health = fetch_health(&connection, Duration::from_secs(2))
        .expect("HTTPS health request")
        .expect("healthy HTTPS gateway");
    assert_eq!(health["generation"], 9);

    let connected = Arc::new(AtomicBool::new(false));
    let keep_streaming = Arc::new(AtomicBool::new(true));
    let observed = Arc::new(Mutex::new(Vec::<Value>::new()));
    let connected_for_callback = Arc::clone(&connected);
    let continue_for_event = Arc::clone(&keep_streaming);
    let continue_for_loop = Arc::clone(&keep_streaming);
    let observed_for_event = Arc::clone(&observed);

    stream_http_events(
        &connection,
        move || {
            connected_for_callback.store(true, Ordering::Release);
        },
        move |event| {
            observed_for_event.lock().expect("observed events").push(event);
            continue_for_event.store(false, Ordering::Release);
        },
        move || continue_for_loop.load(Ordering::Acquire),
    )
    .expect("HTTPS SSE stream");

    assert!(connected.load(Ordering::Acquire));
    let events = observed.lock().expect("read observed events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["channel"], "runtime");
    assert_eq!(events[0]["payload"]["value"], 9);

    server.join().expect("TLS gateway server joins");
    unsafe {
        std::env::remove_var(GATEWAY_TLS_CERT_ENV);
    }
    let _ = std::fs::remove_file(cert_path);
}
