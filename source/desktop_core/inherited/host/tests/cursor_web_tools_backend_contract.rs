use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use prost::Message;

use mahayana_host_runtime::cursor_backend::clear_sand_privacy_mode_cache_for_testing;
static WEB_BACKEND_TEST_LOCK: Mutex<()> = Mutex::new(());

use mahayana_host_runtime::extensions::inference::cursor_web_tools::{
    CursorWebAuth, CursorWebBackend, CursorWebBackendOptions, ProductionCursorWebBackend,
    RUN_WEB_FETCH_PATH, RUN_WEB_SEARCH_PATH, WebFetchResponse, WebSearchRequest,
};

#[derive(Clone)]
struct TestAuth;

impl CursorWebAuth for TestAuth {
    fn get_access_token(&self) -> Result<String, String> {
        Ok("web-access-token".into())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        Ok("machine-web".into())
    }
}

#[derive(Clone, PartialEq, Message)]
struct SearchRequest {
    #[prost(string, tag = "1")]
    search_term: String,
    #[prost(string, optional, tag = "2")]
    explanation: Option<String>,
    #[prost(string, tag = "3")]
    model_id: String,
}

#[derive(Clone, PartialEq, Message)]
struct SearchDocument {
    #[prost(string, tag = "1")]
    url: String,
    #[prost(string, tag = "2")]
    title: String,
    #[prost(string, tag = "3")]
    text: String,
}

#[derive(Clone, PartialEq, Message)]
struct SearchResponse {
    #[prost(string, optional, tag = "1")]
    answer: Option<String>,
    #[prost(message, repeated, tag = "2")]
    documents: Vec<SearchDocument>,
}

#[derive(Clone, PartialEq, Message)]
struct FetchRequest {
    #[prost(string, tag = "1")]
    url: String,
}

#[derive(Clone, PartialEq, Message)]
struct FetchSuccess {
    #[prost(string, tag = "1")]
    content: String,
}

#[derive(Clone, PartialEq, Message)]
struct FetchError {
    #[prost(string, tag = "1")]
    error: String,
    #[prost(bool, tag = "2")]
    is_timeout: bool,
}

#[derive(Clone, PartialEq, Message)]
struct FetchResponse {
    #[prost(oneof = "fetch_response::Result", tags = "1, 2")]
    result: Option<fetch_response::Result>,
}

mod fetch_response {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(super::FetchSuccess),
        #[prost(message, tag = "2")]
        Error(super::FetchError),
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    loop {
        let count = stream.read(&mut buffer).expect("read request");
        assert!(count > 0, "client closed before request completed");
        request.extend_from_slice(&buffer[..count]);
        if expected.is_none() {
            if let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                let length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap_or(0);
                expected = Some(header_end + 4 + length);
            }
        }
        if expected.is_some_and(|value| request.len() >= value) {
            return request;
        }
    }
}

fn serve(listener: &TcpListener, body: &[u8]) -> Vec<u8> {
    let (mut stream, _) = listener.accept().expect("accept");
    let request = read_request(&mut stream);
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/proto\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
    request
}

fn split_request(request: &[u8]) -> (&str, &[u8]) {
    let header_end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (
        std::str::from_utf8(&request[..header_end]).unwrap(),
        &request[header_end + 4..],
    )
}

fn header_value(headers: &str, wanted: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(wanted)
            .then(|| value.trim().to_string())
    })
}

