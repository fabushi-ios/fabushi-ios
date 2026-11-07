use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
use mahayana_node_agent_coordinator::webauthn::ApprovedWebAuthnConsent;
use mahayana_node_agent_coordinator::webauthn::provider::{
    ProductionWebAuthnRuntime, ProductionWebAuthnRuntimeOptions,
};
use mahayana_node_agent_coordinator::webauthn::signer::SpawnedWebAuthnSigner;
use serde_json::Value;

fn handle_connection(
    mut stream: std::net::TcpStream,
    get_count: Arc<AtomicUsize>,
    post_count: Arc<AtomicUsize>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone gateway stream"));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("request header");
        if line.is_empty() || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().expect("content length");
            }
        }
    }
    if request_line.starts_with("GET /webauthn/requests ") {
        let sequence = get_count.fetch_add(1, Ordering::AcqRel) + 1;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {{\"kind\":\"welcome\",\"providerId\":\"provider-{sequence}\"}}\n\n"
        )
        .expect("write SSE response");
        stream.flush().expect("flush SSE response");
        return;
    }
    if request_line.starts_with("POST /webauthn/responses ") {
        let mut body = vec![0_u8; content_length];
        reader.read_exact(&mut body).expect("response body");
        let _: Value = serde_json::from_slice(&body).expect("response JSON");
        post_count.fetch_add(1, Ordering::AcqRel);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write response acknowledgement");
        stream.flush().expect("flush response acknowledgement");
        return;
    }
    write!(
        stream,
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("write not found");
}

#[test]
fn production_webauthn_runtime_reconnects_and_disposes_without_waiting_for_sse_stall() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind runtime gateway");
    listener.set_nonblocking(true).expect("set nonblocking accept");
    let port = listener.local_addr().expect("gateway address").port();
    let stop_server = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop_server);
    let get_count = Arc::new(AtomicUsize::new(0));
    let post_count = Arc::new(AtomicUsize::new(0));
    let server_get_count = Arc::clone(&get_count);
    let server_post_count = Arc::clone(&post_count);
    let server = thread::spawn(move || {
        let mut handlers = Vec::new();
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let gets = Arc::clone(&server_get_count);
                    let posts = Arc::clone(&server_post_count);
                    handlers.push(thread::spawn(move || handle_connection(stream, gets, posts)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("gateway accept failed: {error}"),
            }
        }
        for handler in handlers {
            handler.join().expect("gateway handler");
        }
    });

    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers: BTreeMap::new(),
    };
    let mut runtime = ProductionWebAuthnRuntime::new(ProductionWebAuthnRuntimeOptions {
        signer: SpawnedWebAuthnSigner {
            binary_path: PathBuf::from("unused-webauthn-signer"),
        },
        resolve_connection: Arc::new(move || Ok(connection.clone())),
        request_consent: Arc::new(|_| Ok(ApprovedWebAuthnConsent::approved())),
        update_status: Arc::new(|_| {}),
        request_pin: Arc::new(|_, _| None),
        finish_consent: Arc::new(|| {}),
        computer_id: Some("computer-runtime".into()),
        label: Some("Fabushi Desktop".into()),
    });
    runtime.start();

    let deadline = Instant::now() + Duration::from_secs(5);
    while get_count.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        get_count.load(Ordering::Acquire) >= 2,
        "production WebAuthn runtime did not reconnect after the stream closed"
    );
    assert!(
        post_count.load(Ordering::Acquire) >= 1,
        "production WebAuthn runtime did not publish its hello frame"
    );

    let dispose_started = Instant::now();
    runtime.dispose();
    assert!(
        dispose_started.elapsed() < Duration::from_secs(2),
        "production WebAuthn runtime disposal waited for the SSE stall timeout"
    );

    stop_server.store(true, Ordering::Release);
    server.join().expect("gateway server");
}
