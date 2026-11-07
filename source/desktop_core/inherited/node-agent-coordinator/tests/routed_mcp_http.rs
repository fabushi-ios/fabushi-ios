use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use mahayana_node_agent_coordinator::protocol::Failure;
use mahayana_node_agent_coordinator::routed_mcp_bridge::{
    start_routed_mcp_server, RoutedMcpBackend, RoutedTool, RoutedToolCall,
};
use serde_json::{json, Value};

#[derive(Clone)]
struct Backend {
    calls: Arc<Mutex<Vec<RoutedToolCall>>>,
}

impl RoutedMcpBackend for Backend {
    fn list_tools(&mut self) -> Result<Vec<RoutedTool>, Failure> {
        Ok(vec![RoutedTool {
            name: "search_docs".into(),
            provider_identifier: "docs".into(),
            tool_name: "search".into(),
            description: Some("Search documentation".into()),
            input_schema: Some(json!({
                "type": "object",
                "properties": { "q": { "type": "string" } }
            })),
        }])
    }

    fn call_tool(&mut self, call: RoutedToolCall) -> Result<Value, Failure> {
        self.calls.lock().expect("calls mutex").push(call.clone());
        Ok(json!({
            "result": {
                "case": "success",
                "value": {
                    "content": [{
                        "content": {
                            "case": "text",
                            "value": { "text": "found" }
                        }
                    }]
                }
            }
        }))
    }
}

fn post_json(url: &str, payload: Value) -> (u16, Value) {
    let rest = url.strip_prefix("http://").expect("loopback http URL");
    let (authority, path) = rest.split_once('/').expect("MCP URL path");
    let body = serde_json::to_vec(&payload).expect("encode request");
    let mut stream = TcpStream::connect(authority).expect("connect routed MCP");
    write!(
        stream,
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write request headers");
    stream.write_all(&body).expect("write request body");
    stream.flush().expect("flush request");

    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read routed MCP response");
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .expect("HTTP response headers");
    let headers = std::str::from_utf8(&response[..header_end]).expect("response headers utf8");
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("HTTP status");
    let value = if response.len() == header_end {
        Value::Null
    } else {
        serde_json::from_slice(&response[header_end..]).expect("JSON response")
    };
    (status, value)
}

#[test]
fn routed_mcp_server_binds_loopback_and_routes_tools() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut server = start_routed_mcp_server(Backend {
        calls: Arc::clone(&calls),
    })
    .expect("start routed MCP server");

    let (status, initialize) = post_json(
        server.url(),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
    );
    assert_eq!(status, 200);
    assert_eq!(
        initialize["result"]["protocolVersion"],
        "2025-03-26"
    );

    let (status, tools) = post_json(
        server.url(),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    );
    assert_eq!(status, 200);
    assert_eq!(tools["result"]["tools"][0]["name"], "search_docs");

    let (status, result) = post_json(
        server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "search_docs", "arguments": { "q": "Grok" } }
        }),
    );
    assert_eq!(status, 200);
    assert_eq!(result["result"]["content"][0]["text"], "found");

    let recorded = calls.lock().expect("calls mutex");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].provider_identifier, "docs");
    assert_eq!(recorded[0].tool_name, "search");
    assert_eq!(recorded[0].args["q"], "Grok");
    drop(recorded);

    server.close();
}

#[test]
fn routed_mcp_server_accepts_initialized_notification() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut server = start_routed_mcp_server(Backend { calls })
        .expect("start routed MCP server");
    let (status, body) = post_json(
        server.url(),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    );
    assert_eq!(status, 202);
    assert!(body.is_null());
    server.close();
}
