use mahayana_mcp_runtime::{McpTransport, NativeMcpClient};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn read_request(stream: &TcpStream) -> (BTreeMap<String, String>, Value) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut request_line = String::new();
    reader.read_line(&mut request_line).expect("read request line");
    assert!(request_line.starts_with("POST /mcp HTTP/1.1"));

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read header");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        let (name, value) = trimmed.split_once(':').expect("header separator");
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }

    let length = headers
        .get("content-length")
        .expect("content-length")
        .parse::<usize>()
        .expect("numeric content-length");
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).expect("read request body");
    let json = serde_json::from_slice(&body).expect("JSON request");
    (headers, json)
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    extra_headers: &[(&str, &str)],
    body: Option<Value>,
) {
    let encoded = body
        .map(|value| serde_json::to_vec(&value).expect("serialize response"))
        .unwrap_or_default();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        encoded.len()
    )
    .expect("write status");
    if !encoded.is_empty() {
        write!(stream, "Content-Type: application/json\r\n").expect("write content type");
    }
    for (name, value) in extra_headers {
        write!(stream, "{name}: {value}\r\n").expect("write response header");
    }
    write!(stream, "\r\n").expect("finish headers");
    stream.write_all(&encoded).expect("write response body");
    stream.flush().expect("flush response");
}

fn spawn_mcp_server(
    expected_method: &'static str,
    expected_params: Value,
    result: Value,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback MCP server");
    let address = listener.local_addr().expect("loopback address");
    let handle = thread::spawn(move || {
        for step in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept MCP request");
            let (headers, request) = read_request(&stream);
            match step {
                0 => {
                    assert_eq!(request["method"], "initialize");
                    assert_eq!(request["id"], 1);
                    assert!(headers.get("mcp-session-id").is_none());
                    respond(
                        &mut stream,
                        "200 OK",
                        &[("MCP-Session-Id", "session-fixture")],
                        Some(json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {},
                                "serverInfo": {"name": "fixture", "version": "1.0.0"}
                            }
                        })),
                    );
                }
                1 => {
                    assert_eq!(request["method"], "notifications/initialized");
                    assert!(request.get("id").is_none());
                    assert_eq!(
                        headers.get("mcp-session-id").map(String::as_str),
                        Some("session-fixture")
                    );
                    respond(&mut stream, "202 Accepted", &[], None);
                }
                2 => {
                    assert_eq!(request["method"], expected_method);
                    assert_eq!(request["id"], 2);
                    assert_eq!(request["params"], expected_params);
                    assert_eq!(
                        headers.get("mcp-session-id").map(String::as_str),
                        Some("session-fixture")
                    );
                    respond(
                        &mut stream,
                        "200 OK",
                        &[],
                        Some(json!({"jsonrpc":"2.0","id":2,"result":result})),
                    );
                }
                _ => unreachable!(),
            }
        }
    });
    (format!("http://{address}/mcp"), handle)
}

#[test]
fn http_list_tools_performs_initialize_notification_and_session_bound_list() {
    let expected_tool = json!({
        "name": "fixture.read",
        "description": "Read fixture state",
        "inputSchema": {"type":"object","additionalProperties":false}
    });
    let (url, server) = spawn_mcp_server(
        "tools/list",
        json!({}),
        json!({"tools":[expected_tool.clone()]}),
    );
    let client = NativeMcpClient::new(McpTransport::Http {
        url,
        headers: BTreeMap::new(),
    });

    let tools = client.list_tools().expect("list tools through real HTTP transport");
    assert_eq!(tools, vec![expected_tool]);
    server.join().expect("MCP server assertions");
}

#[test]
fn http_call_tool_performs_initialize_notification_and_session_bound_call() {
    let arguments = json!({"value":"hello"});
    let (url, server) = spawn_mcp_server(
        "tools/call",
        json!({"name":"fixture.echo","arguments":arguments.clone()}),
        json!({"content":[{"type":"text","text":"hello"}],"isError":false}),
    );
    let client = NativeMcpClient::new(McpTransport::Http {
        url,
        headers: BTreeMap::new(),
    });

    let result = client
        .call_tool("fixture.echo", arguments)
        .expect("call tool through real HTTP transport");
    assert_eq!(result["isError"], false);
    assert_eq!(result["content"][0]["text"], "hello");
    server.join().expect("MCP server assertions");
}
