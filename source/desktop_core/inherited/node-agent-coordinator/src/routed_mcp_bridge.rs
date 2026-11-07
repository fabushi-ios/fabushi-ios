use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::protocol::Failure;

pub const ROUTED_MCP_PROTOCOL_VERSION: &str = "2025-03-26";
pub const ROUTED_MCP_MAX_BODY_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedTool {
    pub name: String,
    pub provider_identifier: String,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedToolCall {
    pub name: String,
    pub provider_identifier: String,
    pub tool_name: String,
    pub args: Value,
    pub tool_call_id: String,
}

pub trait RoutedMcpBackend {
    fn list_tools(&mut self) -> Result<Vec<RoutedTool>, Failure>;
    fn call_tool(&mut self, call: RoutedToolCall) -> Result<Value, Failure>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum RoutedMcpHttpOutcome {
    Accepted,
    Json(Value),
    HttpError(u16),
}

#[derive(Debug)]
pub struct RoutedMcpProtocolBridge {
    secret: String,
    tools: HashMap<String, RoutedTool>,
}

impl Default for RoutedMcpProtocolBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl RoutedMcpProtocolBridge {
    pub fn new() -> Self {
        Self {
            secret: Uuid::new_v4().to_string(),
            tools: HashMap::new(),
        }
    }

    pub fn with_secret(secret: impl Into<String>) -> Result<Self, Failure> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(Failure::new("MCP_BRIDGE_INVALID_SECRET", "bridge secret is empty"));
        }
        Ok(Self {
            secret,
            tools: HashMap::new(),
        })
    }

    pub fn path(&self) -> String {
        format!("/mcp/{}", self.secret)
    }

    pub fn local_url(&self, port: u16) -> Result<String, Failure> {
        if port == 0 {
            return Err(Failure::new("MCP_BRIDGE_INVALID_PORT", "loopback port must be non-zero"));
        }
        Ok(format!("http://127.0.0.1:{port}{}", self.path()))
    }

    pub fn handle_http<B: RoutedMcpBackend>(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
        backend: &mut B,
    ) -> RoutedMcpHttpOutcome {
        if method != "POST" || path != self.path() {
            return RoutedMcpHttpOutcome::HttpError(404);
        }
        if body.len() > ROUTED_MCP_MAX_BODY_BYTES {
            return RoutedMcpHttpOutcome::HttpError(413);
        }
        let message = match serde_json::from_slice::<Value>(body) {
            Ok(value) if value.is_object() => value,
            _ => return RoutedMcpHttpOutcome::HttpError(400),
        };

        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "notifications/initialized" {
            return RoutedMcpHttpOutcome::Accepted;
        }

        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => json!({
                "protocolVersion": ROUTED_MCP_PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "fabushi-plugins", "version": "1" }
            }),
            "tools/list" => match backend.list_tools() {
                Ok(discovered) => {
                    self.tools = discovered
                        .into_iter()
                        .map(|tool| (tool.name.clone(), tool))
                        .collect();
                    let tools = self
                        .tools
                        .values()
                        .map(project_tool)
                        .collect::<Vec<_>>();
                    json!({ "tools": tools })
                }
                Err(error) => mcp_failure(error.message),
            },
            "tools/call" => {
                let params = message.get("params").and_then(Value::as_object);
                let name = params
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let Some(selected) = self.tools.get(name).cloned() else {
                    return RoutedMcpHttpOutcome::Json(json_rpc_reply(
                        id,
                        mcp_failure(format!("Unknown Fabushi plugin tool: {name}")),
                    ));
                };
                let args = params
                    .and_then(|params| params.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let call = RoutedToolCall {
                    name: selected.name.clone(),
                    provider_identifier: selected.provider_identifier.clone(),
                    tool_name: selected.tool_name.clone(),
                    args,
                    tool_call_id: Uuid::new_v4().to_string(),
                };
                match backend.call_tool(call) {
                    Ok(value) => project_mcp_result(value),
                    Err(error) => mcp_failure(error.message),
                }
            }
            _ => json!({}),
        };

        RoutedMcpHttpOutcome::Json(json_rpc_reply(id, result))
    }

    pub fn cached_tool_count(&self) -> usize {
        self.tools.len()
    }
}

fn json_rpc_reply(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn project_tool(tool: &RoutedTool) -> Value {
    let read_only = is_read_only(tool);
    json!({
        "name": tool.name.clone(),
        "description": tool.description.clone().unwrap_or_else(|| {
            format!("{} via {}", tool.tool_name, tool.provider_identifier)
        }),
        "inputSchema": tool
            .input_schema
            .clone()
            .filter(Value::is_object)
            .unwrap_or_else(|| {
                json!({ "type": "object", "additionalProperties": true })
            }),
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": read_only,
            "openWorldHint": !read_only
        }
    })
}

fn is_read_only(tool: &RoutedTool) -> bool {
    let label = format!(
        "{} {} {}",
        tool.name,
        tool.tool_name,
        tool.description.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();
    let words = label
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let read_words = [
        "read", "search", "find", "list", "get", "fetch", "query", "lookup", "inspect",
        "view", "download", "retrieve",
    ];
    let write_words = [
        "send", "create", "update", "delete", "remove", "write", "upload", "post", "reply",
        "archive", "move", "rename", "modify", "cancel", "purchase", "buy",
    ];
    words.iter().any(|word| read_words.contains(word))
        && !words.iter().any(|word| write_words.contains(word))
}

fn mcp_failure(message: impl Into<String>) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": message.into() }]
    })
}

