use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::Message;

use mahayana_host_runtime::cursor_backend::clear_sand_privacy_mode_cache_for_testing;
use mahayana_host_runtime::extensions::attachments::generate_image_service::{
    CursorGenerateImageOptions, GenerateImageAuth, GenerateImageBackendError,
    RUN_GENERATE_IMAGE_PATH, create_cursor_generate_image_backend,
};

#[derive(Clone)]
struct TestAuth;

impl GenerateImageAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("generate-access-token".into())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine-generate".into())
    }
}

#[derive(Clone, PartialEq, Message)]
struct ReferenceImage {
    #[prost(string, tag = "1")]
    data: String,
    #[prost(string, tag = "2")]
    mime_type: String,
}

#[derive(Clone, PartialEq, Message)]
struct GenerateRequest {
    #[prost(string, tag = "1")]
    description: String,
    #[prost(message, repeated, tag = "2")]
    reference_images: Vec<ReferenceImage>,
    #[prost(string, tag = "3")]
    model_id: String,
    #[prost(bool, tag = "4")]
    max_mode: bool,
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

fn push_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn push_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
    push_varint((field << 3) | 2, out);
    push_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn push_string(field: u64, value: &str, out: &mut Vec<u8>) {
    push_bytes(field, value.as_bytes(), out);
}

fn push_bool(field: u64, value: bool, out: &mut Vec<u8>) {
    push_varint(field << 3, out);
    push_varint(u64::from(value), out);
}

fn success_response(image_data: &str, mime_type: &str) -> Vec<u8> {
    let mut success = Vec::new();
    push_string(1, image_data, &mut success);
    push_string(2, mime_type, &mut success);
    let mut response = Vec::new();
    push_bytes(1, &success, &mut response);
    response
}

fn error_response(message: &str, model_restricted: bool) -> Vec<u8> {
    let mut error = Vec::new();
    push_string(1, message, &mut error);
    if model_restricted {
        push_bool(2, true, &mut error);
    }
    let mut response = Vec::new();
    push_bytes(2, &error, &mut response);
    response
}

fn header_value(headers: &str, wanted: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(wanted)
            .then(|| value.trim().to_string())
    })
}

#[test]
fn cursor_generate_image_uses_frozen_proto_auth_model_and_request_id_contract() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let encoded = STANDARD.encode(b"generated-image");
    let encoded_for_server = encoded.clone();
    let server = thread::spawn(move || {
        let privacy = serve(&listener, &[0x08, 0x03]);
        let generate = serve(
            &listener,
            &success_response(&encoded_for_server, "image/webp"),
        );
        (privacy, generate)
    });

    let observed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_for_callback = Arc::clone(&observed_ids);
    let backend = create_cursor_generate_image_backend(CursorGenerateImageOptions {
        auth: Arc::new(TestAuth),
        backend_url: format!("http://127.0.0.1:{port}"),
        model_id: "grok-image-test".into(),
        on_request_id: Some(Arc::new(move |request_id| {
            observed_for_callback
                .lock()
                .expect("request ids")
                .push(request_id.to_string());
        })),
    });

    let generated = backend(
        "a lotus",
        &[("cmVmZXJlbmNl".into(), "image/png".into())],
    )
    .expect("generate image");
    assert_eq!(generated.image_data, encoded);
    assert_eq!(generated.mime_type, "image/webp");

    let (privacy, generate) = server.join().expect("server");
    let (privacy_headers, _) = split_request(&privacy);
    assert!(
        privacy_headers
            .to_ascii_lowercase()
            .contains("x-ghost-mode: true\r\n")
    );

    let (headers, body) = split_request(&generate);
    assert!(headers.starts_with(&format!(
        "POST {RUN_GENERATE_IMAGE_PATH} HTTP/1.1\r\n"
    )));
    let lower = headers.to_ascii_lowercase();
    assert!(lower.contains("authorization: bearer generate-access-token\r\n"));
    assert!(lower.contains("x-ghost-mode: false\r\n"));
    assert!(lower.contains("connect-protocol-version: 1\r\n"));
    let header_request_id = header_value(headers, "x-request-id").expect("request id header");
    assert_eq!(
        observed_ids.lock().expect("request ids").as_slice(),
        &[header_request_id]
    );

    let decoded = GenerateRequest::decode(body).expect("generate request protobuf");
    assert_eq!(decoded.description, "a lotus");
    assert_eq!(decoded.model_id, "grok-image-test");
    assert!(decoded.max_mode);
    assert_eq!(decoded.reference_images.len(), 1);
    assert_eq!(decoded.reference_images[0].data, "cmVmZXJlbmNl");
    assert_eq!(decoded.reference_images[0].mime_type, "image/png");
}

#[test]
fn cursor_generate_image_preserves_model_restricted_error_class() {
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let port = listener.local_addr().expect("backend address").port();
    let server = thread::spawn(move || {
        let _ = serve(&listener, &[0x08, 0x01]);
        let _ = serve(&listener, &error_response("model unavailable", true));
    });

    let backend = create_cursor_generate_image_backend(CursorGenerateImageOptions {
        auth: Arc::new(TestAuth),
        backend_url: format!("http://127.0.0.1:{port}"),
        model_id: "restricted-model".into(),
        on_request_id: None,
    });
    assert_eq!(
        backend("test", &[]).unwrap_err(),
        GenerateImageBackendError::ModelRestricted("model unavailable".into())
    );
    server.join().expect("server");
}
