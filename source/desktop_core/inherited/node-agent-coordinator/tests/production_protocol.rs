#[cfg(unix)]
mod unix {
    use mahayana_node_agent_coordinator::carrier::{
        CarrierChannel, CarrierEnvelope,
    };
    use mahayana_node_agent_coordinator::protocol::{
        CoordinatorFrame, Failure, LifecyclePhase, ReplyOutcome, COORDINATOR_DISCONNECTED,
        COORDINATOR_PROTOCOL_VERSION,
    };
    use serde_json::{Value, json};
    use std::fs;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn send(stdin: &mut impl Write, channel: CarrierChannel, frame: &CoordinatorFrame) {
        let envelope = CarrierEnvelope::new(
            channel,
            serde_json::to_value(frame).expect("serialize coordinator frame"),
        );
        serde_json::to_writer(&mut *stdin, &envelope).expect("serialize carrier envelope");
        writeln!(stdin).expect("write carrier envelope");
        stdin.flush().expect("flush carrier envelope");
    }

    struct FakeGateway {
        address: std::net::SocketAddr,
        token: String,
        oauth_callback_port: u16,
        oauth_completion: Arc<Mutex<Option<Value>>>,
        oauth_completion_attempts: Arc<AtomicUsize>,
        webauthn_batches: Arc<Mutex<Vec<Value>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl FakeGateway {
        fn start(data_dir: &Path) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Host gateway");
            listener
                .set_nonblocking(true)
                .expect("set fake Host gateway nonblocking");
            let address = listener.local_addr().expect("fake Host gateway address");
            let token = "production-protocol-token".to_string();
            let oauth_callback_probe =
                TcpListener::bind("127.0.0.1:0").expect("reserve OAuth callback port");
            let oauth_callback_port = oauth_callback_probe
                .local_addr()
                .expect("OAuth callback address")
                .port();
            drop(oauth_callback_probe);
            let oauth_completion = Arc::new(Mutex::new(None));
            let oauth_completion_attempts = Arc::new(AtomicUsize::new(0));
            let webauthn_batches = Arc::new(Mutex::new(Vec::new()));
            let crash_path = data_dir.join("fake-host-crash");
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = Arc::clone(&stop);
            let worker_token = token.clone();
            let worker_oauth_completion = Arc::clone(&oauth_completion);
            let worker_oauth_completion_attempts = Arc::clone(&oauth_completion_attempts);
            let worker_webauthn_batches = Arc::clone(&webauthn_batches);
            let worker = thread::spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let token = worker_token.clone();
                            let crash_path = crash_path.clone();
                            let handler_stop = Arc::clone(&worker_stop);
                            let oauth_completion = Arc::clone(&worker_oauth_completion);
                            let oauth_completion_attempts =
                                Arc::clone(&worker_oauth_completion_attempts);
                            let webauthn_batches = Arc::clone(&worker_webauthn_batches);
                            thread::spawn(move || {
                                let _ = serve_fake_gateway(
                                    stream,
                                    &token,
                                    &crash_path,
                                    handler_stop.as_ref(),
                                    oauth_callback_port,
                                    oauth_completion.as_ref(),
                                    oauth_completion_attempts.as_ref(),
                                    webauthn_batches.as_ref(),
                                );
                            });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => {
                            if worker_stop.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                }
            });
            Self {
                address,
                token,
                oauth_callback_port,
                oauth_completion,
                oauth_completion_attempts,
                webauthn_batches,
                stop,
                worker: Some(worker),
            }
        }

        fn port(&self) -> u16 {
            self.address.port()
        }

        fn token(&self) -> &str {
            &self.token
        }

        fn oauth_callback_port(&self) -> u16 {
            self.oauth_callback_port
        }

        fn oauth_completion_attempts(&self) -> usize {
            self.oauth_completion_attempts.load(Ordering::Acquire)
        }

        fn wait_for_oauth_completion(&self) -> Value {
            for _ in 0..200 {
                if let Some(value) = self
                    .oauth_completion
                    .lock()
                    .expect("OAuth completion lock")
                    .clone()
                {
                    return value;
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("shipping Coordinator did not complete MCP OAuth");
        }

        fn webauthn_result(&self) -> Option<Value> {
            self.webauthn_batches
                .lock()
                .expect("WebAuthn batches lock")
                .iter()
                .find_map(|batch| {
                    batch
                        .get("frames")
                        .and_then(Value::as_array)
                        .and_then(|frames| {
                            frames.iter().find_map(|frame| {
                                (frame.get("kind").and_then(Value::as_str) == Some("result"))
                                    .then(|| frame.clone())
                            })
                        })
                })
        }

        fn wait_for_webauthn_result(&self) -> Value {
            for _ in 0..300 {
                if let Some(value) = self.webauthn_result() {
                    return value;
                }
                thread::sleep(Duration::from_millis(10));
            }
            panic!("shipping Coordinator did not deliver the WebAuthn result");
        }
    }

    impl Drop for FakeGateway {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = TcpStream::connect(self.address);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn serve_fake_gateway(
        mut stream: TcpStream,
        expected_token: &str,
        crash_path: &Path,
        stop: &AtomicBool,
        oauth_callback_port: u16,
        oauth_completion: &Mutex<Option<Value>>,
        oauth_completion_attempts: &AtomicUsize,
        webauthn_batches: &Mutex<Vec<Value>>,
    ) -> io::Result<()> {
        // The fake gateway listener is non-blocking so its accept loop can poll for
        // shutdown. Accepted sockets may inherit that mode on macOS, making the
        // first read/write in a retry race with the client and fail spuriously.
        // Normalize each accepted HTTP exchange to blocking mode; the explicit
        // timeouts below still bound the test and preserve production semantics.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;

        let mut request = Vec::with_capacity(2048);
        let mut chunk = [0_u8; 2048];
        let header_end = loop {
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Ok(());
            }
            request.extend_from_slice(&chunk[..count]);
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            if request.len() > 64 * 1024 {
                return write_fake_response(&mut stream, 431, "");
            }
        };

        let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
        let mut lines = headers.lines();
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        let authorized = headers.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!("authorization: Bearer {expected_token}"))
        });
        if !authorized {
            return write_fake_response(&mut stream, 401, "");
        }

        let total = header_end.saturating_add(content_length);
        while request.len() < total {
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
        }
        if request.len() < total {
            return write_fake_response(&mut stream, 400, "");
        }

        match (method.as_str(), path.as_str()) {
            ("GET", "/webauthn/requests") => {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n"
                )?;
                writeln!(
                    stream,
                    "data: {}\n",
                    json!({
                        "kind": "welcome",
                        "providerId": "provider-production"
                    })
                )?;
                writeln!(
                    stream,
                    "data: {}\n",
                    json!({
                        "kind": "ceremony",
                        "requestId": "webauthn-production-1",
                        "ceremony": {
                            "kind": "get",
                            "origin": "https://example.test",
                            "optionsJson": "{\"rpId\":\"example.test\"}",
                            "challenge": "production"
                        }
                    })
                )?;
                stream.flush()?;
                while !stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            }
            ("POST", "/webauthn/responses") => {
                let payload = serde_json::from_slice::<Value>(&request[header_end..total])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                webauthn_batches
                    .lock()
                    .map_err(|_| io::Error::other("WebAuthn batches lock poisoned"))?
                    .push(payload);
                write_fake_response(&mut stream, 200, r#"{"ok":true}"#)
            }
            ("GET", "/events") => {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n"
                )?;
                writeln!(
                    stream,
                    "data: {}\n",
                    json!({
                        "channel": "runtime",
                        "payload": { "type": "host.test", "value": 1 }
                    })
                )?;
                writeln!(
                    stream,
                    "data: {}\n",
                    json!({
                        "channel": "mcp-oauth-pending",
                        "payload": {
                            "redirectUrl": format!(
                                "http://127.0.0.1:{oauth_callback_port}/oauth/callback"
                            ),
                            "state": "state-live",
                            "serverName": "github"
                        }
                    })
                )?;
                writeln!(
                    stream,
                    "data: {}\n",
                    json!({
                        "channel": "client-side-tool-v2",
                        "payload": {
                            "version": 1,
                            "kind": "call",
                            "accountSlot": "host",
                            "agentId": "agent-tool",
                            "epoch": "epoch-1",
                            "sequence": 1,
                            "message": {
                                "encoding": "protobuf-base64",
                                "messageType": "aiserver.v1.ClientSideToolV2Call",
                                "bytes": "GgZjYWxsLTc="
                            }
                        }
                    })
                )?;
                stream.flush()?;
                while !stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            }
            ("POST", "/api/echo") => write_fake_response(
                &mut stream,
                200,
                r#"{"echoed":true}"#,
            ),
            ("POST", "/api/sendPrompt") => write_fake_response(
                &mut stream,
                200,
                r#"{"accepted":true}"#,
            ),
            ("POST", "/api/completeMcpOAuth") => {
                let attempt = oauth_completion_attempts.fetch_add(1, Ordering::AcqRel);
                if attempt == 0 {
                    return write_fake_response(
                        &mut stream,
                        503,
                        r#"{"error":"transient oauth completion failure"}"#,
                    );
                }
                let payload = serde_json::from_slice::<Value>(&request[header_end..total])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                *oauth_completion
                    .lock()
                    .map_err(|_| io::Error::other("OAuth completion lock poisoned"))? =
                    Some(payload);
                write_fake_response(&mut stream, 200, r#"{"completed":true}"#)
            }
            ("POST", "/api/crash") => {
                fs::write(crash_path, b"crash")?;
                thread::sleep(Duration::from_millis(750));
                Ok(())
            }
            _ => write_fake_response(&mut stream, 404, ""),
        }
    }

    fn write_fake_response(stream: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            431 => "Request Header Fields Too Large",
            503 => "Service Unavailable",
            _ => "Response",
        };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?;
        stream.flush()
    }

    fn service_control_frame(
        stdin: &mut impl Write,
        seen_control_methods: &mut Vec<String>,
        frame: CoordinatorFrame,
    ) {
        match frame {
            CoordinatorFrame::Request {
                request_id,
                method,
                args,
            } => {
                seen_control_methods.push(method.clone());
                let outcome = match method.as_str() {
                    "resolveGatewayConnection" => ReplyOutcome::Failed {
                        failure: Failure::new(
                            "SAND_CLIENT_PAUSE",
                            "SAND_CLIENT_PAUSE: fake main keeps LocalExec paused",
                        ),
                    },
                    "mintLocalExecDaemonCredential" => ReplyOutcome::Ok {
                        value: Value::Null,
                    },
                    "requestWebAuthnConsent" => {
                        assert_eq!(args["origin"], "https://example.test");
                        assert_eq!(args["rpId"], "example.test");
                        ReplyOutcome::Ok {
                            value: json!({
                                "approved": true,
                                "promptId": "prompt-production",
                                "windowHandle": 4_294_967_297_u64
                            }),
                        }
                    }
                    "requestWebAuthnPin" => {
                        assert_eq!(args["promptId"], "prompt-production");
                        assert_eq!(args["invalid"], false);
                        ReplyOutcome::Ok {
                            value: json!({ "pin": "2468" }),
                        }
                    }
                    "updateWebAuthnConsent" => ReplyOutcome::Ok {
                        value: Value::Null,
                    },
                    "finishWebAuthnConsent" => ReplyOutcome::Ok {
                        value: Value::Null,
                    },
                    "getRpcTraceWindowTraceparent"
                    | "reportTransportStage"
                    | "reportGatewayCommandSpan" => ReplyOutcome::Ok {
                        value: Value::Null,
                    },
                    _ => ReplyOutcome::Failed {
                        failure: Failure::new(
                            "TEST_UNKNOWN_CONTROL_COMMAND",
                            format!("fake main does not implement {method}"),
                        ),
                    },
                };
                send(
                    stdin,
                    CarrierChannel::Control,
                    &CoordinatorFrame::Reply {
                        request_id,
                        outcome,
                    },
                );
            }
            other => panic!("unexpected post-handshake control frame: {other:?}"),
        }
    }

    fn recv_application_frame(
        rx: &mpsc::Receiver<Result<(CarrierChannel, CoordinatorFrame), serde_json::Error>>,
        stdin: &mut impl Write,
        seen_control_methods: &mut Vec<String>,
        timeout: Duration,
    ) -> (CarrierChannel, CoordinatorFrame) {
        loop {
            let (channel, frame) = rx
                .recv_timeout(timeout)
                .expect("Coordinator frame")
                .expect("valid Coordinator frame");
            if channel != CarrierChannel::Control {
                return (channel, frame);
            }
            service_control_frame(stdin, seen_control_methods, frame);
        }
    }

    fn pump_control_until_webauthn_result(
        rx: &mpsc::Receiver<Result<(CarrierChannel, CoordinatorFrame), serde_json::Error>>,
        stdin: &mut impl Write,
        seen_control_methods: &mut Vec<String>,
        gateway: &FakeGateway,
    ) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = gateway.webauthn_result() {
                return value;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "shipping Coordinator did not deliver the WebAuthn result"
            );
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok((CarrierChannel::Control, frame))) => {
                    service_control_frame(stdin, seen_control_methods, frame);
                }
                Ok(Ok((_channel, CoordinatorFrame::Event { .. }))) => {}
                Ok(Ok((channel, frame))) => panic!(
                    "unexpected application frame while awaiting WebAuthn result on {channel:?}: {frame:?}"
                ),
                Ok(Err(error)) => panic!("invalid Coordinator frame: {error}"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("Coordinator output closed while awaiting WebAuthn result")
                }
            }
        }
    }

    fn complete_live_oauth(port: u16) -> String {
        let mut stream = None;
        for _ in 0..200 {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(value) => {
                    stream = Some(value);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut stream = stream.expect("connect to Coordinator OAuth callback listener");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set OAuth callback read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("set OAuth callback write timeout");
        write!(
            stream,
            "GET /oauth/callback?code=code-live&state=state-live HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        )
        .expect("write OAuth callback");
        stream.flush().expect("flush OAuth callback");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read OAuth callback response");
        response
    }


    fn fake_webauthn_signer(data_dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let request_path = data_dir.join("webauthn-signer-request.json");
        let pin_path = data_dir.join("webauthn-signer-pin.json");
        let path = data_dir.join("fake-webauthn-signer.sh");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n\
                 IFS= read -r request\n\
                 printf '%s' \"$request\" > \"{}\"\n\
                 printf '%s\\n' '[signer-event] {{\"kind\":\"pin-required\"}}' >&2\n\
                 IFS= read -r pin\n\
                 printf '%s' \"$pin\" > \"{}\"\n\
                 printf '%s' '{{\"ok\":true,\"credentialJson\":{{\"id\":\"credential-production\"}}}}'\n",
                request_path.display(),
                pin_path.display(),
            ),
        )
        .expect("write fake WebAuthn signer");
        let mut permissions = fs::metadata(&path)
            .expect("fake WebAuthn signer metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("make fake WebAuthn signer executable");
        (path, request_path, pin_path)
    }

    fn fake_host() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mahayana-coordinator-production-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create fake host directory");
        let path = dir.join("fake-host.sh");
        fs::write(
            &path,
            r#"#!/bin/sh
cat > "$SAND_DATA_ROOT/gateway.json" <<EOF
{"port":${FABUSHI_TEST_GATEWAY_PORT},"pid":$$,"startedAt":1,"scheme":"http","host":"127.0.0.1","token":"${FABUSHI_TEST_GATEWAY_TOKEN}"}
EOF
printf '%s\n' '{"event":{"type":"host.test","value":1}}'
while [ ! -f "$SAND_DATA_ROOT/fake-host-crash" ]; do
  sleep 0.1
done
exit 17
"#,
        )
        .expect("write fake host");
        let mut permissions = fs::metadata(&path).expect("fake host metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("make fake host executable");
        path
    }


    #[test]
    fn shipping_coordinator_binary_enforces_protocol_and_settles_host_crash() {
        let host = fake_host();
        let data_dir = host.parent().expect("fake host parent").to_path_buf();
        let gateway = FakeGateway::start(&data_dir);
        let (webauthn_signer, webauthn_request_path, webauthn_pin_path) =
            fake_webauthn_signer(&data_dir);
        let coordinator = env!("CARGO_BIN_EXE_mahayana-node-agent-coordinator");
        let bootstrap = format!(
            "--bootstrap={}",
            json!({
                "processConfig": {
                    "appVersion": "test-0.18",
                    "isPackaged": false,
                    "dataDir": host.parent().expect("fake host parent").to_string_lossy()
                }
            })
        );
        let mut child = Command::new(coordinator)
            .arg(bootstrap)
            .env("MAHAYANA_APP_HOST_BIN", &host)
            .env("FABUSHI_TEST_GATEWAY_PORT", gateway.port().to_string())
            .env("FABUSHI_TEST_GATEWAY_TOKEN", gateway.token())
            .env("SAND_WEBAUTHN_SIGNER_PATH", &webauthn_signer)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn shipping coordinator binary");

        let mut stdin = child.stdin.take().expect("coordinator stdin");
        let stdout = child.stdout.take().expect("coordinator stdout");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let parsed = serde_json::from_str::<CarrierEnvelope>(&line).and_then(|envelope| {
                    let channel = envelope
                        .classify()
                        .ok_or_else(|| serde_json::Error::io(std::io::Error::other("unknown channel")))?;
                    let frame = serde_json::from_value::<CoordinatorFrame>(envelope.frame)?;
                    Ok((channel, frame))
                });
                if tx.send(parsed).is_err() {
                    break;
                }
            }
        });

        let (channel, hello) = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("control hello")
            .expect("valid control hello");
        assert_eq!(channel, CarrierChannel::Control);
        assert_eq!(hello, CoordinatorFrame::hello());
        send(&mut stdin, CarrierChannel::Control, &CoordinatorFrame::ready());
        let mut seen_control_methods = Vec::new();

        for channel in [CarrierChannel::Data, CarrierChannel::MainData] {
            send(
                &mut stdin,
                channel,
                &CoordinatorFrame::Lifecycle {
                    phase: LifecyclePhase::Hello,
                    protocol_version: Some(COORDINATOR_PROTOCOL_VERSION),
                    reason: None,
                    detail: None,
                },
            );
            assert_eq!(
                recv_application_frame(
                    &rx,
                    &mut stdin,
                    &mut seen_control_methods,
                    Duration::from_secs(2),
                ),
                (channel, CoordinatorFrame::ready())
            );
        }

        send(
            &mut stdin,
            CarrierChannel::Data,
            &CoordinatorFrame::Request {
                request_id: "r-health".into(),
                method: "coordinator.health".into(),
                args: json!({}),
            },
        );
        let (channel, health) = recv_application_frame(
            &rx,
            &mut stdin,
            &mut seen_control_methods,
            Duration::from_secs(2),
        );
        assert_eq!(channel, CarrierChannel::Data);
        match health {
            CoordinatorFrame::Reply {
                request_id,
                outcome: ReplyOutcome::Ok { value },
            } => {
                assert_eq!(request_id, "r-health");
                assert_eq!(value["protocolVersion"], 1);
                assert_eq!(value["processConfig"]["appVersion"], "test-0.18");
                assert_eq!(value["processConfig"]["isPackaged"], false);
                assert_eq!(value["inference"]["provider"], "cursor");
                assert_eq!(value["inference"]["queueWorkers"], 0);
            }
            other => panic!("unexpected health frame: {other:?}"),
        }

        send(
            &mut stdin,
            CarrierChannel::Data,
            &CoordinatorFrame::Request {
                request_id: "r-echo".into(),
                method: "echo".into(),
                args: json!({"value": 1}),
            },
        );

        let mut saw_host_running = false;
        let mut saw_runtime_event = false;
        let mut saw_tool_event = false;
        let mut saw_echo_reply = false;
        for _ in 0..12 {
            let (channel, frame) = recv_application_frame(
                &rx,
                &mut stdin,
                &mut seen_control_methods,
                Duration::from_secs(2),
            );
            assert_eq!(channel, CarrierChannel::Data);
            match frame {
                CoordinatorFrame::Event { family, payload } if family == "runtime" => {
                    if payload.get("type").and_then(|value| value.as_str()) == Some("host.lifecycle")
                        && payload.get("lifecycle").and_then(|value| value.as_str()) == Some("running")
                    {
                        saw_host_running = true;
                    }
                    if payload.get("type").and_then(|value| value.as_str()) == Some("host.test") {
                        saw_runtime_event = true;
                    }
                }
                CoordinatorFrame::Event { family, payload }
                    if family == "client-side-tool-v2" =>
                {
                    assert_eq!(payload["agentId"], "agent-tool");
                    assert_eq!(payload["epoch"], "epoch-1");
                    assert_eq!(payload["sequence"], 1);
                    saw_tool_event = true;
                }
                CoordinatorFrame::Reply {
                    request_id,
                    outcome: ReplyOutcome::Ok { value },
                } if request_id == "r-echo" => {
                    assert_eq!(value, json!({"echoed": true}));
                    saw_echo_reply = true;
                }
                _ => {}
            }
            if saw_host_running && saw_runtime_event && saw_tool_event && saw_echo_reply {
                break;
            }
        }
        assert!(saw_host_running, "shipping Coordinator did not expose Host lifecycle");
        assert!(saw_runtime_event, "shipping Coordinator did not relay Host event");
        assert!(saw_tool_event, "shipping Coordinator did not relay client-side tool events");
        assert!(saw_echo_reply, "shipping Coordinator did not settle Host reply");

        send(
            &mut stdin,
            CarrierChannel::Data,
            &CoordinatorFrame::Request {
                request_id: "r-send".into(),
                method: "sendPrompt".into(),
                args: json!({
                    "clientNonce": "production-send-1",
                    "message": "production gateway command policy"
                }),
            },
        );
        let (channel, send_reply) = recv_application_frame(
            &rx,
            &mut stdin,
            &mut seen_control_methods,
            Duration::from_secs(2),
        );
        assert_eq!(channel, CarrierChannel::Data);
        match send_reply {
            CoordinatorFrame::Reply {
                request_id,
                outcome: ReplyOutcome::Ok { value },
            } => {
                assert_eq!(request_id, "r-send");
                assert_eq!(value["accepted"], true);
            }
            other => panic!("unexpected sendPrompt frame: {other:?}"),
        }

        let webauthn_result = pump_control_until_webauthn_result(
            &rx,
            &mut stdin,
            &mut seen_control_methods,
            &gateway,
        );
        assert_eq!(webauthn_result["credentialJson"]["id"], "credential-production");
        let signer_request: Value = serde_json::from_str(
            &fs::read_to_string(&webauthn_request_path).expect("read WebAuthn signer request"),
        )
        .expect("WebAuthn signer request JSON");
        assert_eq!(signer_request["windowHandle"].as_u64(), Some(4_294_967_297));
        let signer_pin: Value = serde_json::from_str(
            &fs::read_to_string(&webauthn_pin_path).expect("read WebAuthn signer PIN reply"),
        )
        .expect("WebAuthn signer PIN JSON");
        assert_eq!(signer_pin, json!({ "kind": "pin", "pin": "2468" }));
        for method in [
            "requestWebAuthnConsent",
            "requestWebAuthnPin",
            "finishWebAuthnConsent",
        ] {
            assert!(
                seen_control_methods.iter().any(|seen| seen == method),
                "shipping Coordinator did not wire production WebAuthn control method {method}"
            );
        }

        assert!(
            seen_control_methods
                .iter()
                .any(|method| method == "resolveGatewayConnection"),
            "shipping Coordinator did not start LocalExec through the control port"
        );
        assert!(
            seen_control_methods
                .iter()
                .any(|method| method == "mintLocalExecDaemonCredential"),
            "shipping Coordinator did not request the LocalExec credential through the control port"
        );

        let oauth_response = complete_live_oauth(gateway.oauth_callback_port());
        assert!(
            oauth_response.starts_with("HTTP/1.1 200 OK"),
            "Coordinator OAuth callback did not complete successfully: {oauth_response}"
        );
        assert_eq!(
            gateway.wait_for_oauth_completion(),
            json!({
                "code": "code-live",
                "state": "state-live"
            }),
            "Coordinator did not forward the browser callback to completeMcpOAuth"
        );
        assert_eq!(
            gateway.oauth_completion_attempts(),
            2,
            "shipping Coordinator did not retry one transient OAuth completion failure"
        );

        send(
            &mut stdin,
            CarrierChannel::Data,
            &CoordinatorFrame::Request {
                request_id: "r-crash".into(),
                method: "crash".into(),
                args: json!({}),
            },
        );

        let expected_failure = Failure::new(
            COORDINATOR_DISCONNECTED,
            "placeholder overwritten below",
        );
        let mut saw_crash_reply = false;
        let mut saw_stopped = false;
        for _ in 0..10 {
            let (channel, frame) = recv_application_frame(
                &rx,
                &mut stdin,
                &mut seen_control_methods,
                Duration::from_secs(3),
            );
            assert_eq!(channel, CarrierChannel::Data);
            match frame {
                CoordinatorFrame::Reply {
                    request_id,
                    outcome: ReplyOutcome::Failed { failure },
                } if request_id == "r-crash" => {
                    assert_eq!(failure.code, expected_failure.code);
                    assert!(failure.message.contains("Host exited"));
                    saw_crash_reply = true;
                }
                CoordinatorFrame::Event { family, payload } if family == "runtime" => {
                    if payload.get("type").and_then(|value| value.as_str()) == Some("host.lifecycle")
                        && payload.get("lifecycle").and_then(|value| value.as_str()) == Some("stopped")
                    {
                        saw_stopped = true;
                    }
                }
                _ => {}
            }
            if saw_crash_reply && saw_stopped {
                break;
            }
        }
        assert!(saw_crash_reply, "Host crash did not reject the pending request");
        assert!(saw_stopped, "Host crash did not emit stopped lifecycle");

        fs::write(
            data_dir.join("settings.json"),
            r#"{"inferenceProvider":"codex"}"#,
        )
        .expect("switch shipping Coordinator to local inference provider");
        send(
            &mut stdin,
            CarrierChannel::Data,
            &CoordinatorFrame::Request {
                request_id: "r-local-inference".into(),
                method: "sendPrompt".into(),
                args: json!({
                    "agentId": "agent-local",
                    "prompt": "",
                    "clientNonce": "local-inference-1"
                }),
            },
        );
        let mut saw_local_accept = false;
        for _ in 0..8 {
            let (channel, frame) = recv_application_frame(
                &rx,
                &mut stdin,
                &mut seen_control_methods,
                Duration::from_secs(2),
            );
            assert_eq!(channel, CarrierChannel::Data);
            if let CoordinatorFrame::Reply {
                request_id,
                outcome: ReplyOutcome::Ok { value },
            } = frame
                && request_id == "r-local-inference"
            {
                assert_eq!(value["accepted"], true);
                assert_eq!(value["provider"], "codex");
                assert_eq!(value["clientNonce"], "local-inference-1");
                saw_local_accept = true;
                break;
            }
        }
        assert!(
            saw_local_accept,
            "shipping Coordinator did not accept a local-provider sendPrompt"
        );

        // Provider/router failures remain recoverable diagnostics, but the
        // renderer-visible transcript is Host-owned and must not receive a
        // synthetic assistant "Router error" message.
        let inference_path = data_dir.join("inference-router-transcript.json");
        let mut saw_private_local_error = false;
        for _ in 0..100 {
            if let Ok(raw) = fs::read_to_string(&inference_path)
                && let Ok(value) = serde_json::from_str::<Value>(&raw)
            {
                saw_private_local_error = value["agents"]["agent-local"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|entry| {
                        entry["role"] == "assistant"
                            && entry["content"]
                                .as_str()
                                .is_some_and(|content| content.contains("Router error:"))
                    });
                if saw_private_local_error {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            saw_private_local_error,
            "shipping Coordinator did not retain a failed local-provider turn in private inference diagnostics"
        );

        drop(stdin);
        let status = child.wait().expect("wait for Coordinator");
        assert!(status.success(), "Coordinator did not shut down cleanly: {status}");
        drop(gateway);
        let _ = fs::remove_dir_all(data_dir);
    }
}
