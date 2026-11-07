use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use mahayana_node_agent_coordinator::gateway::gateway_client::{
    dispatch_gateway_command, GatewayClientTiming, GatewayCommandPolicy,
};
use mahayana_node_agent_coordinator::gateway::gateway_request_dispatcher::GatewayDispatchError;
use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;
use serde_json::{Value, json};

fn read_request(stream: &mut std::net::TcpStream) -> (String, BTreeMap<String, String>, Value) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone scripted gateway stream"));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("request line");
    let mut headers = BTreeMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("request header");
        if line.is_empty() || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.trim_end().split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().expect("content length");
            }
            headers.insert(name.to_ascii_lowercase(), value);
        }
    }
    let mut body = vec![0_u8; content_length];
    reader.read_exact(&mut body).expect("request body");
    (
        request_line.trim_end().to_string(),
        headers,
        serde_json::from_slice(&body).expect("request JSON"),
    )
}

fn write_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    headers: &[(&str, &str)],
    body: Value,
) {
    let body = serde_json::to_vec(&body).expect("response JSON");
    let reason = if status == 200 { "OK" } else { "Service Unavailable" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    )
    .expect("response head");
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").expect("response header");
    }
    write!(stream, "Connection: close\r\n\r\n").expect("response terminator");
    stream.write_all(&body).expect("response body");
    stream.flush().expect("response flush");
}

fn test_timing() -> GatewayClientTiming {
    GatewayClientTiming {
        create_agent_retry_initial_ms: 1,
        create_agent_retry_max_ms: 1,
        ..GatewayClientTiming::default()
    }
}

#[test]
fn send_prompt_accept_return_proves_dedupe_and_retries_on_the_pinned_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&requests);
    let server = thread::spawn(move || {
        for index in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept gateway request");
            let request = read_request(&mut stream);
            observed.lock().expect("observed requests").push(request);
            match index {
                0 => write_response(&mut stream, 200, &[], json!({"accepted":true})),
                1 => write_response(&mut stream, 503, &[], json!({"error":"transient"})),
                _ => write_response(&mut stream, 200, &[], json!({"accepted":true,"retry":true})),
            }
        }
    });

    let policy = GatewayCommandPolicy::with_timing(test_timing());
    let connection = GatewayConnection { base_url: base_url.clone(), headers: BTreeMap::new() };
    let resolve = |required: Option<&str>| -> Result<GatewayConnection, GatewayDispatchError> {
        if let Some(required) = required {
            assert_eq!(required, base_url);
        }
        Ok(connection.clone())
    };
    let first = dispatch_gateway_command(
        &policy,
        "sendPrompt",
        json!({"clientNonce":"nonce-1","message":"first"}),
        1,
        resolve,
    )
    .expect("first send");
    assert_eq!(first.value["accepted"], true);
    assert!(policy.send_dedupe_proven(&base_url));

    let connection = GatewayConnection { base_url: base_url.clone(), headers: BTreeMap::new() };
    let second = dispatch_gateway_command(
        &policy,
        "sendPrompt",
        json!({"clientNonce":"nonce-2","message":"second"}),
        2,
        |required| {
            if let Some(required) = required {
                assert_eq!(required, base_url);
            }
            Ok(connection.clone())
        },
    )
    .expect("dedupe-safe retry");
    assert_eq!(second.value["retry"], true);
    assert_eq!(
        second
            .transport_stages
            .iter()
            .filter(|stage| stage.stage == "gateway-post")
            .count(),
        2
    );
    server.join().expect("gateway server");
    assert_eq!(requests.lock().expect("requests").len(), 3);
}

#[test]
fn roster_read_is_bounded_to_one_retry_and_create_agent_pins_after_mint_dedupe() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = Arc::clone(&bodies);
    let server = thread::spawn(move || {
        for index in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept gateway request");
            let (request_line, _headers, body) = read_request(&mut stream);
            observed.lock().expect("bodies").push(body);
            match index {
                0 => {
                    assert!(request_line.starts_with("POST /api/listAgents "));
                    write_response(&mut stream, 503, &[], json!({"error":"retry"}));
                }
                1 => write_response(&mut stream, 200, &[], json!([{"id":"a1"}])),
                2 => write_response(
                    &mut stream,
                    200,
                    &[("x-sand-mint-dedupe", "1")],
                    json!({"ok":true}),
                ),
                3 => {
                    assert!(request_line.starts_with("POST /api/createAgent "));
                    write_response(&mut stream, 503, &[], json!({"error":"retry"}));
                }
                _ => write_response(&mut stream, 200, &[], json!({"id":"agent-2"})),
            }
        }
    });

    let policy = GatewayCommandPolicy::with_timing(test_timing());
    let connection = GatewayConnection { base_url: base_url.clone(), headers: BTreeMap::new() };
    let roster = dispatch_gateway_command(
        &policy,
        "listAgents",
        json!({"ignored":true}),
        1,
        |_| Ok(connection.clone()),
    )
    .expect("bounded roster retry");
    assert_eq!(roster.value[0]["id"], "a1");

    dispatch_gateway_command(
        &policy,
        "probeMintDedupe",
        json!({}),
        2,
        |_| Ok(connection.clone()),
    )
    .expect("dedupe probe");
    assert!(policy.mint_dedupe_proven(&base_url));

    let agent = dispatch_gateway_command(
        &policy,
        "createAgent",
        json!({"name":"Pinned"}),
        3,
        |required| {
            if let Some(required) = required {
                assert_eq!(required, base_url);
            }
            Ok(connection.clone())
        },
    )
    .expect("create retry");
    assert_eq!(agent.value["id"], "agent-2");

    server.join().expect("gateway server");
    let bodies = bodies.lock().expect("bodies");
    let first_create_nonce = bodies[3]["clientNonce"].as_str().expect("first create nonce");
    let second_create_nonce = bodies[4]["clientNonce"].as_str().expect("second create nonce");
    assert_eq!(first_create_nonce, second_create_nonce);
    assert!(!first_create_nonce.is_empty());
}

#[test]
fn trace_window_root_is_cached_and_produces_child_span_and_header() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind gateway");
    let base_url = format!("http://{}", listener.local_addr().expect("address"));
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept gateway request");
        let (_request_line, headers, _body) = read_request(&mut stream);
        let traceparent = headers.get("traceparent").expect("traceparent header").clone();
        write_response(&mut stream, 200, &[], json!({"traceparent":traceparent}));
    });

    let policy = GatewayCommandPolicy::with_timing(test_timing());
    let root = "00-0123456789abcdef0123456789abcdef-1111111111111111-01";
    policy.cache_trace_window_root(Some(root.into()), 100);
    assert!(!policy.trace_window_root_needs_refresh(101));
    assert!(policy.trace_window_root_needs_refresh(100 + 5_000));

    let connection = GatewayConnection { base_url, headers: BTreeMap::new() };
    let execution = dispatch_gateway_command(
        &policy,
        "getTranscript",
        json!({}),
        101,
        |_| Ok(connection.clone()),
    )
    .expect("traced command");
    let sent = execution.value["traceparent"].as_str().expect("returned traceparent");
    assert!(sent.starts_with("00-0123456789abcdef0123456789abcdef-"));
    assert_ne!(sent, root);
    assert_eq!(execution.command_spans.len(), 1);
    assert_eq!(execution.command_spans[0].root_traceparent, root);
    assert!(!execution.command_spans[0].span_id.is_empty());
    server.join().expect("gateway server");
}
