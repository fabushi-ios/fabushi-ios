use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::r#box::box_file_transfer::{
    FileTransferAccessor, WriteExecResult,
};
use mahayana_host_runtime::r#box::box_shell_command::{
    HostShellArgsInput, build_host_shell_args,
};
use mahayana_host_runtime::r#box::box_windows::{
    ShellAccessor, ShellExecutionOutcome,
};
use mahayana_host_runtime::r#box::generated_production::{
    BACKGROUND_SHELL_SPAWN_FIELD_NUMBER, CONNECT_STREAM_CONTENT_TYPE, EXEC_PATH, PING_PATH,
    ProductionBackgroundShellSpawnArgs, ProductionBackgroundShellSpawnResult,
    ProductionBoxExecError, ProductionReadArgs, ProductionReadOutput, ProductionReadResult,
    ProductionShellResult, ProductionShellStreamArgs, ProductionShellStreamEvent,
    SHELL_BACKGROUND_REASON_TIMEOUT,
};
use mahayana_host_runtime::r#box::box_factory::{
    format_sand_box_startup_summary, should_apply_shared_desktop,
};
use mahayana_host_runtime::r#box::loopback_sand_box::{
    LoopbackSandBox, LoopbackSandBoxOptions,
};
use mahayana_host_runtime::r#box::production::ProductionBoxEnvironment;
use mahayana_host_runtime::extensions::box_lifecycle::RecreateSandBoxResponse;
use mahayana_host_runtime::extensions::forever_box::{
    ForeverBoxLifecycle, ForeverBoxRunnerResourcePort, ForeverBoxService, HostBox,
};
use mahayana_host_runtime::runner::background_work::RunnerBackgroundShellWatches;
use mahayana_host_runtime::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest,
};

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_key(field: u32, wire: u8, out: &mut Vec<u8>) {
    encode_varint((u64::from(field) << 3) | u64::from(wire), out);
}

fn push_varint(field: u32, value: u64, out: &mut Vec<u8>) {
    push_key(field, 0, out);
    encode_varint(value, out);
}

