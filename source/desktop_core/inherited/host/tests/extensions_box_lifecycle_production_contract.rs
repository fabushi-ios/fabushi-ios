use std::future::Future;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::Duration;

use mahayana_host_runtime::extensions::box_lifecycle::{
    BoxLifecycleAuth, BoxLifecycleService, ProductionBoxLifecycleClient,
    create_cursor_checksum,
};

struct NoopWake;
impl Wake for NoopWake { fn wake(self: Arc<Self>) {} }

fn block_on_ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("production box lifecycle future unexpectedly pending"),
    }
}

#[derive(Default)]
struct FakeAuth;

impl BoxLifecycleAuth for FakeAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("access-token".into())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine-42".into())
    }
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
    let (mut stream, _) = listener.accept().expect("accept lifecycle request");
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
fn checksum_matches_frozen_obfuscation_shape_and_machine_suffix() {
    let checksum = create_cursor_checksum("machine-42", 1_726_000_000_000);
    assert!(checksum.ends_with("machine-42"));
    assert!(checksum.len() > "machine-42".len());
}

#[test]
fn production_box_lifecycle_uses_auth_headers_and_frozen_grok_rpc_paths() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let server = thread::spawn(move || {
        let privacy_request = serve(&listener, &[0x08, 0x03]);
        let state_request = serve(&listener, &[0x10, 0x01]);
        let reason = b"recreating";
        let mut recreate_response = vec![0x08, 0x01, 0x12, reason.len() as u8];
        recreate_response.extend_from_slice(reason);
        let recreate_request = serve(&listener, &recreate_response);
        (privacy_request, state_request, recreate_request)
    });

    let client = ProductionBoxLifecycleClient::new(
        Arc::new(FakeAuth),
        format!("http://127.0.0.1:{port}"),
    )
    .expect("production client");
    let service = BoxLifecycleService::new(client);

    let signal = ();
    assert!(
        block_on_ready(service.fetch_image_update_available(&signal))
            .expect("image update")
    );
    let recreated = block_on_ready(service.recreate_in_box::<()>(true, Some(true)))
        .expect("recreate");
    assert!(recreated.started);
    assert_eq!(recreated.reason.as_deref(), Some("recreating"));

    let (privacy_request, state_request, recreate_request) =
        server.join().expect("backend server");
    let (privacy_headers, privacy_body) = split_request(&privacy_request);
    assert!(privacy_headers.starts_with(
        "POST /aiserver.v1.DashboardService/GetUserPrivacyMode HTTP/1.1\r\n"
    ));
    assert_eq!(privacy_body, &[0x08, 0x01]);
    assert!(privacy_headers.to_ascii_lowercase().contains("x-ghost-mode: true\r\n"));

    let (state_headers, state_body) = split_request(&state_request);
    assert!(state_headers.starts_with(
        "POST /aiserver.v1.GrokBotService/GetSandBoxRunState HTTP/1.1\r\n"
    ));
    assert!(state_body.is_empty());

    for headers in [state_headers, split_request(&recreate_request).0] {
        let lowercase = headers.to_ascii_lowercase();
        assert!(lowercase.contains("authorization: bearer access-token\r\n"));
        assert!(lowercase.contains("content-type: application/proto\r\n"));
        assert!(lowercase.contains("connect-protocol-version: 1\r\n"));
        assert!(lowercase.contains("x-cursor-client-type: sand\r\n"));
        assert!(lowercase.contains("x-cursor-checksum: "));
        assert!(lowercase.contains("x-ghost-mode: false\r\n"));
    }

    let (recreate_headers, recreate_body) = split_request(&recreate_request);
    assert!(recreate_headers.starts_with(
        "POST /aiserver.v1.GrokBotService/RecreateSandBox HTTP/1.1\r\n"
    ));
    assert_eq!(recreate_body, &[0x08, 0x01, 0x10, 0x01]);
}
