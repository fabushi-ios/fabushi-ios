use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};
use url::Url;

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};

use super::routed_provider_runtime::RoutedToolBridge;

pub const NAVIGATION_PROBE_CDP_BASE_PORT: u16 = 9_222;
pub const NAVIGATION_PROBE_MIN_INTERVAL_MS: u64 = 2_000;
pub const IGNORED_URL_PREFIXES: &[&str] = &[
    "about:",
    "chrome://",
    "chrome-extension://",
    "chrome-untrusted://",
    "devtools://",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionAuditRecord {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub occurred_at_ms: u64,
    pub action: Value,
}

pub trait ActionAuditSink: Send + Sync {
    fn record(&self, record: ActionAuditRecord);

    fn resolve_transport(&self, _server_identifier: &str) -> String {
        "unknown".to_string()
    }
}

impl<F> ActionAuditSink for F
where
    F: Fn(ActionAuditRecord) + Send + Sync,
{
    fn record(&self, record: ActionAuditRecord) {
        self(record);
    }
}

pub type TransportResolver = Arc<dyn Fn(&str) -> String + Send + Sync>;

pub struct TransportResolvingActionAuditSink {
    inner: Arc<dyn ActionAuditSink>,
    resolver: TransportResolver,
}

impl ActionAuditSink for TransportResolvingActionAuditSink {
    fn record(&self, record: ActionAuditRecord) {
        self.inner.record(record);
    }

    fn resolve_transport(&self, server_identifier: &str) -> String {
        (self.resolver)(server_identifier)
    }
}

pub fn with_transport_resolving_sink(
    inner: Arc<dyn ActionAuditSink>,
    resolver: TransportResolver,
) -> Arc<dyn ActionAuditSink> {
    Arc::new(TransportResolvingActionAuditSink { inner, resolver })
}

#[derive(Clone)]
pub struct RoutedMcpAuditConfig {
    pub agent_id: String,
    pub turn_id: Option<String>,
    pub sink: Arc<dyn ActionAuditSink>,
    pub resolve_transport: TransportResolver,
}

impl RoutedMcpAuditConfig {
    pub fn new(
        agent_id: impl Into<String>,
        turn_id: Option<String>,
        sink: Arc<dyn ActionAuditSink>,
    ) -> Self {
        let resolver_sink = Arc::clone(&sink);
        Self {
            agent_id: agent_id.into(),
            turn_id,
            sink,
            resolve_transport: Arc::new(move |server_identifier| {
                resolver_sink.resolve_transport(server_identifier)
            }),
        }
    }

    pub fn with_transport_resolver(mut self, resolver: TransportResolver) -> Self {
        self.resolve_transport = resolver;
        self
    }
}

pub struct AuditedRoutedToolBridge {
    inner: Arc<dyn RoutedToolBridge>,
    config: RoutedMcpAuditConfig,
}

impl AuditedRoutedToolBridge {
    pub fn new(inner: Arc<dyn RoutedToolBridge>, config: RoutedMcpAuditConfig) -> Self {
        Self { inner, config }
    }
}

impl RoutedToolBridge for AuditedRoutedToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.inner.list_tools()
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let occurred_at_ms = now_ms();
        let started = Instant::now();
        let result = self.inner.call_tool(tool, args, tool_call_id);
        let status = match &result {
            Ok(value) => mcp_audit_status(value),
            Err(_) => "error",
        };
        let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let transport = (self.config.resolve_transport)(&tool.provider_identifier);
        self.config.sink.record(ActionAuditRecord {
            agent_id: self.config.agent_id.clone(),
            turn_id: self.config.turn_id.clone(),
            occurred_at_ms,
            action: json!({
                "kind": "mcpToolCall",
                "toolCallId": tool_call_id,
                "serverIdentifier": tool.provider_identifier,
                "serverName": tool.provider_identifier,
                "toolName": tool.tool_name,
                "transport": transport,
                "status": status,
                "durationMs": duration_ms,
            }),
        });
        result
    }
}

pub fn mcp_audit_status(result: &Value) -> &'static str {
    if let Some(envelope) = result.get("result").and_then(Value::as_object) {
        return match envelope.get("case").and_then(Value::as_str) {
            Some("success") => {
                if envelope
                    .get("value")
                    .and_then(Value::as_object)
                    .and_then(|value| value.get("isError"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    "error"
                } else {
                    "ok"
                }
            }
            Some("approved") => "ok",
            _ => "error",
        };
    }
    if result.get("isError").and_then(Value::as_bool) == Some(true)
        || result.get("error").is_some()
    {
        "error"
    } else {
        "ok"
    }
}

pub fn navigation_probe_command(display_number: u16) -> String {
    let port = NAVIGATION_PROBE_CDP_BASE_PORT.saturating_add(display_number);
    format!("curl -sf --max-time 2 \"http://127.0.0.1:{port}/json/list\"")
}

pub fn normalize_navigation_url(raw_url: &str) -> Option<String> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty()
        || IGNORED_URL_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
    {
        return None;
    }
    let parsed = Url::parse(trimmed).ok()?;
    let origin = parsed.origin().ascii_serialization();
    if origin == "null" {
        return None;
    }
    Some(format!("{origin}{}", parsed.path()))
}

pub fn parse_navigation_probe_output(stdout: &str) -> Vec<Value> {
    let mut targets = Vec::new();
    let mut depth = 0i64;
    let mut start: Option<usize> = None;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in stdout.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '[' | '{' => {
                if depth == 0 && character == '[' {
                    start = Some(index);
                }
                depth += 1;
            }
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start_index) = start.take() {
                        let end = index + character.len_utf8();
                        if let Ok(Value::Array(entries)) =
                            serde_json::from_str::<Value>(&stdout[start_index..end])
                        {
                            targets.extend(entries.into_iter().filter(Value::is_object));
                        }
                    }
                }
                if depth < 0 {
                    depth = 0;
                    start = None;
                }
            }
            _ => {}
        }
    }
    targets
}

pub fn computer_use_audit_kind(action_case: &str) -> Option<&'static str> {
    match action_case {
        "screenshot" => Some("screenshot"),
        "click" => Some("click"),
        "mouseMove" => Some("mouse_move"),
        "drag" => Some("drag"),
        "type" => Some("type"),
        "key" => Some("key"),
        "scroll" => Some("scroll"),
        "wait" => Some("wait"),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