fn push_len(field: u32, value: &[u8], out: &mut Vec<u8>) {
    push_key(field, 2, out);
    encode_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn connect_envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(flags);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn background_shell_spawn_element(result: &[u8]) -> Vec<u8> {
    let mut client_message = Vec::new();
    push_len(BACKGROUND_SHELL_SPAWN_FIELD_NUMBER, result, &mut client_message);
    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn background_shell_spawn_success(
    shell_id: u32,
    command: &str,
    working_directory: &str,
    pid: Option<u32>,
) -> Vec<u8> {
    let mut success = Vec::new();
    push_varint(1, u64::from(shell_id), &mut success);
    push_len(2, command.as_bytes(), &mut success);
    push_len(3, working_directory.as_bytes(), &mut success);
    if let Some(pid) = pid {
        push_varint(4, u64::from(pid), &mut success);
    }
    let mut result = Vec::new();
    push_len(1, &success, &mut result);
    background_shell_spawn_element(&result)
}

fn background_shell_spawn_error(
    case: u32,
    command: &str,
    working_directory: &str,
    detail: &str,
) -> Vec<u8> {
    let mut nested = Vec::new();
    push_len(1, command.as_bytes(), &mut nested);
    push_len(2, working_directory.as_bytes(), &mut nested);
    push_len(3, detail.as_bytes(), &mut nested);
    if case == 3 || case == 4 {
        push_varint(4, 1, &mut nested);
    }
    let mut result = Vec::new();
    push_len(case, &nested, &mut result);
    background_shell_spawn_element(&result)
}

fn background_shell_spawn_sandbox_unsupported(
    command: &str,
    working_directory: &str,
    policy_type: u32,
    reason: &str,
) -> Vec<u8> {
    let mut nested = Vec::new();
    push_len(1, command.as_bytes(), &mut nested);
    push_len(2, working_directory.as_bytes(), &mut nested);
    push_varint(3, u64::from(policy_type), &mut nested);
    push_len(4, reason.as_bytes(), &mut nested);
    push_varint(5, 1, &mut nested);
    let mut result = Vec::new();
    push_len(5, &nested, &mut result);
    background_shell_spawn_element(&result)
}

fn shell_stream_element(event_case: u32, nested: &[u8]) -> Vec<u8> {
    let mut shell_stream = Vec::new();
    push_len(event_case, nested, &mut shell_stream);
    let mut client_message = Vec::new();
    push_len(14, &shell_stream, &mut client_message);
    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn shell_stream_start_element() -> Vec<u8> {
    shell_stream_element(4, &[])
}

fn shell_stream_stdout_element(data: &str) -> Vec<u8> {
    let mut stdout = Vec::new();
    push_len(1, data.as_bytes(), &mut stdout);
    shell_stream_element(1, &stdout)
}

fn shell_stream_backgrounded_element(
    shell_id: u32,
    command: &str,
    working_directory: &str,
    pid: Option<u32>,
    ms_to_wait: Option<i32>,
    reason: Option<i32>,
) -> Vec<u8> {
    let mut backgrounded = Vec::new();
    push_varint(1, u64::from(shell_id), &mut backgrounded);
    push_len(2, command.as_bytes(), &mut backgrounded);
    push_len(3, working_directory.as_bytes(), &mut backgrounded);
    if let Some(pid) = pid {
        push_varint(4, u64::from(pid), &mut backgrounded);
    }
    if let Some(ms_to_wait) = ms_to_wait {
        push_varint(5, ms_to_wait as u32 as u64, &mut backgrounded);
    }
    if let Some(reason) = reason {
        push_varint(6, reason as u32 as u64, &mut backgrounded);
    }
    shell_stream_element(7, &backgrounded)
}

fn shell_success_element(stderr: &str) -> Vec<u8> {
    shell_success_element_with_output("", stderr)
}

fn shell_success_element_with_output(stdout: &str, stderr: &str) -> Vec<u8> {
    let mut success = Vec::new();
    push_varint(3, 0, &mut success);
    push_len(5, stdout.as_bytes(), &mut success);
    push_len(6, stderr.as_bytes(), &mut success);

    let mut shell_result = Vec::new();
    push_len(1, &success, &mut shell_result);

    let mut client_message = Vec::new();
    push_len(2, &shell_result, &mut client_message);

    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn write_success_element() -> Vec<u8> {
    let mut write_result = Vec::new();
    push_len(1, &[], &mut write_result);

    let mut client_message = Vec::new();
    push_len(3, &write_result, &mut client_message);

    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn read_success_element(path: &str, content: &str) -> Vec<u8> {
    let mut success = Vec::new();
    push_len(1, path.as_bytes(), &mut success);
    push_len(2, content.as_bytes(), &mut success);
    push_varint(3, 2, &mut success);
    push_varint(4, content.len() as u64, &mut success);
    push_varint(6, 0, &mut success);
    push_varint(8, 0, &mut success);

    let mut read_result = Vec::new();
    push_len(1, &success, &mut read_result);

    let mut client_message = Vec::new();
    push_len(7, &read_result, &mut client_message);

    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn computer_use_success_element() -> Vec<u8> {
    let mut computer_result = Vec::new();
    push_len(1, &[], &mut computer_result);

    let mut client_message = Vec::new();
    push_len(22, &computer_result, &mut client_message);

    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn raw_resource_element(field_number: u32, protobuf_result: &[u8]) -> Vec<u8> {
    let mut client_message = Vec::new();
    push_len(field_number, protobuf_result, &mut client_message);

    let mut element = Vec::new();
    push_len(1, &client_message, &mut element);
    element
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut expected_total = None;
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        assert!(read > 0, "client closed before request was complete");
        request.extend_from_slice(&buffer[..read]);
        if expected_total.is_none() {
            if let Some(header_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
            {
                let headers = std::str::from_utf8(&request[..header_end])
                    .expect("UTF-8 request headers");
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                    })
                    .expect("content length header");
                expected_total = Some(header_end + 4 + content_length);
            }
        }
        if expected_total.is_some_and(|expected| request.len() >= expected) {
            return request;
        }
    }
}

fn serve_exec_response(
    listener: &TcpListener,
    expected_fragment: &[u8],
    element: Vec<u8>,
) {
    let (mut stream, _) = listener.accept().expect("accept ExecService request");
    let request = read_request(&mut stream);
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header end");
    let headers = std::str::from_utf8(&request[..header_end]).expect("headers");
    assert!(headers.starts_with(&format!("POST {EXEC_PATH} HTTP/1.1\r\n")));
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer secret"))
    );
    assert!(
        headers.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!(
                "Content-Type: {CONNECT_STREAM_CONTENT_TYPE}"
            ))
        })
    );

    let body = &request[header_end + 4..];
    assert_eq!(body.first().copied(), Some(0), "Connect data envelope");
    if expected_fragment == b"printf background-contract" {
        assert!(
            body.windows(2).any(|window| window == [0x82, 0x01]),
            "BackgroundShellSpawn request must use frozen ExecServerMessage field 16"
        );
    }
    assert!(
        body.windows(expected_fragment.len())
            .any(|window| window == expected_fragment),
        "request protobuf did not contain expected payload"
    );

    let mut connect_body = connect_envelope(0, &element);
    connect_body.extend_from_slice(&connect_envelope(0x02, br#"{}"#));

    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {CONNECT_STREAM_CONTENT_TYPE}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
        connect_body.len()
    )
    .expect("write response headers");
    stream.write_all(&connect_body).expect("write Connect body");
    stream.write_all(b"\r\n0\r\n\r\n").expect("finish chunks");
    stream.flush().expect("flush response");
}

fn serve_exec_responses(
    listener: &TcpListener,
    expected_fragment: &[u8],
    elements: Vec<Vec<u8>>,
) {
    let (mut stream, _) = listener.accept().expect("accept ExecService request");
    let request = read_request(&mut stream);
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header end");
    let headers = std::str::from_utf8(&request[..header_end]).expect("headers");
    assert!(headers.starts_with(&format!("POST {EXEC_PATH} HTTP/1.1\r\n")));
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer secret"))
    );
    assert!(
        headers.lines().any(|line| {
            line.eq_ignore_ascii_case(&format!(
                "Content-Type: {CONNECT_STREAM_CONTENT_TYPE}"
            ))
        })
    );

    let body = &request[header_end + 4..];
    assert_eq!(body.first().copied(), Some(0), "Connect data envelope");
    assert!(
        body.windows(expected_fragment.len())
            .any(|window| window == expected_fragment),
        "request protobuf did not contain expected payload"
    );
    assert!(
        body.contains(&0x72),
        "ShellStream request must use frozen ExecServerMessage field 14"
    );
    assert!(
        body.windows(2).any(|window| window == [0x68, 0x02]),
        "ShellStream timeout_behavior must be frozen BACKGROUND=2"
    );

    let mut connect_body = Vec::new();
    for element in elements {
        connect_body.extend_from_slice(&connect_envelope(0, &element));
    }
    connect_body.extend_from_slice(&connect_envelope(0x02, br#"{}"#));

    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {CONNECT_STREAM_CONTENT_TYPE}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
        connect_body.len()
    )
    .expect("write response headers");
    stream.write_all(&connect_body).expect("write Connect body");
    stream.write_all(b"\r\n0\r\n\r\n").expect("finish chunks");
    stream.flush().expect("flush response");
}

#[test]
fn production_exec_service_background_shell_spawn_uses_frozen_field_16_and_real_shell_id() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake background ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_exec_response(
            &listener,
            b"printf background-contract",
            background_shell_spawn_success(
                4242,
                "printf background-contract",
                "/workspace",
                Some(31337),
            ),
        );
        serve_exec_response(
            &listener,
            b"background-error",
            background_shell_spawn_error(2, "background-error", "/workspace", "spawn failed"),
        );
        serve_exec_response(
            &listener,
            b"background-rejected",
            background_shell_spawn_error(3, "background-rejected", "/workspace", "blocked"),
        );
        serve_exec_response(
            &listener,
            b"background-permission",
            background_shell_spawn_error(4, "background-permission", "/workspace", "denied"),
        );
        serve_exec_response(
            &listener,
            b"background-sandbox",
            background_shell_spawn_sandbox_unsupported(
                "background-sandbox",
                "/workspace",
                7,
                "sandbox unsupported",
            ),
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    let mut accessor = environment.remote_resource_accessor();
    let spawn = |command: &str| -> ProductionBackgroundShellSpawnArgs {
        build_host_shell_args(HostShellArgsInput {
            command: command.into(),
            name: command.split_whitespace().next().unwrap_or("shell").into(),
            working_directory: "/workspace".into(),
            tool_call_id: format!("{command}-tool"),
        })
        .into()
    };

    assert_eq!(
        accessor
            .execute_background_shell_spawn(&(), spawn("printf background-contract"))
            .expect("typed field-16 success"),
        ProductionBackgroundShellSpawnResult::Success {
            shell_id: 4242,
            command: "printf background-contract".into(),
            working_directory: "/workspace".into(),
            pid: Some(31337),
        }
    );
    assert_eq!(
        accessor
            .execute_background_shell_spawn(&(), spawn("background-error"))
            .expect("typed field-16 error"),
        ProductionBackgroundShellSpawnResult::Error {
            command: "background-error".into(),
            working_directory: "/workspace".into(),
            error: "spawn failed".into(),
        }
    );
    assert_eq!(
        accessor
            .execute_background_shell_spawn(&(), spawn("background-rejected"))
            .expect("typed field-16 rejected"),
        ProductionBackgroundShellSpawnResult::Rejected {
            command: "background-rejected".into(),
            working_directory: "/workspace".into(),
            reason: "blocked".into(),
            is_readonly: true,
        }
    );
    assert_eq!(
        accessor
            .execute_background_shell_spawn(&(), spawn("background-permission"))
            .expect("typed field-16 permission"),
        ProductionBackgroundShellSpawnResult::PermissionDenied {
            command: "background-permission".into(),
            working_directory: "/workspace".into(),
            error: "denied".into(),
            is_readonly: true,
        }
    );
    assert_eq!(
        accessor
            .execute_background_shell_spawn(&(), spawn("background-sandbox"))
            .expect("typed field-16 sandbox"),
        ProductionBackgroundShellSpawnResult::SandboxUnsupported {
            command: "background-sandbox".into(),
            working_directory: "/workspace".into(),
            sandbox_policy_type: 7,
            reason: "sandbox unsupported".into(),
            is_readonly: true,
        }
    );

    server.join().expect("fake background ExecService thread");
}

#[test]
fn production_exec_service_preserves_shell_stdout_for_internal_consumers() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_exec_response(
            &listener,
            b"printf shipping-full-result",
            shell_success_element_with_output("pending-recording\n", "diagnostic"),
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    let mut accessor = environment.remote_resource_accessor();
    let result = accessor
        .execute_shell_result(
            &(),
            build_host_shell_args(HostShellArgsInput {
                command: "printf shipping-full-result".into(),
                name: "printf".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "shipping-full-result-contract".into(),
            }),
        )
        .expect("shipping full shell result");

    assert_eq!(
        result,
        ProductionShellResult::Success {
            exit_code: 0,
            stdout: "pending-recording\n".into(),
            stderr: "diagnostic".into(),
        }
    );

    server.join().expect("fake ExecService thread");
}

#[test]
fn production_exec_service_streams_shell_and_write_through_shipping_accessor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_exec_response(
            &listener,
            b"echo shipping-exec",
            shell_success_element("ready"),
        );
        serve_exec_response(
            &listener,
            b"/tmp/fabushi-exec-proof",
            write_success_element(),
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    let mut accessor = environment.remote_resource_accessor();

    let shell = accessor
        .execute(
            &(),
            build_host_shell_args(HostShellArgsInput {
                command: "echo shipping-exec".into(),
                name: "echo".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "shipping-exec-contract".into(),
            }),
        )
        .expect("shipping shell ExecService call");
    assert_eq!(
        shell.result,
        ShellExecutionOutcome::Success {
            exit_code: 0,
            stderr: "ready".into(),
        }
    );

    let write = accessor
        .execute_write(
            &(),
            "/tmp/fabushi-exec-proof",
            b"payload",
            "shipping-write-contract",
        )
        .expect("shipping write ExecService call");
    assert_eq!(write, WriteExecResult::Success);

    server.join().expect("fake ExecService thread");
}


fn serve_unary_success(listener: &TcpListener, expected_path: &str) {
    let (mut stream, _) = listener.accept().expect("accept ControlService request");
    let request = read_request(&mut stream);
    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header end");
    let headers = std::str::from_utf8(&request[..header_end]).expect("headers");
    assert!(headers.starts_with(&format!("POST {expected_path} HTTP/1.1\r\n")));
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer secret"))
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("write unary response");
    stream.flush().expect("flush unary response");
}

#[test]
fn production_loopback_factory_gates_exec_accessor_on_authenticated_readiness() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake box daemon");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_unary_success(&listener, PING_PATH);
        serve_exec_response(
            &listener,
            b"echo loopback-production",
            shell_success_element("loopback-ready"),
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    assert_eq!(environment.loopback().describe(), "loopback");
    let mut ready = environment
        .ensure_ready("agent-production")
        .expect("authenticated loopback readiness");
    assert_eq!(
        ready.vnc_url,
        "http://127.0.0.1:6080/vnc.html"
    );
    assert_eq!(
        ready.terminals_folder,
        "/root/.cursor/projects/workspace/terminals"
    );

    let shell = ready
        .remote_accessor
        .execute(
            &(),
            build_host_shell_args(HostShellArgsInput {
                command: "echo loopback-production".into(),
                name: "echo".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "loopback-production-contract".into(),
            }),
        )
        .expect("loopback production shell");
    assert_eq!(
        shell.result,
        ShellExecutionOutcome::Success {
            exit_code: 0,
            stderr: "loopback-ready".into(),
        }
    );

    let computer_error = ready
        .remote_accessor
        .execute_computer_use_protobuf(&(), Vec::new())
        .expect_err("standalone production primary must block monitor computer use");
    assert!(
        matches!(computer_error, ProductionBoxExecError::NoMonitor(_)),
        "standalone production primary must use the Grok no-monitor overlay"
    );

    assert!(should_apply_shared_desktop(environment.loopback().max_windows()));
    assert_eq!(
        format_sand_box_startup_summary(true, true),
        "[sand-host] agent box backend: loopback (in-box); image: host's own container; auto-update: on; build: packaged"
    );

    server.join().expect("fake box daemon thread");
}


#[test]
fn production_exec_service_reads_through_protected_shipping_accessor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake read ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_unary_success(&listener, PING_PATH);
        serve_exec_response(
            &listener,
            b"/workspace/safe.txt",
            read_success_element("/workspace/safe.txt", "safe\ncontent"),
        );
    });

    let loopback = LoopbackSandBox::new(LoopbackSandBoxOptions {
        host: "127.0.0.1".into(),
        auth_token: "secret".into(),
        exec_daemon_port: port,
        ready_timeout_ms: 5_000,
        poll_interval_ms: 0,
        watchdog_interval_ms: 30_000,
        protected_box_paths: vec![PathBuf::from("/workspace/private")],
    });
    let mut ready = loopback
        .ensure_ready(&(), "agent-read")
        .expect("shipping protected accessor readiness");

    let safe = ready
        .remote_accessor
        .execute_read(
            &(),
            ProductionReadArgs {
                path: "/workspace/safe.txt".into(),
                tool_call_id: "shipping-read-contract".into(),
                offset: None,
                limit: None,
                encoding_hint: None,
            },
        )
        .expect("safe read should reach ExecService");
    assert_eq!(
        safe,
        ProductionReadResult::Success {
            path: "/workspace/safe.txt".into(),
            output: ProductionReadOutput::Content("safe\ncontent".into()),
            total_lines: 2,
            file_size: 12,
            truncated: false,
            output_blob_id: None,
            range_applied: false,
        }
    );

    let protected = ready.remote_accessor.execute_read(
        &(),
        ProductionReadArgs {
            path: "/workspace/private/secret.db".into(),
            tool_call_id: "blocked-read-contract".into(),
            offset: None,
            limit: None,
            encoding_hint: None,
        },
    );
    assert!(
        protected.is_err(),
        "protected host paths must be rejected before a remote ExecService call"
    );

    server.join().expect("fake read ExecService thread");
}

