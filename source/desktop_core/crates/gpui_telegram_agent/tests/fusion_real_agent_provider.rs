use gpui_telegram_agent::{AcpRunner, AgentTurnInput, AgentTurnOutput};
use gpui_telegram_core::error::TelegramError;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedProvider, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::Duration;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct ScopedTempDir {
    path: PathBuf,
}

impl ScopedTempDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gpui_telegram_agent_{}_{}",
            prefix,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create temp test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ScopedEnvVars {
    previous: Vec<(String, Option<String>)>,
}

impl ScopedEnvVars {
    fn new(vars: &[(&str, Option<&str>)]) -> Self {
        let mut previous = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            let prev = std::env::var(key).ok();
            previous.push(((*key).to_string(), prev));
            // SAFETY: serialized across all tests in this module via `ENV_MUTEX`.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self { previous }
    }
}

impl Drop for ScopedEnvVars {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            // SAFETY: serialized across all tests in this module via `ENV_MUTEX`.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(&key, v),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

fn write_private_fabushi_credential(dir: &Path, issuer: &str, token: &str) -> PathBuf {
    let cred_path = dir.join("fabushi-credential.json");
    let payload = json!({
        "issuer": issuer,
        "accessToken": token,
    });
    std::fs::write(&cred_path, payload.to_string()).expect("write credential file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600))
            .expect("set 0600 permissions on credential file");
    }
    cred_path
}

fn read_http_request_body(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).expect("read request headers");
        if n == 0 {
            return String::new();
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, val) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| val.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let n = stream.read(&mut chunk).expect("read request body");
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8_lossy(&buffer[header_end..header_end.saturating_add(content_length)])
        .into_owned()
}

fn write_sse_response(stream: &mut TcpStream, sse_events: &[Value]) {
    let mut body = String::new();
    for event in sse_events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .expect("write SSE response");
    stream.flush().expect("flush SSE response");
}

struct LookupOrderBridge {
    calls: Mutex<Vec<(String, Value, String)>>,
}

impl LookupOrderBridge {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl RoutedToolBridge for LookupOrderBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "lookup_order".into(),
            provider_identifier: "orders_mcp".into(),
            tool_name: "lookup_order".into(),
            description: Some("Look up order delivery status by ID".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "order_id": { "type": "string" }
                },
                "required": ["order_id"]
            }),
        }])
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        self.calls
            .lock()
            .unwrap()
            .push((tool.name.clone(), args.clone(), tool_call_id.to_string()));
        let order_id = args
            .get("order_id")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        Ok(json!({
            "content": [{
                "type": "text",
                "text": format!("Order {order_id} status: out_for_delivery")
            }],
            "isError": false
        }))
    }
}

