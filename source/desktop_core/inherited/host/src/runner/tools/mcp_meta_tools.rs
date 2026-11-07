use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedMcpMetaToolDefinition,
    RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const GET_MCP_TOOLS_TOOL_NAME: &str = "GetMcpTools";
pub const CALL_MCP_TOOL_NAME: &str = "CallMcpTool";
pub const MCP_META_TOOL_PROVIDER: &str = "mahayana-mcp-meta";

pub const EXTRA_SHORT_TOOL_TIMEOUT_MS: u64 = 5 * 60 * 1_000;
pub const SHORT_TOOL_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
pub const MEDIUM_TOOL_TIMEOUT_MS: u64 = 30 * 60 * 1_000;
pub const LONG_TOOL_TIMEOUT_MS: u64 = 60 * 60 * 1_000;
pub const EXTRA_LONG_TOOL_TIMEOUT_MS: u64 = 2 * 60 * 60 * 1_000;

const TIMEOUT_BUFFER_MS: u64 = 60 * 1_000;
const TOOL_CALL_GUARD_HEADROOM_MS: u64 = 60 * 1_000;
const TOOL_CALL_GUARD_BLOCK_GRACE_MS: u64 = 30 * 1_000;

pub const TOOL_CALL_TIMEOUT_TIERS_MS: [u64; 5] = [
    EXTRA_SHORT_TOOL_TIMEOUT_MS,
    SHORT_TOOL_TIMEOUT_MS,
    MEDIUM_TOOL_TIMEOUT_MS,
    LONG_TOOL_TIMEOUT_MS,
    EXTRA_LONG_TOOL_TIMEOUT_MS,
];

pub fn parse_block_until_ms(args: &Value) -> Option<u64> {
    let raw = args.get("block_until_ms")?.as_f64()?;
    if !raw.is_finite() || raw < 0.0 {
        return None;
    }
    if raw >= u64::MAX as f64 {
        Some(u64::MAX)
    } else {
        Some(raw.floor() as u64)
    }
}

pub fn is_subagent_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name.trim().to_ascii_lowercase().as_str(),
        "task" | "mcp_task" | "subagent"
    )
}

pub fn suggested_tool_timeout_ms(tool_name: &str, args: &Value) -> u64 {
    if is_subagent_tool_name(tool_name) {
        return LONG_TOOL_TIMEOUT_MS;
    }
    parse_block_until_ms(args)
        .map(|block| block.saturating_add(TIMEOUT_BUFFER_MS))
        .unwrap_or(SHORT_TOOL_TIMEOUT_MS)
}

pub fn pick_tool_call_timeout_tier_ms(suggested_ms: u64) -> u64 {
    TOOL_CALL_TIMEOUT_TIERS_MS
        .into_iter()
        .find(|tier| *tier >= suggested_ms)
        .unwrap_or(EXTRA_LONG_TOOL_TIMEOUT_MS)
}

pub fn tool_call_execution_guard_ms(
    tool_name: &str,
    args: &Value,
    is_computer_use_subagent: bool,
) -> u64 {
    let effective_name = if is_computer_use_subagent {
        "subagent"
    } else {
        tool_name
    };
    let tier_ms = pick_tool_call_timeout_tier_ms(
        suggested_tool_timeout_ms(effective_name, args),
    );
    let tier_headroom_ms = tier_ms.saturating_sub(TOOL_CALL_GUARD_HEADROOM_MS);
    let Some(block_ms) = parse_block_until_ms(args) else {
        return tier_headroom_ms;
    };
    let requested_ms = tier_headroom_ms.max(
        block_ms.saturating_add(TOOL_CALL_GUARD_BLOCK_GRACE_MS),
    );
    if requested_ms >= tier_ms {
        tier_headroom_ms
    } else {
        requested_ms
    }
}

pub fn build_tool_call_execution_timed_out_message(
    tool_name: &str,
    execution_timeout_ms: u64,
) -> String {
    let shell_hint = if tool_name.eq_ignore_ascii_case("shell") {
        " For long-running commands, re-run with block_until_ms set to a small value (or 0) so the command runs in the background, then poll its output instead of blocking on it."
    } else {
        ""
    };
    if execution_timeout_ms == 0 {
        format!(
            "The {tool_name} tool call could not start because activity setup exceeded the per-call time limit. The execution environment may be slow or overloaded.{shell_hint}"
        )
    } else {
        format!(
            "The {tool_name} tool call timed out after {} seconds and was terminated. The execution environment may be unresponsive, or the operation needs longer than the per-call time limit.{shell_hint}",
            execution_timeout_ms / 1_000
        )
    }
}