#[test]
fn production_exec_service_computer_use_has_remote_and_no_monitor_paths() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake computer ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_exec_response(
            &listener,
            b"computer-contract",
            computer_use_success_element(),
        );
        serve_exec_response(
            &listener,
            b"raw-field-44",
            raw_resource_element(44, &[0x08, 0x01]),
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    let mut accessor = environment.remote_resource_accessor();
    let protobuf_args = {
        let mut args = Vec::new();
        push_len(1, b"computer-contract", &mut args);
        args
    };
    let result = accessor
        .execute_computer_use_protobuf(&(), protobuf_args)
        .expect("shipping computer-use ExecService call");
    assert_eq!(result, vec![0x0a, 0x00]);

    let mut raw_args = Vec::new();
    push_len(1, b"raw-field-44", &mut raw_args);
    let raw_result = accessor
        .execute_raw_resource(&(), 44, raw_args)
        .expect("generic generated ExecService resource binding");
    assert_eq!(raw_result, vec![0x08, 0x01]);

    let mut denied = environment
        .remote_resource_accessor()
        .with_no_monitor_computer_use();
    let error = denied
        .execute_computer_use_protobuf(&(), Vec::new())
        .expect_err("no-monitor overlay must fail locally");
    assert!(
        error.to_string().contains("No private desktop monitor"),
        "no-monitor overlay should preserve the Grok error contract"
    );

    server.join().expect("fake computer ExecService thread");
}