#[tokio::test]
async fn real_routed_provider_executes_tool_streams_deltas_and_persists_checkpoint() {
    let temp_dir = ScopedTempDir::new("positive_routed_provider");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback Responses server");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://{addr}");
    let endpoint = format!("{issuer}/responses");
    let cred_path = write_private_fabushi_credential(temp_dir.path(), &issuer, "token-positive-1");

    let server_handle = std::thread::spawn(move || {
        // Step 1: Receive initial prompt + tool definitions, emit a function_call for `lookup_order`.
        let (mut stream1, _) = listener.accept().expect("accept step 1 request");
        let req1_body = read_http_request_body(&mut stream1);
        let req1_json: Value = serde_json::from_str(&req1_body).expect("parse step 1 JSON");
        assert!(
            req1_body.contains("Where is order ORD-42?"),
            "step 1 request must include user prompt"
        );
        assert!(
            req1_json
                .get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| tools
                    .iter()
                    .any(|t| t.get("name").and_then(Value::as_str) == Some("lookup_order"))),
            "step 1 request must declare lookup_order tool"
        );

        let fn_call_item = json!({
            "type": "function_call",
            "name": "lookup_order",
            "call_id": "call_ord_42",
            "arguments": "{\"order_id\":\"ORD-42\"}"
        });
        write_sse_response(
            &mut stream1,
            &[
                json!({
                    "type": "response.output_item.done",
                    "item": fn_call_item.clone()
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_step_1",
                        "output": [fn_call_item],
                        "usage": { "input_tokens": 18, "output_tokens": 9 }
                    }
                }),
            ],
        );

        // Step 2: Receive replayed function_call_output, stream real text deltas.
        let (mut stream2, _) = listener.accept().expect("accept step 2 request");
        let req2_body = read_http_request_body(&mut stream2);
        assert!(
            req2_body.contains("function_call_output")
                && req2_body.contains("call_ord_42")
                && req2_body.contains("out_for_delivery"),
            "step 2 request must include executed tool output: {req2_body}"
        );

        write_sse_response(
            &mut stream2,
            &[
                json!({
                    "type": "response.output_text.delta",
                    "delta": "Order ORD-42 is "
                }),
                json!({
                    "type": "response.output_text.delta",
                    "delta": "out for delivery today."
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_step_2",
                        "output": [],
                        "usage": { "input_tokens": 29, "output_tokens": 11 }
                    }
                }),
            ],
        );
    });

    let deltas: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let deltas_for_cb = Arc::clone(&deltas);
    let bridge = Arc::new(LookupOrderBridge::new());

    let runner = AcpRunner::new("fabushi-default")
        .with_data_dir(temp_dir.path())
        .with_provider(RoutedProvider::Fabushi)
        .with_tool_bridge(bridge.clone())
        .with_text_delta_callback(Arc::new(move |delta, accumulated| {
            deltas_for_cb
                .lock()
                .unwrap()
                .push((delta.to_string(), accumulated.to_string()));
        }));

    let input = AgentTurnInput {
        session_id: "sess_positive_42".into(),
        prompt: "Where is order ORD-42?".into(),
        parent_turn_id: None,
        model_id: Some("grok-3".into()),
        thinking_budget_tokens: None,
    };

    let outputs = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("FABUSHI_RESPONSES_URL", Some(&endpoint)),
            (
                "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE",
                Some(cred_path.to_str().unwrap()),
            ),
            ("OPENROUTER_API_KEY", None),
        ]);
        runner
            .run_turn(input)
            .await
            .expect("real routed provider turn should succeed")
    };

    server_handle.join().expect("join loopback server");

    // Verify tool bridge was invoked once with exact arguments.
    let recorded_calls = bridge.calls.lock().unwrap();
    assert_eq!(recorded_calls.len(), 1);
    assert_eq!(recorded_calls[0].0, "lookup_order");
    assert_eq!(recorded_calls[0].1, json!({"order_id": "ORD-42"}));
    assert_eq!(recorded_calls[0].2, "call_ord_42");

    // Verify outputs contain ToolCall, ToolResult, and FinalResponse, and NO fabricated strings.
    assert!(outputs.iter().any(|out| matches!(
        out,
        AgentTurnOutput::ToolCall {
            tool_call_id,
            tool_name,
            arguments
        } if tool_call_id == "call_ord_42"
            && tool_name == "lookup_order"
            && arguments.contains("ORD-42")
    )));
    assert!(outputs.iter().any(|out| matches!(
        out,
        AgentTurnOutput::ToolResult {
            tool_call_id,
            output,
            is_error: false
        } if tool_call_id == "call_ord_42" && output.contains("out_for_delivery")
    )));
    assert!(outputs.iter().any(|out| matches!(
        out,
        AgentTurnOutput::FinalResponse { text }
            if text == "Order ORD-42 is out for delivery today."
    )));
    assert!(
        !outputs.iter().any(|out| match out {
            AgentTurnOutput::Thinking { token, .. } => token.contains("Reasoning with model"),
            AgentTurnOutput::FinalResponse { text } => {
                text.contains("Substantive analysis completed for")
                    || text.contains("Autonomous Agent Response")
            }
            _ => false,
        }),
        "AcpRunner must never emit fabricated thinking or canned response strings"
    );

    // Verify live text deltas came directly from the provider SSE chunks.
    let observed_deltas = deltas.lock().unwrap().clone();
    assert_eq!(
        observed_deltas,
        vec![
            ("Order ORD-42 is ".to_string(), "Order ORD-42 is ".to_string()),
            (
                "out for delivery today.".to_string(),
                "Order ORD-42 is out for delivery today.".to_string()
            ),
        ]
    );

    // Verify ProductionRoutedProviderCheckpointStore persisted the tool-step checkpoint on disk.
    let checkpoint_file = temp_dir
        .path()
        .join("runner-provider-checkpoints")
        .join("sess_positive_42")
        .join("current.json");
    assert!(
        checkpoint_file.is_file(),
        "ProductionRoutedProviderCheckpointStore must persist tool-step checkpoint at {}",
        checkpoint_file.display()
    );
}