pub fn effective_routed_tool_name(tool: &RoutedToolDefinition) -> &str {
    let tool_name = tool.tool_name.trim();
    if tool_name.is_empty() {
        tool.name.as_str()
    } else {
        tool_name
    }
}

pub fn execute_routed_tool_with_timeout<F>(
    tool: &RoutedToolDefinition,
    args: &Value,
    is_computer_use_subagent: bool,
    operation: F,
) -> Result<Value, ProviderSessionError>
where
    F: FnOnce() -> Result<Value, ProviderSessionError> + Send + 'static,
{
    let tool_name = effective_meta_invocation_tool_name(tool, args);
    let execution_timeout_ms =
        tool_call_execution_guard_ms(&tool_name, args, is_computer_use_subagent);
    execute_with_timeout_ms(tool_name, execution_timeout_ms, operation)
}

fn execute_with_timeout_ms<F>(
    tool_name: String,
    execution_timeout_ms: u64,
    operation: F,
) -> Result<Value, ProviderSessionError>
where
    F: FnOnce() -> Result<Value, ProviderSessionError> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name(format!("mahayana-tool-guard-{}", sanitize_thread_label(&tool_name)))
        .spawn(move || {
            let _ = sender.send(operation());
        })
        .map_err(|error| {
            ProviderSessionError::Tool(format!(
                "could not start {tool_name} tool execution guard: {error}"
            ))
        })?;

    match receiver.recv_timeout(Duration::from_millis(execution_timeout_ms)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ProviderSessionError::Tool(
            build_tool_call_execution_timed_out_message(
                &tool_name,
                execution_timeout_ms,
            ),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(ProviderSessionError::Tool(
            format!("{tool_name} tool execution ended without a result"),
        )),
    }
}

fn sanitize_thread_label(value: &str) -> String {
    let label = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(32)
        .collect::<String>();
    if label.is_empty() {
        "tool".to_string()
    } else {
        label
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDescriptor {
    pub server_identifier: String,
    pub server_name: String,
    pub plugin: Option<Value>,
    pub marketplace: Option<Value>,
    pub plugin_db_id: Option<String>,
    pub marketplace_id: Option<String>,
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandMcpMetaToolOptions {
    pub enabled: bool,
    pub mcp_descriptors: Vec<McpDescriptor>,
}

pub fn effective_meta_invocation_tool_name(
    tool: &RoutedToolDefinition,
    args: &Value,
) -> String {
    let routed = effective_routed_tool_name(tool);
    if routed.eq_ignore_ascii_case(CALL_MCP_TOOL_NAME) {
        if let Some(inner) = args
            .get("toolName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return inner.to_string();
        }
    }
    routed.to_string()
}

pub fn create_sand_mcp_meta_tool_options(
    mcp_tools: &[RoutedMcpMetaToolDefinition],
) -> SandMcpMetaToolOptions {
    let mut descriptors = Vec::<McpDescriptor>::new();
    for source in mcp_tools {
        let tool = &source.tool;
        let server_identifier = tool.provider_identifier.clone();
        let index = descriptors
            .iter()
            .position(|descriptor| descriptor.server_identifier == server_identifier);
        let descriptor = match index {
            Some(index) => &mut descriptors[index],
            None => {
                descriptors.push(McpDescriptor {
                    server_identifier: server_identifier.clone(),
                    server_name: server_identifier,
                    plugin: source.plugin.clone(),
                    marketplace: source.marketplace.clone(),
                    plugin_db_id: source.plugin_id.clone(),
                    marketplace_id: source.marketplace_id.clone(),
                    tools: Vec::new(),
                });
                descriptors.last_mut().expect("descriptor was just inserted")
            }
        };
        descriptor.tools.push(McpToolDescriptor {
            tool_name: tool.tool_name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
        });
    }
    for descriptor in &mut descriptors {
        descriptor
            .tools
            .sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
    }
    SandMcpMetaToolOptions {
        enabled: true,
        mcp_descriptors: descriptors,
    }
}

#[derive(Clone)]
pub struct McpMetaToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    source_tools: Arc<Vec<RoutedMcpMetaToolDefinition>>,
    options: Arc<SandMcpMetaToolOptions>,
}

impl McpMetaToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        source_tools: Vec<RoutedMcpMetaToolDefinition>,
    ) -> Self {
        let options = create_sand_mcp_meta_tool_options(&source_tools);
        Self {
            delegate,
            source_tools: Arc::new(source_tools),
            options: Arc::new(options),
        }
    }

    pub fn options(&self) -> &SandMcpMetaToolOptions {
        self.options.as_ref()
    }
}