#[test]
fn production_exec_service_shell_stream_uses_frozen_field_14_and_real_backgrounded_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake stream ExecService");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_exec_responses(
            &listener,
            b"printf timed-background",
            vec![
                shell_stream_start_element(),
                shell_stream_stdout_element("before\n"),
                shell_stream_backgrounded_element(
                    5150,
                    "printf timed-background",
                    "/workspace",
                    Some(27182),
                    Some(75),
                    Some(SHELL_BACKGROUND_REASON_TIMEOUT),
                ),
            ],
        );
    });

    let environment = ProductionBoxEnvironment::new("127.0.0.1", port, "secret");
    let mut accessor = environment.remote_resource_accessor();
    let events = accessor
        .execute_shell_stream(
            &(),
            ProductionShellStreamArgs {
                shell_args: build_host_shell_args(HostShellArgsInput {
                    command: "printf timed-background".into(),
                    name: "printf".into(),
                    working_directory: "/workspace".into(),
                    tool_call_id: "timed-background-direct-contract".into(),
                }),
                timeout_ms: 75,
                hard_timeout_ms: None,
            },
        )
        .expect("typed field-14 shell stream");

    assert_eq!(
        events,
        vec![
            ProductionShellStreamEvent::Start,
            ProductionShellStreamEvent::Stdout("before\n".into()),
            ProductionShellStreamEvent::Backgrounded {
                shell_id: 5150,
                command: "printf timed-background".into(),
                working_directory: "/workspace".into(),
                pid: Some(27182),
                ms_to_wait: Some(75),
                reason: Some(SHELL_BACKGROUND_REASON_TIMEOUT),
            },
        ],
        "field 14 must preserve the frozen typed event stream and real shellId"
    );

    server.join().expect("fake stream ExecService thread");
}

