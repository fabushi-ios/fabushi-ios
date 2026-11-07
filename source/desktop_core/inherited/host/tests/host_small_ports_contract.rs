use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mahayana_host_runtime::r#box::box_env::{
    BoxEnvironmentControlClient, BoxEnvironmentUpdate, apply_box_environment_via_transport,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedEnvironmentCall {
    ctx: String,
    update: BoxEnvironmentUpdate,
}

struct RecordingTransport {
    calls: Rc<RefCell<Vec<RecordedEnvironmentCall>>>,
}

struct RecordingControlClient {
    calls: Rc<RefCell<Vec<RecordedEnvironmentCall>>>,
}

impl BoxEnvironmentControlClient<String> for RecordingControlClient {
    type Error = &'static str;

    fn update_environment_variables(
        &mut self,
        ctx: &String,
        request: BoxEnvironmentUpdate,
    ) -> Result<(), Self::Error> {
        self.calls.borrow_mut().push(RecordedEnvironmentCall {
            ctx: ctx.clone(),
            update: request,
        });
        Ok(())
    }
}

#[test]
fn box_environment_port_is_wired_and_forwards_a_cloned_update() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let transport = RecordingTransport {
        calls: Rc::clone(&calls),
    };
    let ctx = "request-context".to_string();
    let update = BoxEnvironmentUpdate {
        env: BTreeMap::from([
            ("FABUSHI_AGENT".to_string(), "enabled".to_string()),
            ("SHELL".to_string(), "/bin/zsh".to_string()),
        ]),
        replace: true,
    };
    let original = update.clone();

    apply_box_environment_via_transport(&ctx, &transport, &update, |transport| {
        RecordingControlClient {
            calls: Rc::clone(&transport.calls),
        }
    })
    .expect("box environment transport should succeed");

    assert_eq!(update, original, "the caller-owned update must remain unchanged");
    assert_eq!(
        calls.borrow().as_slice(),
        &[RecordedEnvironmentCall {
            ctx,
            update: original,
        }]
    );
}

#[test]
fn box_shell_command_builder_matches_grok_host_contract() {
    use mahayana_host_runtime::r#box::box_shell_command::{
        HostShellArgsInput, ShellCommandExecutable, ShellCommandParsingResult, build_host_shell_args,
    };

    let args = build_host_shell_args(HostShellArgsInput {
        command: "git status --short".to_string(),
        name: "git".to_string(),
        working_directory: "/workspace".to_string(),
        tool_call_id: "tool-42".to_string(),
    });

    assert_eq!(args.command, "git status --short");
    assert_eq!(args.working_directory, "/workspace");
    assert_eq!(args.tool_call_id, "tool-42");
    assert!(args.skip_approval);
    assert_eq!(
        args.parsing_result,
        ShellCommandParsingResult {
            parsing_failed: false,
            executable_commands: vec![ShellCommandExecutable {
                name: "git".to_string(),
                args: Vec::new(),
                full_text: "git status --short".to_string(),
            }],
            has_redirects: false,
            has_command_substitution: false,
        }
    );
}

#[test]
fn host_request_context_prefers_injected_timezone_and_normalizes_identity() {
    use mahayana_host_runtime::host_request_context::create_host_request_context;

    let provider = create_host_request_context(
        "/tmp/transcripts",
        || Some("Asia/Shanghai".to_string()),
        || vec!["rule-a", "rule-b"],
        || Some("  Gloria   Chan  ".to_string()),
    );

    let resolved = provider.resolve();
    assert_eq!(resolved.transcripts_folder, "/tmp/transcripts");
    assert_eq!(resolved.time_zone.as_deref(), Some("Asia/Shanghai"));
    assert_eq!(resolved.user_full_name.as_deref(), Some("Gloria Chan"));
    assert!(!resolved.os_version.trim().is_empty());
    assert_eq!(provider.resolve_rules(), vec!["rule-a", "rule-b"]);
}

#[test]
fn host_request_context_omits_blank_user_identity() {
    use mahayana_host_runtime::host_request_context::create_host_request_context;

    let provider = create_host_request_context(
        "/tmp/transcripts",
        || Some("UTC".to_string()),
        || Vec::<String>::new(),
        || Some("   ".to_string()),
    );

    assert_eq!(provider.resolve().user_full_name, None);
}

