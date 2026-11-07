use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use prost::Message;
use serde_json::json;

use mahayana_host_runtime::cursor_backend::clear_sand_privacy_mode_cache_for_testing;
use mahayana_host_runtime::extensions::auto_review::sand_backend_smart_mode_classifier_exec::{
    DASHBOARD_CLASSIFY_SAND_AUTO_REVIEW_PATH,
    SandBackendSmartModeClassifierExecutor,
    SandBackendSmartModeClassifierOptions,
};
use mahayana_host_runtime::runner::sand_auto_review_classifier_run::{
    AutoReviewClassifierError, AutoReviewClassifierRequest,
    SandAutoReviewClassifierExecutor, SmartModeClassifierDecision,
    SmartModeClassifierResult,
};

const PRIVACY_PATH: &str = "/aiserver.v1.DashboardService/GetUserPrivacyMode";

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
    let (mut stream, _) = listener.accept().expect("accept backend request");
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

#[derive(Clone, PartialEq, Message)]
struct RequestProbe {
    #[prost(message, optional, tag = "1")]
    args: Option<ArgsProbe>,
    #[prost(uint32, optional, tag = "2")]
    attempt_index: Option<u32>,
    #[prost(string, optional, tag = "3")]
    mode: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ArgsProbe {
    #[prost(string, tag = "1")]
    tool_call_id: String,
    #[prost(string, optional, tag = "2")]
    parent_conversation_id: Option<String>,
    #[prost(message, optional, tag = "3")]
    target: Option<TargetProbe>,
    #[prost(message, repeated, tag = "4")]
    conversation_context: Vec<ConversationProbe>,
}

#[derive(Clone, PartialEq, Message)]
struct TargetProbe {
    #[prost(string, tag = "1")]
    action: String,
    #[prost(bytes, optional, tag = "2")]
    arguments: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
struct ConversationProbe {
    #[prost(string, tag = "1")]
    role: String,
    #[prost(string, tag = "2")]
    content: String,
}

#[derive(Clone, PartialEq, Message)]
struct ResponseWire {
    #[prost(message, optional, tag = "1")]
    result: Option<ResultWire>,
}

#[derive(Clone, PartialEq, Message)]
struct ResultWire {
    #[prost(oneof = "result_wire::Result", tags = "1, 2")]
    result: Option<result_wire::Result>,
}

mod result_wire {
    use super::{ErrorWire, SuccessWire};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(SuccessWire),
        #[prost(message, tag = "2")]
        Error(ErrorWire),
    }
}

#[derive(Clone, PartialEq, Message)]
struct SuccessWire {
    #[prost(int32, tag = "1")]
    decision: i32,
    #[prost(string, optional, tag = "2")]
    block_reason: Option<String>,
    #[prost(string, optional, tag = "3")]
    proposed_allow_rule: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct ErrorWire {
    #[prost(string, tag = "1")]
    error: String,
}

#[test]
fn backend_classifier_projects_frozen_dashboard_contract_without_tool_call_id() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();

    let response = ResponseWire {
        result: Some(ResultWire {
            result: Some(result_wire::Result::Success(SuccessWire {
                decision: 2,
                block_reason: Some("sensitive boundary".into()),
                proposed_allow_rule: Some("allow approved host".into()),
            })),
        }),
    }
    .encode_to_vec();

    let server = thread::spawn(move || {
        let privacy = serve(&listener, &[0x08, 0x03]);
        let classify = serve(&listener, &response);
        (privacy, classify)
    });

    let backend_url = format!("http://127.0.0.1:{port}");
    let mut executor = SandBackendSmartModeClassifierExecutor::new(
        SandBackendSmartModeClassifierOptions {
            backend_url,
            get_access_token: Arc::new(|| Ok("access-token".into())),
            get_machine_id: Arc::new(|| Ok("machine-42".into())),
        },
    );

    let workspace_paths = vec!["/workspace".to_string()];
    let result = executor
        .execute(AutoReviewClassifierRequest {
            tool_call_id: "host-local-secret-tool-call-id",
            parent_conversation_id: "conversation-1",
            mode: "shadow",
            target: json!({
                "action": "shell",
                "arguments": {
                    "command": "echo ok",
                    "execution_surface": "host_machine"
                }
            }),
            conversation_context: vec![
                json!({"role": "user", "content": "run it"}),
                json!({"role": "assistant", "content": "checking"}),
            ],
            workspace_paths: &workspace_paths,
            suppress_tool_call_id_logging: true,
            max_attempts: 1,
        })
        .expect("classifier response");

    assert_eq!(
        result,
        SmartModeClassifierResult::Success(
            mahayana_host_runtime::runner::sand_auto_review_classifier_run::SmartModeClassifierSuccess {
                decision: SmartModeClassifierDecision::Block,
                block_reason: Some("sensitive boundary".into()),
                proposed_allow_rule: Some("allow approved host".into()),
            }
        )
    );

    let (privacy, classify) = server.join().expect("server");
    let (privacy_headers, _) = split_request(&privacy);
    assert!(privacy_headers.starts_with(&format!("POST {PRIVACY_PATH} HTTP/1.1\r\n")));

