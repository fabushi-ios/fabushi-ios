use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
use mahayana_node_agent_coordinator::webauthn::provider::{
    WebAuthnRequestFrame, WebAuthnResponseFrame, post_webauthn_frames,
    stream_webauthn_requests,
};
use serde_json::{Value, json};

#[test]
fn webauthn_request_stream_reads_authenticated_sse_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind WebAuthn request stream");
    let port = listener.local_addr().expect("request stream address").port();
    let observed = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept WebAuthn request stream");
        let mut reader = BufReader::new(stream.try_clone().expect("clone request stream"));
        let mut request = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request header");
            if line.is_empty() || line == "\r\n" {
                break;
            }
            request.push_str(&line);
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\ndata: {{\"kind\":\"welcome\",\"providerId\":\"provider-7\"}}\n\ndata: {{\"kind\":\"cancel\",\"requestId\":\"request-9\"}}\n\n"
        )
        .expect("write request stream response");
        stream.flush().expect("flush request stream");
        request
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer webauthn-secret".into());
    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers,
    };
    let connected = Cell::new(false);
    let frame_count = Cell::new(0usize);
    let frames = RefCell::new(Vec::new());
    stream_webauthn_requests(
        &connection,
        || connected.set(true),
        |frame| {
            frames.borrow_mut().push(frame);
            frame_count.set(frame_count.get() + 1);
        },
        || frame_count.get() < 2,
    )
    .expect("read WebAuthn SSE stream");

    assert!(connected.get());
    let frames = frames.into_inner();
    assert_eq!(frames.len(), 2);
    assert!(matches!(
        &frames[0],
        WebAuthnRequestFrame::Welcome { provider_id } if provider_id == "provider-7"
    ));
    assert!(matches!(
        &frames[1],
        WebAuthnRequestFrame::Cancel { request_id } if request_id == "request-9"
    ));

    let request = observed.join().expect("request stream observer");
    assert!(
        request.starts_with("GET /webauthn/requests HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer webauthn-secret\r\n"),
        "{request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("accept: text/event-stream\r\n"),
        "{request}"
    );
}

#[test]
fn webauthn_response_delivery_posts_provider_batch_and_auth() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind WebAuthn response endpoint");
    let port = listener.local_addr().expect("response endpoint address").port();
    let observed = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept WebAuthn response");
        let mut reader = BufReader::new(stream.try_clone().expect("clone response stream"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("read request line");
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read response header");
            if line.is_empty() || line == "\r\n" {
                break;
            }
            if let Some(value) = line
                .to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
            {
                content_length = value.parse().expect("content length");
            }
            headers.push(line);
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).expect("read response body");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write response acknowledgement");
        stream.flush().expect("flush response acknowledgement");
        (request_line, headers.concat(), body)
    });

    let mut headers = BTreeMap::new();
    headers.insert("authorization".into(), "Bearer webauthn-secret".into());
    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers,
    };
    post_webauthn_frames(
        &connection,
        Some("provider-7"),
        &[
            WebAuthnResponseFrame::Hello {
                computer_id: Some("computer-3".into()),
                label: Some("Fabushi Desktop".into()),
            },
            WebAuthnResponseFrame::Ping,
        ],
    )
    .expect("deliver WebAuthn response frames");

    let (request_line, headers, body) = observed.join().expect("response observer");
    assert_eq!(request_line, "POST /webauthn/responses HTTP/1.1\r\n");
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer webauthn-secret\r\n"),
        "{headers}"
    );
    let body: Value = serde_json::from_slice(&body).expect("response batch JSON");
    assert_eq!(body["providerId"], "provider-7");
    assert_eq!(
        body["frames"],
        json!([
            {
                "kind": "hello",
                "computerId": "computer-3",
                "label": "Fabushi Desktop"
            },
            { "kind": "ping" }
        ])
    );
}


#[test]
fn webauthn_request_stream_observes_async_cancellation_without_waiting_for_stall_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind cancellable WebAuthn stream");
    let port = listener.local_addr().expect("stream address").port();
    let observed = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept cancellable WebAuthn stream");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read request header");
            if line.is_empty() || line == "\r\n" {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n"
        )
        .expect("write SSE headers");
        stream.flush().expect("flush SSE headers");
        thread::sleep(Duration::from_secs(2));
    });

    let connection = GatewayConnection {
        base_url: format!("http://127.0.0.1:{port}"),
        headers: BTreeMap::new(),
    };
    let keep_running = Arc::new(AtomicBool::new(true));
    let stop = Arc::clone(&keep_running);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        stop.store(false, Ordering::Release);
    });

    let started = Instant::now();
    stream_webauthn_requests(
        &connection,
        || {},
        |_| {},
        || keep_running.load(Ordering::Acquire),
    )
    .expect("cancellation should close WebAuthn stream cleanly");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stream cancellation waited for the full stall timeout"
    );
    observed.join().expect("stream observer");
}