struct NoopForeverBoxLifecycle;

impl ForeverBoxLifecycle for NoopForeverBoxLifecycle {
    fn fetch_image_update_available(&self) -> Result<bool, String> {
        Ok(false)
    }

    fn recreate_in_box(
        &self,
        _preserve_data: bool,
        _force: Option<bool>,
    ) -> Result<RecreateSandBoxResponse, String> {
        Ok(RecreateSandBoxResponse {
            started: false,
            reason: Some("not-used-by-runner-resource-contract".into()),
        })
    }
}

#[test]
fn forever_box_runner_resource_port_background_spawn_registers_real_shell_id_with_unique_watcher() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind Runner background box daemon");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_unary_success(&listener, PING_PATH);
        serve_exec_response(
            &listener,
            b"echo live-background",
            background_shell_spawn_success(
                4242,
                "echo live-background",
                "/workspace",
                Some(31337),
            ),
        );
    });

    let service = Arc::new(ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", port, "secret")),
        Arc::new(NoopForeverBoxLifecycle),
        false,
        false,
        false,
    ));
    let pending = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let pending_capture = Arc::clone(&pending);
    let watches = Arc::new(RunnerBackgroundShellWatches::new(
        Arc::new(|_, _, _| None),
        Some(Arc::new(move |watch| {
            pending_capture
                .lock()
                .expect("pending background watch capture")
                .push((
                    watch.parent_agent_id.clone(),
                    watch.work_id.clone(),
                    watch.title.clone(),
                ));
        })),
        None,
        None,
    ));
    let port_adapter = ForeverBoxRunnerResourcePort::new(service, "agent-runner")
        .with_background_shell_watches(Arc::clone(&watches));

    let shell = port_adapter
        .execute_shell(RunnerBoxShellRequest {
            command: "echo live-background".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "runner-background-shell-contract".into(),
            is_background: true,
            block_until_ms: None,
        })
        .expect("Runner background Shell through ForeverBox");
    assert_eq!(shell["kind"], "backgrounded");
    assert_eq!(shell["shellId"], 4242);
    assert_eq!(shell["pid"], 31337);
    assert_eq!(
        pending.lock().expect("pending background watches").as_slice(),
        &[(
            "agent-runner".into(),
            "4242".into(),
            "echo live-background".into(),
        )],
        "the real wire shellId must be registered with the unique Runner watcher"
    );

    server.join().expect("Runner background box daemon thread");
}