    let (headers, body) = split_request(&classify);
    assert!(headers.starts_with(&format!(
        "POST {DASHBOARD_CLASSIFY_SAND_AUTO_REVIEW_PATH} HTTP/1.1\r\n"
    )));
    let lower_headers = headers.to_ascii_lowercase();
    assert!(lower_headers.contains("authorization: bearer access-token\r\n"));
    assert!(lower_headers.contains("content-type: application/proto\r\n"));
    assert!(lower_headers.contains("connect-protocol-version: 1\r\n"));
    assert!(lower_headers.contains("x-cursor-client-type: sand\r\n"));
    assert!(lower_headers.contains("x-ghost-mode: false\r\n"));

    let request = RequestProbe::decode(body).expect("classifier request protobuf");
    assert_eq!(request.attempt_index, Some(0));
    assert_eq!(request.mode.as_deref(), Some("shadow"));
    let args = request.args.expect("args");
    assert_eq!(args.tool_call_id, "");
    assert_eq!(
        args.parent_conversation_id.as_deref(),
        Some("conversation-1")
    );
    let target = args.target.expect("target");
    assert_eq!(target.action, "shell");
    assert!(target.arguments.is_some());
    assert_eq!(args.conversation_context.len(), 2);
    assert_eq!(args.conversation_context[0].role, "user");
    assert_eq!(args.conversation_context[0].content, "run it");
    assert!(!body
        .windows("host-local-secret-tool-call-id".len())
        .any(|window| window == b"host-local-secret-tool-call-id"));
}

#[test]
fn backend_classifier_defaults_mode_and_preserves_missing_result() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let server = thread::spawn(move || {
        let _privacy = serve(&listener, &[0x08, 0x01]);
        serve(&listener, &[])
    });

    let backend_url = format!("http://127.0.0.1:{port}");
    let mut executor = SandBackendSmartModeClassifierExecutor::new(
        SandBackendSmartModeClassifierOptions {
            backend_url,
            get_access_token: Arc::new(|| Ok("access-token-2".into())),
            get_machine_id: Arc::new(|| Ok("machine-99".into())),
        },
    );
    let result = executor
        .execute(AutoReviewClassifierRequest {
            tool_call_id: "local-only",
            parent_conversation_id: "conversation-2",
            mode: "",
            target: json!({"action": "mcp", "arguments": {}}),
            conversation_context: Vec::<serde_json::Value>::new(),
            workspace_paths: &[],
            suppress_tool_call_id_logging: true,
            max_attempts: 1,
        })
        .expect("missing backend result remains a classifier result");
    assert_eq!(result, SmartModeClassifierResult::Missing);

    let classify = server.join().expect("server");
    let (_, body) = split_request(&classify);
    let request = RequestProbe::decode(body).expect("classifier request protobuf");
    assert_eq!(request.mode.as_deref(), Some("enforce"));
}

#[test]
fn backend_classifier_aborts_in_flight_request_when_runner_is_cancelled() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let cancelled = Arc::new(AtomicBool::new(false));
    let server_cancelled = Arc::clone(&cancelled);
    let (started_tx, started_rx) = mpsc::channel();

    let server = thread::spawn(move || {
        let _privacy = serve(&listener, &[0x08, 0x03]);
        let (mut stream, _) = listener.accept().expect("accept classifier request");
        let request = read_request(&mut stream);
        started_tx.send(()).expect("signal request started");
        while !server_cancelled.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        thread::sleep(Duration::from_millis(150));
        request
    });

    let cancellation_for_executor = Arc::clone(&cancelled);
    let backend_url = format!("http://127.0.0.1:{port}");
    let mut executor = SandBackendSmartModeClassifierExecutor::with_cancellation(
        SandBackendSmartModeClassifierOptions {
            backend_url,
            get_access_token: Arc::new(|| Ok("access-token-cancel".into())),
            get_machine_id: Arc::new(|| Ok("machine-cancel".into())),
        },
        Arc::new(move || cancellation_for_executor.load(Ordering::Acquire)),
    );

    let cancellation_for_thread = Arc::clone(&cancelled);
    let canceller = thread::spawn(move || {
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("classifier request should start");
        cancellation_for_thread.store(true, Ordering::Release);
    });

    let started = Instant::now();
    let error = executor
        .execute(AutoReviewClassifierRequest {
            tool_call_id: "cancelled-local-only",
            parent_conversation_id: "conversation-cancel",
            mode: "enforce",
            target: json!({"action": "cloud_agent", "arguments": {}}),
            conversation_context: Vec::<serde_json::Value>::new(),
            workspace_paths: &[],
            suppress_tool_call_id_logging: true,
            max_attempts: 1,
        })
        .expect_err("in-flight classifier must abort");

    assert!(
        matches!(error, AutoReviewClassifierError::Aborted(_)),
        "unexpected classifier cancellation result: {error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "classifier cancellation waited for the 10s transport timeout"
    );

    canceller.join().expect("canceller");
    let classify = server.join().expect("server");
    let (headers, _) = split_request(&classify);
    assert!(headers.starts_with(&format!(
        "POST {DASHBOARD_CLASSIFY_SAND_AUTO_REVIEW_PATH} HTTP/1.1\r\n"
    )));
}

