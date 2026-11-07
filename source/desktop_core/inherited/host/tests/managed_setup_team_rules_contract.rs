use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::cursor_backend::clear_sand_privacy_mode_cache_for_testing;
use mahayana_host_runtime::extensions::managed_setup::team_rules::{
    DASHBOARD_GET_TEAM_RULES_PATH, DASHBOARD_GET_TEAMS_PATH, fetch_sand_team_rules,
};
use serde_json::json;

fn push_varint(mut value: u64, output: &mut Vec<u8>) {
    loop {
        if value < 0x80 {
            output.push(value as u8);
            return;
        }
        output.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
}

fn varint_field(field: u64, value: u64, output: &mut Vec<u8>) {
    push_varint(field << 3, output);
    push_varint(value, output);
}

fn bytes_field(field: u64, value: &[u8], output: &mut Vec<u8>) {
    push_varint((field << 3) | 2, output);
    push_varint(value.len() as u64, output);
    output.extend_from_slice(value);
}

fn string_field(field: u64, value: &str, output: &mut Vec<u8>) {
    bytes_field(field, value.as_bytes(), output);
}

fn team(id: u64, direct: bool) -> Vec<u8> {
    let mut output = Vec::new();
    varint_field(2, id, &mut output);
    varint_field(36, u64::from(direct), &mut output);
    output
}

fn rule(
    name: &str,
    content: &str,
    required: bool,
    globs: &[&str],
    agent_type: u64,
) -> Vec<u8> {
    let mut output = Vec::new();
    string_field(2, name, &mut output);
    string_field(3, content, &mut output);
    varint_field(5, u64::from(required), &mut output);
    for glob in globs {
        string_field(6, glob, &mut output);
    }
    varint_field(7, agent_type, &mut output);
    output
}

fn response_with_messages(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut output = Vec::new();
    for message in messages {
        bytes_field(1, message, &mut output);
    }
    output
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
    let (mut stream, _) = listener.accept().expect("accept dashboard request");
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

#[test]
fn production_team_rules_fetches_direct_teams_filters_agent_types_and_maps_globs() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind dashboard");
    let port = listener.local_addr().expect("dashboard address").port();

    let teams_response = response_with_messages(&[team(7, true), team(9, false)]);
    let rules_response = response_with_messages(&[
        rule(
            "security",
            "Never publish credentials.",
            true,
            &["**/*.env", "**/secrets/**"],
            2,
        ),
        rule("cursor-only", "Ignore in Sand.", false, &[], 3),
        rule("global-style", "Keep answers concise.", false, &[], 1),
    ]);
    let server = thread::spawn(move || {
        let privacy = serve(&listener, &[0x08, 0x03]);
        let teams = serve(&listener, &teams_response);
        let rules = serve(&listener, &rules_response);
        (privacy, teams, rules)
    });

    let backend = format!("http://127.0.0.1:{port}");
    let rules = fetch_sand_team_rules(&backend, "access-token", "machine-42")
        .expect("production team rules");
    assert_eq!(
        rules,
        vec![
            json!({
                "fullPath":"security",
                "content":"Never publish credentials.",
                "type":{"kind":"fileGlobbed","globs":["**/*.env","**/secrets/**"]},
                "source":"team",
                "isRequired":true
            }),
            json!({
                "fullPath":"global-style",
                "content":"Keep answers concise.",
                "type":{"kind":"global"},
                "source":"team",
                "isRequired":false
            })
        ]
    );

    let (privacy, teams, team_rules) = server.join().expect("dashboard server");
    let (privacy_headers, privacy_body) = split_request(&privacy);
    assert!(privacy_headers.starts_with(
        "POST /aiserver.v1.DashboardService/GetUserPrivacyMode HTTP/1.1\r\n"
    ));
    assert_eq!(privacy_body, &[0x08, 0x01]);

    let (team_headers, team_body) = split_request(&teams);
    assert!(team_headers.starts_with(&format!("POST {DASHBOARD_GET_TEAMS_PATH} HTTP/1.1\r\n")));
    assert_eq!(team_body, &[0x08, 0x01]);
    let team_headers = team_headers.to_ascii_lowercase();
    assert!(team_headers.contains("authorization: bearer access-token\r\n"));
    assert!(team_headers.contains("x-ghost-mode: false\r\n"));

    let (rule_headers, rule_body) = split_request(&team_rules);
    assert!(rule_headers.starts_with(&format!(
        "POST {DASHBOARD_GET_TEAM_RULES_PATH} HTTP/1.1\r\n"
    )));
    assert_eq!(rule_body, &[0x08, 0x01, 0x10, 0x07]);
    assert!(rule_headers.to_ascii_lowercase().contains("x-ghost-mode: false\r\n"));
}