fn project_mcp_result(value: Value) -> Value {
    let Some(root) = value.as_object() else {
        return json!({
            "isError": false,
            "content": [{ "type": "text", "text": value.to_string() }]
        });
    };

    if let Some(result) = root.get("result").and_then(Value::as_object) {
        if result.get("case").and_then(Value::as_str) != Some("success") {
            let detail = result
                .get("value")
                .and_then(Value::as_object)
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| Value::Object(root.clone()).to_string());
            return mcp_failure(detail);
        }
        if let Some(success) = result.get("value").and_then(Value::as_object) {
            return normalize_success(success);
        }
    }

    if root.contains_key("content") || root.contains_key("structuredContent") {
        return normalize_success(root);
    }

    json!({
        "isError": false,
        "content": [{ "type": "text", "text": Value::Object(root.clone()).to_string() }]
    })
}

fn normalize_success(success: &serde_json::Map<String, Value>) -> Value {
    let mut content = Vec::new();
    if let Some(items) = success.get("content").and_then(Value::as_array) {
        for raw in items {
            let Some(item) = raw.as_object() else { continue };
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                content.push(json!({ "type": "text", "text": text }));
                continue;
            }
            let carrier = item.get("content").and_then(Value::as_object);
            let case = carrier.and_then(|value| value.get("case")).and_then(Value::as_str);
            let payload = carrier
                .and_then(|value| value.get("value"))
                .and_then(Value::as_object);
            match case {
                Some("text") => {
                    if let Some(text) = payload
                        .and_then(|value| value.get("text"))
                        .and_then(Value::as_str)
                    {
                        content.push(json!({ "type": "text", "text": text }));
                    }
                }
                Some("image") => {
                    if let (Some(data), Some(mime_type)) = (
                        payload.and_then(|value| value.get("data")).cloned(),
                        payload
                            .and_then(|value| value.get("mimeType"))
                            .and_then(Value::as_str),
                    ) {
                        content.push(json!({
                            "type": "image",
                            "data": data,
                            "mimeType": mime_type
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": Value::Object(success.clone()).to_string() }));
    }

    let mut normalized = serde_json::Map::new();
    normalized.insert(
        "isError".into(),
        Value::Bool(success.get("isError").and_then(Value::as_bool).unwrap_or(false)),
    );
    normalized.insert("content".into(), Value::Array(content));
    if let Some(structured) = success.get("structuredContent") {
        normalized.insert("structuredContent".into(), structured.clone());
    }
    Value::Object(normalized)
}


use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct RoutedMcpServer {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RoutedMcpServer {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RoutedMcpServer {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn start_routed_mcp_server<B>(backend: B) -> Result<RoutedMcpServer, Failure>
where
    B: RoutedMcpBackend + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| {
        Failure::new(
            "MCP_BRIDGE_BIND_FAILED",
            format!("could not bind routed MCP loopback server: {error}"),
        )
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        Failure::new(
            "MCP_BRIDGE_BIND_FAILED",
            format!("could not configure routed MCP loopback server: {error}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|error| Failure::new("MCP_BRIDGE_BIND_FAILED", error.to_string()))?
        .port();
    let mut bridge = RoutedMcpProtocolBridge::new();
    let url = bridge.local_url(port)?;
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        let mut backend = backend;
        while !worker_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = serve_routed_mcp_request(stream, &mut bridge, &mut backend);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    });
    Ok(RoutedMcpServer {
        url,
        stop,
        worker: Some(worker),
    })
}

fn serve_routed_mcp_request<B: RoutedMcpBackend>(
    mut stream: TcpStream,
    bridge: &mut RoutedMcpProtocolBridge,
    backend: &mut B,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    const MAX_HEADER_BYTES: usize = 64 * 1024;
    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if request.len() > MAX_HEADER_BYTES {
            write_routed_mcp_response(&mut stream, RoutedMcpHttpOutcome::HttpError(431))?;
            return Ok(());
        }
    };

    let headers = std::str::from_utf8(&request[..header_end]).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "routed MCP request headers are not UTF-8",
        )
    })?;
    let mut lines = headers.lines();
    let Some(request_line) = lines.next() else {
        write_routed_mcp_response(&mut stream, RoutedMcpHttpOutcome::HttpError(400))?;
        return Ok(());
    };
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("").to_string();
    let path = request_parts.next().unwrap_or("").to_string();
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);

    if content_length > ROUTED_MCP_MAX_BODY_BYTES {
        write_routed_mcp_response(&mut stream, RoutedMcpHttpOutcome::HttpError(413))?;
        return Ok(());
    }

    let total_needed = header_end.saturating_add(content_length);
    while request.len() < total_needed {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.len() > total_needed {
            request.truncate(total_needed);
            break;
        }
    }
    if request.len() < total_needed {
        write_routed_mcp_response(&mut stream, RoutedMcpHttpOutcome::HttpError(400))?;
        return Ok(());
    }

    let outcome = bridge.handle_http(
        &method,
        &path,
        &request[header_end..total_needed],
        backend,
    );
    write_routed_mcp_response(&mut stream, outcome)
}

fn write_routed_mcp_response(
    stream: &mut TcpStream,
    outcome: RoutedMcpHttpOutcome,
) -> std::io::Result<()> {
    let (status, content_type, body) = match outcome {
        RoutedMcpHttpOutcome::Accepted => (202, None, Vec::new()),
        RoutedMcpHttpOutcome::Json(value) => (
            200,
            Some("application/json"),
            serde_json::to_vec(&value)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        ),
        RoutedMcpHttpOutcome::HttpError(status) => (status, None, Vec::new()),
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Response",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    if let Some(content_type) = content_type {
        write!(stream, "Content-Type: {content_type}\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(&body)?;
    stream.flush()
}