#[test]
fn forever_box_runner_resource_port_positive_block_backgrounds_real_shell_id_with_unique_watcher() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind timed Runner box daemon");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        serve_unary_success(&listener, PING_PATH);
        serve_exec_responses(
            &listener,
            b"printf timed-runner",
            vec![
                shell_stream_start_element(),
                shell_stream_stdout_element("before\n"),
                shell_stream_backgrounded_element(
                    5150,
                    "printf timed-runner",
                    "/workspace",
                    Some(27182),
                    Some(75),
                    Some(SHELL_BACKGROUND_REASON_TIMEOUT),
                ),
            ],
        );
    });

    let service = Arc::new(ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", port, "secret")),
        Arc::new(NoopForeverBoxLifecycle),
        false,
        false,
        false,
    ));
    let pending = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let pending_capture = Arc::clone(&pending);
    let watches = Arc::new(RunnerBackgroundShellWatches::new(
        Arc::new(|_, _, _| None),
        Some(Arc::new(move |watch| {
            pending_capture
                .lock()
                .expect("timed pending background watch capture")
                .push((
                    watch.parent_agent_id.clone(),
                    watch.work_id.clone(),
                    watch.title.clone(),
                ));
        })),
        None,
        None,
    ));
    let port_adapter = ForeverBoxRunnerResourcePort::new(service, "agent-timed-runner")
        .with_background_shell_watches(Arc::clone(&watches));

    let shell = port_adapter
        .execute_shell(RunnerBoxShellRequest {
            command: "printf timed-runner".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "runner-timed-shell-contract".into(),
            is_background: false,
            block_until_ms: Some(75),
        })
        .expect("Runner timed Shell through ForeverBox");
    assert_eq!(shell["kind"], "backgrounded");
    assert_eq!(shell["shellId"], 5150);
    assert_eq!(shell["pid"], 27182);
    assert_eq!(shell["blockUntilMs"], 75);
    assert_eq!(shell["backgroundReason"], SHELL_BACKGROUND_REASON_TIMEOUT);
    assert_eq!(shell["stdout"], "before\n");
    assert_eq!(
        pending.lock().expect("timed pending watches").as_slice(),
        &[(
            "agent-timed-runner".into(),
            "5150".into(),
            "printf timed-runner".into(),
        )],
        "positive block_until_ms must register the real field-14 shellId with the unique watcher"
    );

    server.join().expect("timed Runner box daemon thread");
}