#[test]
fn production_cursor_web_backend_uses_frozen_proto_auth_privacy_and_request_id_contract() {
    let _guard = WEB_BACKEND_TEST_LOCK.lock().unwrap();
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let privacy = serve(&listener, &[0x08, 0x03]);
        let search = serve(
            &listener,
            &SearchResponse {
                answer: Some("answer".into()),
                documents: vec![SearchDocument {
                    url: "https://example.invalid".into(),
                    title: "Example".into(),
                    text: "body".into(),
                }],
            }
            .encode_to_vec(),
        );
        let fetch = serve(
            &listener,
            &FetchResponse {
                result: Some(fetch_response::Result::Success(FetchSuccess {
                    content: "page".into(),
                })),
            }
            .encode_to_vec(),
        );
        (privacy, search, fetch)
    });

    let ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&ids);
    let backend = ProductionCursorWebBackend::new(CursorWebBackendOptions {
        auth: Arc::new(TestAuth),
        backend_url: format!("http://127.0.0.1:{port}"),
        on_request_id: Some(Arc::new(move |id| sink.lock().unwrap().push(id.to_string()))),
    });

    let search_response = backend.run_web_search(WebSearchRequest {
        search_term: "Grok architecture".into(),
        explanation: Some("parity".into()),
        model_id: "grok-4.5".into(),
    }).expect("search");
    assert_eq!(search_response.answer.as_deref(), Some("answer"));
    assert_eq!(search_response.documents[0].title, "Example");

    let fetch_response = backend.run_web_fetch("https://example.invalid/page").expect("fetch");
    assert_eq!(
        fetch_response,
        WebFetchResponse::Success { content: "page".into() }
    );

    let (privacy, search, fetch) = server.join().unwrap();
    let (privacy_headers, _) = split_request(&privacy);
    assert!(privacy_headers.to_ascii_lowercase().contains("x-ghost-mode: true\r\n"));

    let (search_headers, search_body) = split_request(&search);
    assert!(search_headers.starts_with(&format!("POST {RUN_WEB_SEARCH_PATH} HTTP/1.1\r\n")));
    let lower = search_headers.to_ascii_lowercase();
    assert!(lower.contains("authorization: bearer web-access-token\r\n"));
    assert!(lower.contains("connect-protocol-version: 1\r\n"));
    assert!(lower.contains("x-ghost-mode: false\r\n"));
    let search_request = SearchRequest::decode(search_body).expect("search request");
    assert_eq!(search_request.search_term, "Grok architecture");
    assert_eq!(search_request.explanation.as_deref(), Some("parity"));
    assert_eq!(search_request.model_id, "grok-4.5");

    let (fetch_headers, fetch_body) = split_request(&fetch);
    assert!(fetch_headers.starts_with(&format!("POST {RUN_WEB_FETCH_PATH} HTTP/1.1\r\n")));
    let fetch_request = FetchRequest::decode(fetch_body).expect("fetch request");
    assert_eq!(fetch_request.url, "https://example.invalid/page");

    let observed = ids.lock().unwrap().clone();
    assert_eq!(observed.len(), 2);
    assert_eq!(
        observed[0],
        header_value(search_headers, "x-request-id").expect("search request id")
    );
    assert_eq!(
        observed[1],
        header_value(fetch_headers, "x-request-id").expect("fetch request id")
    );
}

#[test]
fn production_cursor_web_backend_maps_error_and_missing_fetch_results() {
    let _guard = WEB_BACKEND_TEST_LOCK.lock().unwrap();
    clear_sand_privacy_mode_cache_for_testing();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let _privacy = serve(&listener, &[0x08, 0x03]);
        let _error = serve(
            &listener,
            &FetchResponse {
                result: Some(fetch_response::Result::Error(FetchError {
                    error: "timeout".into(),
                    is_timeout: true,
                })),
            }
            .encode_to_vec(),
        );
        let _missing = serve(
            &listener,
            &FetchResponse { result: None }.encode_to_vec(),
        );
    });

    let backend = ProductionCursorWebBackend::new(CursorWebBackendOptions {
        auth: Arc::new(TestAuth),
        backend_url: format!("http://127.0.0.1:{port}"),
        on_request_id: None,
    });
    assert_eq!(
        backend.run_web_fetch("https://timeout.invalid").unwrap(),
        WebFetchResponse::Error {
            error: "timeout".into(),
            is_timeout: Some(true),
        }
    );
    assert_eq!(
        backend.run_web_fetch("https://missing.invalid").unwrap(),
        WebFetchResponse::MissingResult
    );
    server.join().unwrap();
}