#[test]
fn ua_token_kill_switch_retries_failures_and_reconciles_marker() {
    use std::cell::Cell;
    use std::fs;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use mahayana_host_runtime::extensions::browser_ua::ua_token_kill_switch_service::create_ua_token_kill_switch_reconciler;

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("fabushi-ua-kill-switch-{suffix}"));
    let marker = root.join("nested").join("disabled");
    let enabled = Rc::new(Cell::new(true));
    let log_count = Rc::new(Cell::new(0usize));

    let enabled_for_reconciler = Rc::clone(&enabled);
    let logs_for_reconciler = Rc::clone(&log_count);
    let mut reconcile = create_ua_token_kill_switch_reconciler(
        Some(marker.clone()),
        move || enabled_for_reconciler.get(),
        move |_message| logs_for_reconciler.set(logs_for_reconciler.get() + 1),
    );

    reconcile.reconcile();
    assert_eq!(log_count.get(), 1);
    assert_eq!(reconcile.last_applied(), None);

    fs::create_dir_all(marker.parent().expect("marker parent")).expect("create marker parent");
    reconcile.reconcile();
    assert_eq!(fs::read_to_string(&marker).expect("marker text"), "1\n");
    assert_eq!(reconcile.last_applied(), Some(true));

    enabled.set(false);
    reconcile.reconcile();
    assert!(!marker.exists());
    assert_eq!(reconcile.last_applied(), Some(false));

    fs::remove_dir_all(root).expect("remove UA test root");
}


#[test]
fn production_box_environment_uses_connect_unary_control_service_from_host_graph() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use mahayana_host_runtime::r#box::production::ProductionBoxEnvironment;

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout");
        let mut received = Vec::new();
        let mut buffer = [0u8; 1024];
        let header_end;
        loop {
            let count = stream.read(&mut buffer).expect("read HTTP request");
            assert!(count > 0, "client closed before HTTP headers completed");
            received.extend_from_slice(&buffer[..count]);
            if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = index;
                break;
            }
        }
        let header_text =
            std::str::from_utf8(&received[..header_end]).expect("UTF-8 request headers");
        let content_length = header_text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .expect("content-length header");
        while received.len() < header_end + 4 + content_length {
            let count = stream.read(&mut buffer).expect("read HTTP body");
            assert!(count > 0, "client closed before HTTP body completed");
            received.extend_from_slice(&buffer[..count]);
        }
        received
    }

    fn reply_ok(stream: &mut TcpStream) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("write fake Connect response");
        stream.flush().expect("flush fake Connect response");
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake exec daemon");
    let port = listener.local_addr().expect("fake daemon address").port();
    let server = thread::spawn(move || {
        let (mut ping_stream, _) = listener.accept().expect("accept readiness Ping");
        let ping = read_request(&mut ping_stream);
        let ping_headers = request_parts(&ping).0;
        assert!(ping_headers.starts_with(
            "POST /agent.v1.ControlService/Ping HTTP/1.1\r\n"
        ));
        assert!(
            ping_headers
                .lines()
                .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer test-token"))
        );
        reply_ok(&mut ping_stream);
        // Production ControlService calls use Connection: close and read to EOF.
        // Close the Ping response before accepting the independent update request.
        drop(ping_stream);

        let (mut update_stream, _) = listener.accept().expect("accept environment update");
        let update = read_request(&mut update_stream);
        reply_ok(&mut update_stream);
        update
    });

    let service = ProductionBoxEnvironment::new("127.0.0.1", port, "test-token");
    let update = BoxEnvironmentUpdate {
        env: BTreeMap::from([
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "two".to_string()),
        ]),
        replace: true,
    };
    service
        .apply_environment(&update)
        .expect("shipping production environment call");

    let request = server.join().expect("fake daemon server");
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator");
    let headers = std::str::from_utf8(&request[..header_end]).expect("request headers");
    assert!(headers.starts_with(
        "POST /agent.v1.ControlService/UpdateEnvironmentVariables HTTP/1.1\r\n"
    ));
    let header_value = |expected_name: &str| {
        headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(expected_name).then_some(value.trim())
        })
    };
    assert_eq!(header_value("authorization"), Some("Bearer test-token"));
    assert_eq!(header_value("content-type"), Some("application/proto"));
    assert_eq!(header_value("connect-protocol-version"), Some("1"));

    let body = &request[header_end + 4..];
    assert_eq!(
        body,
        &[
            0x0a, 0x06, 0x0a, 0x01, b'A', 0x12, 0x01, b'1',
            0x0a, 0x08, 0x0a, 0x01, b'B', 0x12, 0x03, b't', b'w', b'o',
            0x10, 0x01,
        ]
    );
}