#[test]
fn forever_box_runner_resource_port_reuses_authenticated_shipping_exec_service_for_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind Runner box daemon");
    let port = listener.local_addr().expect("local address").port();
    let server = thread::spawn(move || {
        // Frozen remote-box-resources owns one connectionPromise per Runner.
        // The shipping adapter is turn-scoped, so Shell authenticates once and
        // the following Read reuses that guarded accessor without another ping.
        serve_unary_success(&listener, PING_PATH);
        serve_exec_response(
            &listener,
            b"echo runner-box",
            shell_success_element("runner-shell"),
        );
        serve_exec_response(
            &listener,
            b"/workspace/runner.txt",
            read_success_element("/workspace/runner.txt", "runner-read"),
        );
    });

    let service = Arc::new(ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", port, "secret")),
        Arc::new(NoopForeverBoxLifecycle),
        false,
        false,
        false,
    ));
    let port_adapter = ForeverBoxRunnerResourcePort::new(service, "agent-runner");

    let shell = port_adapter
        .execute_shell(RunnerBoxShellRequest {
            command: "echo runner-box".into(),
            working_directory: "/workspace".into(),
            tool_call_id: "runner-shell-contract".into(),
            is_background: false,
            block_until_ms: None,
        })
        .expect("Runner Shell through ForeverBox");
    assert_eq!(shell["kind"], "success");
    assert_eq!(shell["exitCode"], 0);
    assert_eq!(shell["stderr"], "runner-shell");

    let read = port_adapter
        .execute_read(RunnerBoxReadRequest {
            path: "/workspace/runner.txt".into(),
            tool_call_id: "runner-read-contract".into(),
            offset: None,
            limit: None,
            encoding_hint: None,
        })
        .expect("Runner Read through ForeverBox");
    assert_eq!(read["kind"], "success");
    assert_eq!(read["path"], "/workspace/runner.txt");
    assert_eq!(read["output"]["content"], "runner-read");

    server.join().expect("Runner box daemon thread");
}