impl RoutedToolBridge for McpMetaToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(mcp_meta_tool_definitions())
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        Ok(self.source_tools.as_ref().clone())
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        match effective_routed_tool_name(tool) {
            GET_MCP_TOOLS_TOOL_NAME => Ok(render_mcp_tool_discovery(self.options.as_ref(), &args)),
            CALL_MCP_TOOL_NAME => {
                let server = required_meta_argument(&args, "server")?;
                let tool_name = required_meta_argument(&args, "toolName")?;
                let target = self
                    .source_tools
                    .iter()
                    .find(|candidate| {
                        candidate.tool.provider_identifier == server
                            && candidate.tool.tool_name == tool_name
                    })
                    .map(|candidate| candidate.tool.clone())
                    .ok_or_else(|| {
                        ProviderSessionError::Tool(format!(
                            "MCP tool {tool_name} was not found on server {server}"
                        ))
                    })?;
                let call_args = args
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                self.delegate.call_tool(&target, call_args, tool_call_id)
            }
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}

fn required_meta_argument<'a>(
    args: &'a Value,
    key: &str,
) -> Result<&'a str, ProviderSessionError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderSessionError::Tool(format!("{key} is required")))
}

pub(crate) fn mcp_meta_tool_definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: GET_MCP_TOOLS_TOOL_NAME.into(),
            provider_identifier: MCP_META_TOOL_PROVIDER.into(),
            tool_name: GET_MCP_TOOLS_TOOL_NAME.into(),
            description: Some(
                "Discover available MCP servers and tool schemas before invoking them.".into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "server": {"type": "string"},
                    "toolName": {"type": "string"},
                    "pattern": {"type": "string"}
                }
            }),
        },
        RoutedToolDefinition {
            name: CALL_MCP_TOOL_NAME.into(),
            provider_identifier: MCP_META_TOOL_PROVIDER.into(),
            tool_name: CALL_MCP_TOOL_NAME.into(),
            description: Some(
                "Invoke one MCP tool after discovering its schema with GetMcpTools.".into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["server", "toolName"],
                "properties": {
                    "server": {"type": "string"},
                    "toolName": {"type": "string"},
                    "arguments": {"type": "object"}
                }
            }),
        },
    ]
}

fn render_mcp_tool_discovery(options: &SandMcpMetaToolOptions, args: &Value) -> Value {
    let server_filter = args
        .get("server")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tool_filter = args
        .get("toolName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let pattern = args
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    let descriptors = options
        .mcp_descriptors
        .iter()
        .filter_map(|descriptor| {
            if let Some(server) = server_filter {
                if server != descriptor.server_identifier {
                    return None;
                }
            }
            let tools = descriptor
                .tools
                .iter()
                .filter(|tool| {
                    if let Some(name) = tool_filter {
                        if name != tool.tool_name {
                            return false;
                        }
                    }
                    match pattern.as_ref() {
                        None => true,
                        Some(needle) => {
                            descriptor
                                .server_identifier
                                .to_ascii_lowercase()
                                .contains(needle)
                                || descriptor.server_name.to_ascii_lowercase().contains(needle)
                                || tool.tool_name.to_ascii_lowercase().contains(needle)
                                || tool
                                    .description
                                    .as_deref()
                                    .unwrap_or_default()
                                    .to_ascii_lowercase()
                                    .contains(needle)
                        }
                    }
                })
                .cloned()
                .collect::<Vec<_>>();
            if tools.is_empty() && (tool_filter.is_some() || pattern.is_some()) {
                return None;
            }
            Some(json!({
                "serverIdentifier": descriptor.server_identifier,
                "serverName": descriptor.server_name,
                "plugin": descriptor.plugin,
                "marketplace": descriptor.marketplace,
                "pluginDbId": descriptor.plugin_db_id,
                "marketplaceId": descriptor.marketplace_id,
                "tools": tools,
            }))
        })
        .collect::<Vec<_>>();
    json!({"enabled": options.enabled, "mcpDescriptors": descriptors})
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    #[test]
    fn timeout_worker_returns_success_and_times_out_without_blocking_caller() {
        assert_eq!(
            execute_with_timeout_ms("fast".into(), 100, || Ok(json!({"ok":true})))
                .expect("fast tool"),
            json!({"ok":true})
        );

        let error = execute_with_timeout_ms("slow".into(), 5, || {
            thread::sleep(Duration::from_millis(50));
            Ok(json!({"late":true}))
        })
        .expect_err("slow tool should time out");
        assert!(error.to_string().contains("timed out"));
    }
}