fn spawn_fake_production_control_service(
    response_body: Vec<u8>,
) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake control service");
    let port = listener.local_addr().expect("fake control service address").port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept production box client");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout");

        let mut received = Vec::new();
        let mut buffer = [0u8; 1024];
        let header_end;
        loop {
            let count = stream.read(&mut buffer).expect("read HTTP request");
            assert!(count > 0, "client closed before HTTP headers completed");
            received.extend_from_slice(&buffer[..count]);
            if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = index;
                break;
            }
        }
        let header_text =
            std::str::from_utf8(&received[..header_end]).expect("UTF-8 request headers");
        let content_length = header_text
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .expect("content-length header");
        while received.len() < header_end + 4 + content_length {
            let count = stream.read(&mut buffer).expect("read HTTP body");
            assert!(count > 0, "client closed before HTTP body completed");
            received.extend_from_slice(&buffer[..count]);
        }

        let response_head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        );
        stream
            .write_all(response_head.as_bytes())
            .expect("write fake Connect response headers");
        stream
            .write_all(&response_body)
            .expect("write fake Connect response body");
        stream.flush().expect("flush fake Connect response");
        received
    });
    (port, server)
}

fn request_parts(request: &[u8]) -> (&str, &[u8]) {
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP header terminator");
    (
        std::str::from_utf8(&request[..header_end]).expect("UTF-8 request headers"),
        &request[header_end + 4..],
    )
}

#[test]
fn production_box_control_service_ping_is_authenticated_and_uses_frozen_path() {
    use mahayana_host_runtime::r#box::box_remote_accessor::{
        BoxEndpoint, BoxPingControlClient,
    };
    use mahayana_host_runtime::r#box::generated_production::{
        ProductionBoxTransport, create_production_box_control_client,
    };

    let (port, server) = spawn_fake_production_control_service(Vec::new());
    let endpoint = BoxEndpoint::new("127.0.0.1", port, "ping-token");
    let transport = ProductionBoxTransport::from_endpoint(&endpoint);
    let mut client = create_production_box_control_client(&transport);
    client
        .ping(&(), 1_500)
        .expect("production Ping should reach fake exec daemon");

    let request = server.join().expect("fake Ping server");
    let (headers, body) = request_parts(&request);
    assert!(headers.starts_with("POST /agent.v1.ControlService/Ping HTTP/1.1\r\n"));
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer ping-token"))
    );
    assert!(body.is_empty(), "PingRequest has no protobuf fields");
}

#[test]
fn production_box_control_service_load_mcp_servers_round_trips_frozen_proto() {
    use mahayana_host_runtime::r#box::box_mcp::{
        BoxMcpControlClient, BoxMcpLoadRequest,
    };
    use mahayana_host_runtime::r#box::box_remote_accessor::BoxEndpoint;
    use mahayana_host_runtime::r#box::generated_production::{
        ProductionBoxTransport, create_production_box_control_client,
    };

    let response = vec![
        0x0a, 0x0a, b'f', b'i', b'l', b'e', b's', b'y', b's', b't', b'e', b'm',
        0x0a, 0x07, b'b', b'r', b'o', b'w', b's', b'e', b'r',
    ];
    let (port, server) = spawn_fake_production_control_service(response);
    let endpoint = BoxEndpoint::new("127.0.0.1", port, "mcp-token");
    let transport = ProductionBoxTransport::from_endpoint(&endpoint);
    let mut client = create_production_box_control_client(&transport);
    let loaded = client
        .load_mcp_servers(
            &(),
            BoxMcpLoadRequest {
                mcp_config_json: "{}".into(),
                remove_missing: true,
            },
        )
        .expect("production MCP load should decode the response");
    assert_eq!(loaded.loaded_server_names, vec!["filesystem", "browser"]);

    let request = server.join().expect("fake MCP server");
    let (headers, body) = request_parts(&request);
    assert!(headers.starts_with(
        "POST /agent.v1.ControlService/LoadMcpServers HTTP/1.1\r\n"
    ));
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer mcp-token"))
    );
    assert_eq!(body, &[0x0a, 0x02, b'{', b'}', 0x10, 0x01]);
}