#[tokio::test]
async fn in_flight_turn_cancellation_aborts_routed_provider_stream() {
    let temp_dir = ScopedTempDir::new("cancel_routed_provider");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://{addr}");
    let endpoint = format!("{issuer}/responses");
    let cred_path = write_private_fabushi_credential(temp_dir.path(), &issuer, "token-cancel-1");

    let (connected_tx, connected_rx) = std_mpsc::channel::<()>();
    let (release_tx, release_rx) = std_mpsc::channel::<()>();

    let server_handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept hanging stream");
        let _ = read_http_request_body(&mut stream);
        let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n";
        let _ = stream.write_all(headers.as_bytes());
        let _ = stream.flush();
        let _ = connected_tx.send(());
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
    });

    let runner = AcpRunner::new("grok-3")
        .with_data_dir(temp_dir.path())
        .with_provider(RoutedProvider::Fabushi);

    let runner_for_cancel = runner.clone();
    std::thread::spawn(move || {
        if connected_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
            std::thread::sleep(Duration::from_millis(40));
            let cancelled =
                runner_for_cancel.cancel_session("sess_cancel_99", "user clicked stop");
            assert!(cancelled, "active session should be cancelled");
        }
    });

    let input = AgentTurnInput {
        session_id: "sess_cancel_99".into(),
        prompt: "Stream a long response and cancel mid-flight".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };

    let result = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("FABUSHI_RESPONSES_URL", Some(&endpoint)),
            (
                "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE",
                Some(cred_path.to_str().unwrap()),
            ),
        ]);
        runner.execute_turn_stream(input).await
    };

    let _ = release_tx.send(());
    let _ = server_handle.join();

    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("cancelled turn must return Err, not Ok"),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("CANCELLED"),
        "expected CANCELLED error, got: {msg}"
    );
}

#[tokio::test]
async fn unconfigured_provider_fails_explicitly_without_fabricated_response() {
    let temp_dir = ScopedTempDir::new("negative_unconfigured");

    // Case 1: Fabushi provider with no FABUSHI_RESPONSES_URL / credential file configured.
    let fabushi_runner = AcpRunner::new("grok-3")
        .with_data_dir(temp_dir.path())
        .with_provider(RoutedProvider::Fabushi);
    let input = AgentTurnInput {
        session_id: "sess_unconfigured_fabushi".into(),
        prompt: "Attempt turn without credentials".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };

    let err = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("FABUSHI_INFERENCE_PROVIDER", None),
            ("SAND_INFERENCE_PROVIDER", None),
            ("FABUSHI_RESPONSES_URL", None),
            ("FABUSHI_HOST_ACCESS_CREDENTIAL_FILE", None),
            ("OPENROUTER_API_KEY", None),
            ("CODEX_HOME", None),
            ("CLAUDE_CODE_PATH", None),
        ]);
        match fabushi_runner.execute_turn_stream(input).await {
            Err(err) => err,
            Ok(_) => panic!("unconfigured Fabushi provider must fail explicitly"),
        }
    };
    assert!(
        matches!(err, TelegramError::Validation(_)),
        "expected TelegramError::Validation for missing Fabushi configuration, got: {err:?}"
    );
    assert!(
        err.to_string().contains("CONFIGURATION"),
        "error message must report CONFIGURATION failure: {err}"
    );

    // Case 2: OpenRouter provider with no OPENROUTER_API_KEY or box-secrets.json.
    let openrouter_runner = AcpRunner::new("grok-3")
        .with_data_dir(temp_dir.path())
        .with_provider(RoutedProvider::OpenRouter);
    let input2 = AgentTurnInput {
        session_id: "sess_unconfigured_openrouter".into(),
        prompt: "Attempt OpenRouter turn without API key".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };

    let err2 = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("OPENROUTER_API_KEY", None),
            ("SAND_DATA_ROOT", Some(temp_dir.path().to_str().unwrap())),
        ]);
        match openrouter_runner.execute_turn_stream(input2).await {
            Err(err) => err,
            Ok(_) => panic!("unconfigured OpenRouter provider must fail explicitly"),
        }
    };
    assert!(
        matches!(err2, TelegramError::AuthFailed(_)),
        "expected TelegramError::AuthFailed for missing OpenRouter key, got: {err2:?}"
    );
    assert!(
        err2.to_string().contains("AUTHENTICATION")
            && err2.to_string().contains("OPENROUTER_API_KEY"),
        "error message must report missing OPENROUTER_API_KEY: {err2}"
    );

    // Case 3: Invalid provider name in settings.json fails explicitly.
    std::fs::write(
        temp_dir.path().join("settings.json"),
        r#"{"inferenceProvider": "nonexistent-provider"}"#,
    )
    .expect("write settings.json");
    let invalid_settings_runner = AcpRunner::new("grok-3").with_data_dir(temp_dir.path());
    let input3 = AgentTurnInput {
        session_id: "sess_invalid_provider".into(),
        prompt: "Attempt turn with invalid settings provider".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };
    let err3 = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("FABUSHI_INFERENCE_PROVIDER", None),
            ("SAND_INFERENCE_PROVIDER", None),
        ]);
        match invalid_settings_runner.execute_turn_stream(input3).await {
            Err(err) => err,
            Ok(_) => panic!("invalid provider setting must fail explicitly"),
        }
    };
    assert!(
        err3.to_string().contains("Unsupported inference provider"),
        "expected unsupported provider message, got: {err3}"
    );
}

