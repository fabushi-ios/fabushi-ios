use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::cursor_backend::{
    DASHBOARD_GET_ME_PATH, DASHBOARD_GET_USER_PRIVACY_MODE_PATH,
    clear_sand_privacy_mode_cache_for_testing, fetch_sand_user_full_name,
    get_sand_ghost_mode_header_from_privacy_mode, resolve_sand_ghost_mode_header,
    SandPrivacyMode,
};

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

fn serve(listener: &TcpListener, status: &str, body: &[u8]) -> Vec<u8> {
    let (mut stream, _) = listener.accept().expect("accept backend request");
    let request = read_request(&mut stream);
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

fn push_string(field: u8, value: &str, out: &mut Vec<u8>) {
    out.push((field << 3) | 2);
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

#[test]
fn frozen_privacy_modes_map_to_fail_closed_ghost_header() {
    assert_eq!(
        get_sand_ghost_mode_header_from_privacy_mode(Some(SandPrivacyMode::NoStorage)),
        "true"
    );
    assert_eq!(
        get_sand_ghost_mode_header_from_privacy_mode(Some(SandPrivacyMode::NoTraining)),
        "true"
    );
    assert_eq!(
        get_sand_ghost_mode_header_from_privacy_mode(
            Some(SandPrivacyMode::UsageDataTrainingAllowed)
        ),
        "false"
    );
    assert_eq!(
        get_sand_ghost_mode_header_from_privacy_mode(
            Some(SandPrivacyMode::UsageCodebaseTrainingAllowed)
        ),
        "false"
    );
    assert_eq!(get_sand_ghost_mode_header_from_privacy_mode(None), "true");
}

#[test]
fn dashboard_get_me_uses_cached_privacy_mode_and_production_headers() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let server = thread::spawn(move || {
        let privacy = serve(&listener, "200 OK", &[0x08, 0x03]);
        let mut me = Vec::new();
        push_string(4, "Gloria", &mut me);
        push_string(5, "Chan", &mut me);
        let get_me = serve(&listener, "200 OK", &me);
        (privacy, get_me)
    });

    let backend = format!("http://127.0.0.1:{port}");
    let full_name =
        fetch_sand_user_full_name(&backend, "access-token", "machine-42").expect("GetMe");
    assert_eq!(full_name.as_deref(), Some("Gloria Chan"));

    let (privacy, get_me) = server.join().expect("server");
    let (privacy_headers, privacy_body) = split_request(&privacy);
    assert!(privacy_headers.starts_with(&format!(
        "POST {DASHBOARD_GET_USER_PRIVACY_MODE_PATH} HTTP/1.1\r\n"
    )));
    assert_eq!(privacy_body, &[0x08, 0x01]);
    let privacy_headers = privacy_headers.to_ascii_lowercase();
    assert!(privacy_headers.contains("authorization: bearer access-token\r\n"));
    assert!(privacy_headers.contains("x-cursor-checksum: "));
    assert!(privacy_headers.contains("x-cursor-client-type: sand\r\n"));
    assert!(privacy_headers.contains("x-ghost-mode: true\r\n"));

    let (me_headers, me_body) = split_request(&get_me);
    assert!(me_headers.starts_with(&format!(
        "POST {DASHBOARD_GET_ME_PATH} HTTP/1.1\r\n"
    )));
    assert!(me_body.is_empty());
    assert!(me_headers.to_ascii_lowercase().contains("x-ghost-mode: false\r\n"));
}

#[test]
fn privacy_lookup_failure_falls_back_to_ghost_mode_true() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let server = thread::spawn(move || {
        let privacy = serve(&listener, "503 Service Unavailable", b"nope");
        privacy
    });
    let backend = format!("http://127.0.0.1:{port}");
    assert_eq!(
        resolve_sand_ghost_mode_header(&backend, "access-token", "machine-42"),
        "true"
    );
    let privacy = server.join().expect("server");
    let (headers, _) = split_request(&privacy);
    assert!(headers.starts_with(&format!(
        "POST {DASHBOARD_GET_USER_PRIVACY_MODE_PATH} HTTP/1.1\r\n"
    )));
}