#[tokio::test]
async fn upstream_provider_http_and_sse_failures_propagate_explicitly() {
    let temp_dir = ScopedTempDir::new("negative_upstream_http");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://{addr}");
    let endpoint = format!("{issuer}/responses");
    let cred_path = write_private_fabushi_credential(temp_dir.path(), &issuer, "token-http-fail");

    let server_handle = std::thread::spawn(move || {
        // Request 1: Return HTTP 400 with structured provider refusal (non-retryable).
        if let Ok((mut stream1, _)) = listener.accept() {
            let _ = read_http_request_body(&mut stream1);
            let body = json!({
                "error": {
                    "code": "misalignment_policy_violation",
                    "misalignment": {
                        "detailed_explanation": "Request blocked by upstream safety policy",
                        "error_type": "policy_refusal"
                    }
                }
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream1.write_all(resp.as_bytes());
            let _ = stream1.flush();
        }

        // Request 2: Return SSE `response.failed` event (non-retryable protocol failure).
        if let Ok((mut stream2, _)) = listener.accept() {
            let _ = read_http_request_body(&mut stream2);
            write_sse_response(
                &mut stream2,
                &[json!({
                    "type": "response.failed",
                    "response": {
                        "error": {
                            "code": "invalid_model_parameter",
                            "message": "invalid model parameter supplied"
                        }
                    }
                })],
            );
        }
    });

    let runner = AcpRunner::new("grok-3")
        .with_data_dir(temp_dir.path())
        .with_provider(RoutedProvider::Fabushi);

    let refusal_input = AgentTurnInput {
        session_id: "sess_http_refusal".into(),
        prompt: "Trigger upstream HTTP 400 refusal".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };

    let sse_fail_input = AgentTurnInput {
        session_id: "sess_sse_failed".into(),
        prompt: "Trigger upstream SSE response.failed".into(),
        parent_turn_id: None,
        model_id: None,
        thinking_budget_tokens: None,
    };

    let (refusal_err, sse_fail_err) = {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _env = ScopedEnvVars::new(&[
            ("FABUSHI_RESPONSES_URL", Some(&endpoint)),
            (
                "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE",
                Some(cred_path.to_str().unwrap()),
            ),
        ]);
        let err1 = match runner.run_turn(refusal_input).await {
            Err(err) => err,
            Ok(outputs) => panic!("upstream refusal must fail, got outputs: {outputs:?}"),
        };
        let err2 = match runner.run_turn(sse_fail_input).await {
            Err(err) => err,
            Ok(outputs) => panic!("upstream SSE failure must fail, got outputs: {outputs:?}"),
        };
        (err1, err2)
    };

    server_handle.join().expect("join loopback server");

    assert!(
        matches!(refusal_err, TelegramError::PermissionDenied(_)),
        "expected TelegramError::PermissionDenied for provider refusal, got: {refusal_err:?}"
    );
    assert!(
        refusal_err
            .to_string()
            .contains("Request blocked by upstream safety policy"),
        "expected refusal explanation in error, got: {refusal_err}"
    );

    let sse_msg = sse_fail_err.to_string();
    assert!(
        sse_msg.contains("PROTOCOL") && sse_msg.contains("invalid model parameter supplied"),
        "expected upstream SSE failure detail in error, got: {sse_msg}"
    );
}
