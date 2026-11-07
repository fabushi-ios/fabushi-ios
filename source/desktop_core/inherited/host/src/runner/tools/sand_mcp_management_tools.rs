use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const DEFAULT_MCP_ACCOUNT_KEY: &str = "default";
pub const PLUGIN_QUERY_MIN_TOKEN_LENGTH: usize = 3;
pub const MAX_RENDERED_MCP_ACCOUNT_LABEL_LENGTH: usize = 64;
pub const MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH: usize = 500;
pub const CARD_SHOWN_NOTE: &str = "Its connect card is now in the chat. Finish unrelated work, then end your turn - you're resumed automatically when the user authorizes. Don't send a link, another card, or reach the service another way meanwhile.";
pub const MCP_AWAITING_SELECTION_MESSAGE: &str = "You just sent a question widget, so this turn is waiting on the user's selection - their answer arrives as the next message. Don't install, uninstall, restart, or authenticate an MCP server in the same turn as the confirmation widget; wait for the user to confirm, then do it on your next turn.";

pub const SEARCH_PLUGINS_TOOL_NAME: &str = "SearchPlugins";
pub const GET_PLUGIN_TOOL_NAME: &str = "GetPlugin";
pub const INSTALL_PLUGIN_TOOL_NAME: &str = "InstallPlugin";
pub const ADD_MCP_SERVER_TOOL_NAME: &str = "AddMcpServer";
pub const UNINSTALL_MCP_SERVER_TOOL_NAME: &str = "UninstallMcpServer";
pub const UNINSTALL_PLUGIN_TOOL_NAME: &str = "UninstallPlugin";
pub const GET_MCP_SERVER_STATUS_TOOL_NAME: &str = "GetMcpServerStatus";
pub const SET_MCP_INSTRUCTIONS_TOOL_NAME: &str = "SetMcpInstructions";
pub const RESTART_MCP_SERVERS_TOOL_NAME: &str = "RestartMcpServers";
pub const AUTHENTICATE_MCP_SERVER_TOOL_NAME: &str = "AuthenticateMcpServer";
pub const REMOVE_MCP_ACCOUNT_TOOL_NAME: &str = "RemoveMcpAccount";
pub const RENAME_MCP_ACCOUNT_TOOL_NAME: &str = "RenameMcpAccount";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInstalledServer {
    pub id: String,
    pub server_identifier: String,
    pub name: String,
    pub status: String,
    pub account_key: String,
    pub transport: String,
    pub tool_count: usize,
    pub disabled_tool_count: Option<usize>,
    pub plugin_id: Option<String>,
    pub status_detail: Option<String>,
    pub custom_instructions: String,
    pub is_team_server: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPluginSkill {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPluginSummary {
    pub plugin_id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub category: String,
    pub is_installed: bool,
    pub install_mode: Option<String>,
    pub connector_count: usize,
    pub skills: Vec<McpPluginSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPluginField {
    pub key: String,
    pub label: String,
    pub is_required: bool,
    pub is_secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPluginDetail {
    #[serde(flatten)]
    pub summary: McpPluginSummary,
    pub fields: Vec<McpPluginField>,
    pub servers: Vec<McpInstalledServer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAuthenticationResult {
    Started { server_name: String },
    AlreadyAuthenticated { server_name: String },
    NotConfigured { server_name: String },
    NotSupported { server_name: String, message: String },
    Unreachable { server_name: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorCard {
    pub connector: String,
    pub server_id: String,
    pub variant: ConnectorCardVariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorCardVariant {
    Connect,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemoveServerResult {
    pub removed: bool,
    pub reason: Option<String>,
    pub servers: Vec<McpInstalledServer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpUninstallPluginResult {
    pub removed: bool,
    pub reason: Option<String>,
}

pub trait McpManagementSink: Send + Sync {
    fn list_plugins(&self) -> Result<Vec<McpPluginSummary>, ProviderSessionError>;
    fn get_plugin(&self, plugin_id: &str) -> Result<Option<McpPluginDetail>, ProviderSessionError>;
    fn install_plugin(
        &self,
        plugin_id: &str,
        values: Option<&BTreeMap<String, String>>,
    ) -> Result<(), ProviderSessionError>;
    fn add_server(
        &self,
        name: &str,
        config_json: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;
    fn list_installed(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;
    fn remove_server(&self, server_id: &str) -> Result<McpRemoveServerResult, ProviderSessionError>;
    fn uninstall_plugin(&self, plugin_id: &str) -> Result<McpUninstallPluginResult, ProviderSessionError>;
    fn set_instructions(
        &self,
        server_id: &str,
        instructions: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;
    fn restart(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;
    fn authenticate(
        &self,
        server_id: &str,
        account_key: &str,
        requesting_agent_id: Option<&str>,
        force_reauth: bool,
    ) -> Result<McpAuthenticationResult, ProviderSessionError>;
    fn remove_account(
        &self,
        server_id: &str,
        account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;
    fn rename_account(
        &self,
        server_id: &str,
        account_key: &str,
        new_account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError>;

    fn requesting_agent_id(&self) -> Option<String> {
        None
    }

    fn is_awaiting_user_selection(&self) -> bool {
        false
    }

    fn is_multi_account_enabled(&self) -> bool {
        false
    }

    fn emit_connector_card(&self, _card: ConnectorCard) -> Result<(), ProviderSessionError> {
        Ok(())
    }
}

pub struct McpManagementToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    management: Arc<dyn McpManagementSink>,
}

impl McpManagementToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        management: Arc<dyn McpManagementSink>,
    ) -> Self {
        Self { delegate, management }
    }
}

impl RoutedToolBridge for McpManagementToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        let names = management_tool_names(self.management.is_multi_account_enabled());
        tools.retain(|tool| {
            !names.contains(tool.name.as_str()) && !names.contains(tool.tool_name.as_str())
        });
        let mut management = management_tool_definitions(
            self.management.is_multi_account_enabled(),
        );
        management.extend(tools);
        Ok(management)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let name = if is_management_tool_name(
            &tool.name,
            self.management.is_multi_account_enabled(),
        ) {
            tool.name.as_str()
        } else if is_management_tool_name(
            &tool.tool_name,
            self.management.is_multi_account_enabled(),
        ) {
            tool.tool_name.as_str()
        } else {
            return self.delegate.call_tool(tool, args, tool_call_id);
        };
        let text = execute_management_tool(name, &args, self.management.as_ref())?;
        Ok(Value::String(text))
    }
}

pub fn tokenize_plugin_query(query: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut tokens = Vec::new();
    for token in query
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() >= PLUGIN_QUERY_MIN_TOKEN_LENGTH)
    {
        if seen.insert(token.to_string()) {
            tokens.push(token.to_string());
        }
    }
    tokens
}

pub fn score_plugin_for_token(plugin: &McpPluginSummary, token: &str) -> usize {
    let name = plugin.name.to_ascii_lowercase();
    let display_name = plugin.display_name.to_ascii_lowercase();
    if name == token || display_name == token {
        return 8;
    }
    if name.contains(token) || display_name.contains(token) {
        return 5;
    }
    if plugin
        .skills
        .iter()
        .any(|skill| skill.name.to_ascii_lowercase().contains(token))
    {
        return 3;
    }
    if plugin.category.to_ascii_lowercase().contains(token) {
        return 2;
    }
    if plugin.description.to_ascii_lowercase().contains(token) {
        return 1;
    }
    0
}

pub fn rank_plugins_lexically(
    plugins: &[McpPluginSummary],
    query: &str,
) -> Vec<McpPluginSummary> {
    let tokens = tokenize_plugin_query(query);
    let mut rows = plugins.to_vec();
    if tokens.is_empty() {
        rows.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        return rows;
    }
    let mut scored = rows
        .into_iter()
        .filter_map(|plugin| {
            let score = tokens
                .iter()
                .map(|token| score_plugin_for_token(&plugin, token))
                .sum::<usize>();
            (score > 0).then_some((score, plugin))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    scored.into_iter().map(|(_, plugin)| plugin).collect()
}

pub fn validate_remote_mcp_url(raw_url: &str) -> Option<String> {
    let parsed = match Url::parse(raw_url) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Some(format!(
                "\"{raw_url}\" is not a valid URL. Ask the user for the server's full https endpoint (e.g. https://example.com/mcp) and try again."
            ))
        }
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return Some(format!(
            "The server URL must be http(s); \"{}:\" is not supported. Fabushi only connects remote http/sse MCP servers over HTTP(S), so ask the user for an https endpoint.",
            parsed.scheme()
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Some(
            "Don't put credentials in the server URL - pass them as headers instead (e.g. { \"Authorization\": \"Bearer <token>\" }), so they aren't stored in plaintext in the URL. Ask the user for the token and try again with a clean URL."
                .into(),
        );
    }
    None
}

pub fn build_server_config_json(
    url: Option<&str>,
    headers: Option<&BTreeMap<String, String>>,
) -> Option<String> {
    let url = url.map(str::trim).filter(|value| !value.is_empty())?;
    let mut object = Map::new();
    object.insert("type".into(), Value::String("http".into()));
    object.insert("url".into(), Value::String(url.to_string()));
    if let Some(headers) = headers.filter(|headers| !headers.is_empty()) {
        object.insert(
            "headers".into(),
            serde_json::to_value(headers).ok()?,
        );
    }
    serde_json::to_string(&Value::Object(object)).ok()
}

pub fn truncate_one_line(value: &str, max: usize) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    if max == 0 {
        return String::new();
    }
    let mut truncated = one_line.chars().take(max.saturating_sub(1)).collect::<String>();
    truncated.push('…');
    truncated
}

pub fn encode_mcp_account_label_for_listing(label: &str) -> String {
    let mut escaped = String::new();
    for character in label.chars() {
        let hostile = matches!(
            character,
            '\u{0000}'..='\u{001f}'
                | '\u{007f}'
                | '"'
                | '\''
                | '`'
                | '\\'
                | '['
                | ']'
                | '{'
                | '}'
                | '('
                | ')'
                | '<'
                | '>'
                | '\u{2028}'
                | '\u{2029}'
        );
        if hostile {
            escaped.push_str(&format!("\\u{:04x}", character as u32));
        } else {
            escaped.push(character);
        }
    }
    format!("\"{escaped}\"")
}

pub fn decode_mcp_account_label_argument(raw_argument: &str) -> String {
    let value = raw_argument.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        if let Ok(Value::String(parsed)) = serde_json::from_str::<Value>(value) {
            return parsed;
        }
    }
    raw_argument.to_string()
}

pub fn format_mcp_account_label_for_prompt(raw_label: &str) -> String {
    raw_label
        .chars()
        .filter(|ch| !matches!(ch, '"' | '\'' | '`' | '\\' | '[' | ']' | '{' | '}' | '(' | ')' | '<' | '>'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_RENDERED_MCP_ACCOUNT_LABEL_LENGTH)
        .collect()
}

pub fn is_mcp_server_id(raw_id: &str) -> bool {
    let value = raw_id.trim();
    !value.is_empty()
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn clamp_mcp_custom_instruction(raw: &str) -> String {
    raw.chars().take(MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH).collect()
}

pub fn get_default_mcp_custom_instruction(server_name: &str) -> &'static str {
    if server_name.trim().eq_ignore_ascii_case("hex") {
        "When using Hex, get the underlying numbers as data: download/export the results as CSV or use the data the connector returns, and analyze those raw values directly. Don't read rendered charts or graphs from screenshots (computer-use chart reading is unreliable) - work from the actual data."
    } else {
        ""
    }
}

pub fn describe_installed(server: &McpInstalledServer) -> String {
    let mut parts = vec![
        format!(
            "- {}: {} [{}]",
            server.server_identifier, server.name, server.status
        ),
        format!(
            "account={}",
            encode_mcp_account_label_for_listing(&server.account_key)
        ),
        format!("transport={}", server.transport),
        match server.disabled_tool_count {
            Some(disabled) if disabled > 0 => format!(
                "tools={}/{} enabled",
                server.tool_count,
                server.tool_count + disabled
            ),
            _ => format!("tools={}", server.tool_count),
        },
    ];
    if let Some(plugin_id) = server.plugin_id.as_deref() {
        parts.push(format!(
            "plugin={plugin_id} (remove via UninstallPlugin - removes the whole plugin)"
        ));
    }
    if let Some(detail) = server.status_detail.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("detail=\"{}\"", truncate_one_line(detail, 200)));
    }
    if !server.custom_instructions.is_empty()
        && server.custom_instructions != get_default_mcp_custom_instruction(&server.name)
    {
        parts.push(format!(
            "instructions=\"{}\"",
            truncate_one_line(&server.custom_instructions, 120)
        ));
    }
    parts.join(" · ")
}

pub fn describe_installed_list(servers: &[McpInstalledServer]) -> String {
    if servers.is_empty() {
        return "No MCP servers are installed.".into();
    }
    let mut lines = vec![format!("{} installed MCP server(s):", servers.len())];
    lines.extend(servers.iter().map(describe_installed));
    lines.join("\n")
}

fn describe_plugin_install_state(plugin: &McpPluginSummary) -> String {
    if !plugin.is_installed {
        return "installed=no".into();
    }
    match plugin.install_mode.as_deref() {
        Some(mode) => format!("installed=yes ({mode})"),
        None => "installed=yes".into(),
    }
}

fn describe_plugin_includes(plugin: &McpPluginSummary) -> String {
    let mut parts = Vec::new();
    if plugin.connector_count > 0 {
        parts.push(format!(
            "{} connector{}",
            plugin.connector_count,
            if plugin.connector_count == 1 { "" } else { "s" }
        ));
    }
    if !plugin.skills.is_empty() {
        parts.push(format!(
            "{} skill{}",
            plugin.skills.len(),
            if plugin.skills.len() == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        "no primitives".into()
    } else {
        parts.join(", ")
    }
}

pub fn describe_plugin_summary(plugin: &McpPluginSummary) -> String {
    let mut lines = vec![
        format!(
            "- {}: {} - {}",
            plugin.plugin_id, plugin.display_name, plugin.description
        ),
        format!(
            "  ({}; includes: {}; category={})",
            describe_plugin_install_state(plugin),
            describe_plugin_includes(plugin),
            plugin.category
        ),
    ];
    let guidance = get_default_mcp_custom_instruction(&plugin.display_name);
    if !guidance.is_empty() {
        lines.push(format!("  usage guidance: {guidance}"));
    }
    lines.join("\n")
}

pub fn describe_plugin_detail(detail: &McpPluginDetail) -> String {
    let plugin = &detail.summary;
    let mut sections = vec![
        format!(
            "{}: {} - {}",
            plugin.plugin_id, plugin.display_name, plugin.description
        ),
        format!(
            "{} · includes: {} · category={}",
            describe_plugin_install_state(plugin),
            describe_plugin_includes(plugin),
            plugin.category
        ),
    ];
    if !plugin.skills.is_empty() {
        let mut lines = vec!["Skills:".to_string()];
        lines.extend(plugin.skills.iter().map(|skill| {
            if skill.description.is_empty() {
                format!("  - {}", skill.name)
            } else {
                format!(
                    "  - {} - {}",
                    skill.name,
                    truncate_one_line(&skill.description, 140)
                )
            }
        }));
        sections.push(lines.join("\n"));
    }
    if !detail.fields.is_empty() {
        let mut lines = vec!["Setup fields (pass in InstallPlugin values):".to_string()];
        lines.extend(detail.fields.iter().map(|field| {
            let mut flags = vec![if field.is_required { "required" } else { "optional" }];
            if field.is_secret {
                flags.push("secret - ask the user, never guess");
            }
            format!(
                "  - {} ({}; {})",
                field.key,
                field.label,
                flags.join(", ")
            )
        }));
        sections.push(lines.join("\n"));
    }
    if !detail.servers.is_empty() {
        let mut lines = vec![
            "Its installed MCP server(s) - statuses live in GetMcpServerStatus:".to_string(),
        ];
        lines.extend(detail.servers.iter().map(describe_installed));
        sections.push(lines.join("\n"));
    }
    if plugin.is_installed && plugin.install_mode.as_deref() == Some("team-required") {
        sections.push("Required by the user's team - it cannot be uninstalled.".into());
    }
    sections.join("\n")
}

pub fn new_needs_auth_rows(
    before: &[McpInstalledServer],
    after: &[McpInstalledServer],
) -> Vec<(String, String)> {
    let prior = before
        .iter()
        .map(|server| server.server_identifier.clone())
        .collect::<BTreeSet<_>>();
    let mut rows = BTreeMap::new();
    for server in after {
        if !prior.contains(&server.server_identifier) && server.status == "needsAuth" {
            rows.insert(server.id.clone(), server.name.clone());
        }
    }
    rows.into_iter().collect()
}

pub fn emit_and_describe_auth_result(
    result: McpAuthenticationResult,
    is_force_reauth: bool,
    server_id: &str,
    management: &dyn McpManagementSink,
) -> Result<String, ProviderSessionError> {
    match result {
        McpAuthenticationResult::Started { server_name } => {
            management.emit_connector_card(ConnectorCard {
                connector: server_name.clone(),
                server_id: server_id.to_string(),
                variant: ConnectorCardVariant::Connect,
            })?;
            Ok(if is_force_reauth {
                format!(
                    "Signed \"{server_name}\" out and started a fresh sign-in. {CARD_SHOWN_NOTE}"
                )
            } else {
                format!(
                    "Authentication started for \"{server_name}\". {CARD_SHOWN_NOTE}"
                )
            })
        }
        McpAuthenticationResult::AlreadyAuthenticated { server_name } => {
            management.emit_connector_card(ConnectorCard {
                connector: server_name.clone(),
                server_id: server_id.to_string(),
                variant: ConnectorCardVariant::Connected,
            })?;
            Ok(format!(
                "\"{server_name}\" is already authenticated and connected; a confirmation card is now in the chat."
            ))
        }
        McpAuthenticationResult::NotConfigured { server_name } => Ok(format!(
            "\"{server_name}\" is not installed, so there is nothing to authenticate. Install it first."
        )),
        McpAuthenticationResult::NotSupported { server_name, message } => Ok(format!(
            "\"{server_name}\" does not support interactive authentication: {message}"
        )),
        McpAuthenticationResult::Unreachable { server_name, message } => Ok(format!(
            "Sign-in for \"{server_name}\" never started. The server or its configuration failed the check: \"{message}\" - not a missing credential, so the user authenticating in Settings would hit the same error. Tell them what it reported instead of sending them to Settings."
        )),
    }
}

pub fn execute_management_tool(
    name: &str,
    args: &Value,
    management: &dyn McpManagementSink,
) -> Result<String, ProviderSessionError> {
    let object = args
        .as_object()
        .ok_or_else(|| tool_error(format!("{name} arguments must be an object")))?;

    if is_mutating_tool(name) && management.is_awaiting_user_selection() {
        return Ok(MCP_AWAITING_SELECTION_MESSAGE.into());
    }

    match name {
        SEARCH_PLUGINS_TOOL_NAME => {
            let query = optional_string(object, "query")?.unwrap_or_default();
            let plugins = rank_plugins_lexically(&management.list_plugins()?, query);
            if plugins.is_empty() {
                return Ok(if query.is_empty() {
                    "The plugin catalog is empty or unavailable right now.".into()
                } else {
                    format!("No plugins match \"{query}\".")
                });
            }
            let mut lines = vec![if query.is_empty() {
                format!("{} plugin(s) available:", plugins.len())
            } else {
                format!(
                    "{} plugin(s) matching \"{query}\" (best first):",
                    plugins.len()
                )
            }];
            lines.extend(plugins.iter().map(describe_plugin_summary));
            Ok(lines.join("\n"))
        }
        GET_PLUGIN_TOOL_NAME => {
            let plugin_id = required_string(object, "plugin_id")?;
            Ok(match management.get_plugin(plugin_id)? {
                Some(detail) => describe_plugin_detail(&detail),
                None => format!("No plugin with id \"{plugin_id}\"."),
            })
        }
        INSTALL_PLUGIN_TOOL_NAME => {
            let plugin_id = required_string(object, "plugin_id")?;
            let before = match management.get_plugin(plugin_id)? {
                Some(detail) => detail,
                None => return Ok(format!("No plugin with id \"{plugin_id}\".")),
            };
            let values = optional_string_map(object, "values")?;
            management.install_plugin(plugin_id, values.as_ref())?;
            let after = match management.get_plugin(plugin_id)? {
                Some(detail) if detail.summary.is_installed => detail,
                _ => return Ok(format!(
                    "The install request for \"{}\" completed, but the plugin does not read as installed yet.",
                    before.summary.display_name
                )),
            };
            let note = emit_needs_auth_cards(&before.servers, &after.servers, management)?;
            let mut sections = vec![
                format!(
                    "Installed {} (plugin {}).",
                    after.summary.display_name, after.summary.plugin_id
                ),
            ];
            if let Some(note) = note {
                sections.push(note);
            }
            sections.push(describe_plugin_detail(&after));
            Ok(sections.join("\n"))
        }
        ADD_MCP_SERVER_TOOL_NAME => {
            let name = required_string(object, "name")?;
            let url = required_string(object, "url")?;
            if let Some(error) = validate_remote_mcp_url(url) {
                return Ok(error);
            }
            let headers = optional_string_map(object, "headers")?;
            let config = build_server_config_json(Some(url), headers.as_ref())
                .ok_or_else(|| tool_error("A remote MCP URL is required."))?;
            let before = management.list_installed()?;
            let servers = management.add_server(name, &config)?;
            let note = emit_needs_auth_cards(&before, &servers, management)?;
            let mut sections = vec![format!("Added \"{name}\".")];
            if let Some(note) = note {
                sections.push(note);
            }
            sections.push(describe_installed_list(&servers));
            Ok(sections.join("\n"))
        }
        UNINSTALL_MCP_SERVER_TOOL_NAME => {
            let token = required_string(object, "server_id")?;
            let installed = management.list_installed()?;
            let Some(row) = resolve_server_row(&installed, token) else {
                return Ok(no_installed_server_message(token));
            };
            if let Some(plugin_id) = row.plugin_id.as_deref() {
                return Ok(format!(
                    "{} was installed from marketplace plugin {plugin_id}; use UninstallPlugin.",
                    row.name
                ));
            }
            if row.is_team_server == Some(true) {
                return Ok(format!(
                    "{} is provided by the user's team, so it can't be removed here.",
                    row.name
                ));
            }
            let result = management.remove_server(&row.id)?;
            let status = if result.removed {
                format!(
                    "Removed MCP server {} ({}).",
                    row.name, row.server_identifier
                )
            } else {
                format!(
                    "The removal request for {} completed, but it still reads as installed.",
                    row.name
                )
            };
            Ok(format!("{status}\n{}", describe_installed_list(&result.servers)))
        }
        UNINSTALL_PLUGIN_TOOL_NAME => {
            let plugin_id = required_string(object, "plugin_id")?;
            let Some(detail) = management.get_plugin(plugin_id)? else {
                return Ok(format!("No plugin with id \"{plugin_id}\"."));
            };
            if !detail.summary.is_installed {
                return Ok(format!(
                    "{} is not installed - nothing to uninstall.",
                    detail.summary.display_name
                ));
            }
            if detail.summary.install_mode.as_deref() == Some("team-required") {
                return Ok(format!(
                    "{} is required by the user's team and cannot be uninstalled.",
                    detail.summary.display_name
                ));
            }
            let result = management.uninstall_plugin(plugin_id)?;
            Ok(if result.removed {
                format!(
                    "Uninstalled {} (plugin {}).",
                    detail.summary.display_name, detail.summary.plugin_id
                )
            } else {
                format!(
                    "The uninstall request for {} completed, but it still reads as installed.",
                    detail.summary.display_name
                )
            })
        }
        GET_MCP_SERVER_STATUS_TOOL_NAME => {
            let installed = management.list_installed()?;
            match optional_string(object, "server_id")? {
                None => Ok(describe_installed_list(&installed)),
                Some(token) => {
                    let rows = resolve_server_rows(&installed, token);
                    if rows.is_empty() {
                        Ok(format!("No installed MCP server \"{token}\"."))
                    } else {
                        Ok(rows
                            .into_iter()
                            .map(describe_installed)
                            .collect::<Vec<_>>()
                            .join("\n"))
                    }
                }
            }
        }
        SET_MCP_INSTRUCTIONS_TOOL_NAME => {
            let token = required_string(object, "server_id")?;
            let instructions = present_string(object, "instructions")?;
            let Some(server_id) = resolve_server_id(management, token)? else {
                return Ok(no_installed_server_message(token));
            };
            let servers = management.set_instructions(
                &server_id,
                &clamp_mcp_custom_instruction(instructions),
            )?;
            Ok(format!(
                "{} custom instructions for MCP server {token}.\n{}",
                if instructions.trim().is_empty() { "Cleared" } else { "Updated" },
                describe_installed_list(&servers)
            ))
        }
        RESTART_MCP_SERVERS_TOOL_NAME => Ok(format!(
            "Restarted MCP servers.\n{}",
            describe_installed_list(&management.restart()?)
        )),
        AUTHENTICATE_MCP_SERVER_TOOL_NAME => {
            let token = required_string(object, "server_id")?;
            let Some(server_id) = resolve_server_id(management, token)? else {
                return Ok(no_installed_server_message(token));
            };
            let account = if management.is_multi_account_enabled() {
                decode_mcp_account_label_argument(
                    optional_string(object, "account_label")?.unwrap_or(DEFAULT_MCP_ACCOUNT_KEY),
                )
            } else {
                DEFAULT_MCP_ACCOUNT_KEY.to_string()
            };
            let force_reauth = optional_bool(object, "force_reauth")?.unwrap_or(false);
            let requesting_agent_id = management.requesting_agent_id();
            let result = management.authenticate(
                &server_id,
                &account,
                requesting_agent_id.as_deref(),
                force_reauth,
            )?;
            emit_and_describe_auth_result(
                result,
                force_reauth,
                &server_id,
                management,
            )
        }
        REMOVE_MCP_ACCOUNT_TOOL_NAME if management.is_multi_account_enabled() => {
            let token = required_string(object, "server_id")?;
            let Some(server_id) = resolve_server_id(management, token)? else {
                return Ok(no_installed_server_message(token));
            };
            let account = decode_mcp_account_label_argument(
                required_string(object, "account_label")?,
            );
            let servers = management.remove_account(&server_id, &account)?;
            Ok(format!(
                "Removed account \"{}\" from MCP server {token}.\n{}",
                format_mcp_account_label_for_prompt(&account),
                describe_installed_list(&servers)
            ))
        }
        RENAME_MCP_ACCOUNT_TOOL_NAME if management.is_multi_account_enabled() => {
            let token = required_string(object, "server_id")?;
            let Some(server_id) = resolve_server_id(management, token)? else {
                return Ok(no_installed_server_message(token));
            };
            let account = decode_mcp_account_label_argument(
                required_string(object, "account_label")?,
            );
            let new_account = decode_mcp_account_label_argument(
                required_string(object, "new_account_label")?,
            );
            let servers = management.rename_account(&server_id, &account, &new_account)?;
            Ok(format!(
                "Renamed account \"{}\" to \"{}\" on MCP server {token}.\n{}",
                format_mcp_account_label_for_prompt(&account),
                format_mcp_account_label_for_prompt(&new_account),
                describe_installed_list(&servers)
            ))
        }
        _ => Err(tool_error(format!("unsupported MCP management tool: {name}"))),
    }
}

fn emit_needs_auth_cards(
    before: &[McpInstalledServer],
    after: &[McpInstalledServer],
    management: &dyn McpManagementSink,
) -> Result<Option<String>, ProviderSessionError> {
    let rows = new_needs_auth_rows(before, after);
    if rows.is_empty() {
        return Ok(None);
    }
    for (id, name) in &rows {
        management.emit_connector_card(ConnectorCard {
            connector: name.clone(),
            server_id: id.clone(),
            variant: ConnectorCardVariant::Connect,
        })?;
    }
    Ok(Some(if rows.len() == 1 {
        format!(
            "\"{}\" needs authentication. {CARD_SHOWN_NOTE}",
            rows[0].1
        )
    } else {
        format!(
            "{} need authentication. {CARD_SHOWN_NOTE}",
            rows.iter()
                .map(|(_, name)| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }))
}

fn resolve_server_id(
    management: &dyn McpManagementSink,
    token: &str,
) -> Result<Option<String>, ProviderSessionError> {
    let trimmed = token.trim();
    if is_mcp_server_id(trimmed) {
        return Ok(Some(trimmed.to_string()));
    }
    let installed = match management.list_installed() {
        Ok(installed) => installed,
        Err(_) => return Ok(Some(trimmed.to_string())),
    };
    Ok(resolve_server_row(&installed, trimmed).map(|row| row.id.clone()))
}

fn resolve_server_rows<'a>(
    rows: &'a [McpInstalledServer],
    token: &str,
) -> Vec<&'a McpInstalledServer> {
    let token = token.trim();
    if token.is_empty() {
        return Vec::new();
    }
    let exact = rows
        .iter()
        .filter(|row| row.server_identifier == token)
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    rows.iter().filter(|row| row.id == token).collect()
}

fn resolve_server_row<'a>(
    rows: &'a [McpInstalledServer],
    token: &str,
) -> Option<&'a McpInstalledServer> {
    resolve_server_rows(rows, token).into_iter().next()
}

fn management_tool_names(multi_account: bool) -> BTreeSet<&'static str> {
    let mut names = [
        SEARCH_PLUGINS_TOOL_NAME,
        GET_PLUGIN_TOOL_NAME,
        INSTALL_PLUGIN_TOOL_NAME,
        ADD_MCP_SERVER_TOOL_NAME,
        UNINSTALL_MCP_SERVER_TOOL_NAME,
        UNINSTALL_PLUGIN_TOOL_NAME,
        GET_MCP_SERVER_STATUS_TOOL_NAME,
        SET_MCP_INSTRUCTIONS_TOOL_NAME,
        RESTART_MCP_SERVERS_TOOL_NAME,
        AUTHENTICATE_MCP_SERVER_TOOL_NAME,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if multi_account {
        names.insert(REMOVE_MCP_ACCOUNT_TOOL_NAME);
        names.insert(RENAME_MCP_ACCOUNT_TOOL_NAME);
    }
    names
}

fn is_management_tool_name(name: &str, multi_account: bool) -> bool {
    management_tool_names(multi_account).contains(name)
}

fn is_mutating_tool(name: &str) -> bool {
    !matches!(
        name,
        SEARCH_PLUGINS_TOOL_NAME | GET_PLUGIN_TOOL_NAME | GET_MCP_SERVER_STATUS_TOOL_NAME
    )
}

fn management_tool_definitions(multi_account: bool) -> Vec<RoutedToolDefinition> {
    let mut tools = vec![
        definition(
            SEARCH_PLUGINS_TOOL_NAME,
            "Search installable or installed plugins by natural-language query.",
            json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}}}),
        ),
        definition(
            GET_PLUGIN_TOOL_NAME,
            "Inspect one plugin by its stable plugin id.",
            json!({"type":"object","required":["plugin_id"],"additionalProperties":false,"properties":{"plugin_id":{"type":"string","minLength":1}}}),
        ),
        definition(
            INSTALL_PLUGIN_TOOL_NAME,
            "Install one marketplace plugin after user confirmation.",
            json!({"type":"object","required":["plugin_id"],"additionalProperties":false,"properties":{"plugin_id":{"type":"string","minLength":1},"values":{"type":"object","additionalProperties":{"type":"string"}}}}),
        ),
        definition(
            ADD_MCP_SERVER_TOOL_NAME,
            "Add a remote HTTP(S) MCP server after user confirmation.",
            json!({"type":"object","required":["name","url"],"additionalProperties":false,"properties":{"name":{"type":"string","minLength":1},"url":{"type":"string","minLength":1},"headers":{"type":"object","additionalProperties":{"type":"string"}}}}),
        ),
        definition(
            UNINSTALL_MCP_SERVER_TOOL_NAME,
            "Remove one custom MCP server after user confirmation.",
            server_id_schema(),
        ),
        definition(
            UNINSTALL_PLUGIN_TOOL_NAME,
            "Uninstall a whole plugin after user confirmation.",
            json!({"type":"object","required":["plugin_id"],"additionalProperties":false,"properties":{"plugin_id":{"type":"string","minLength":1}}}),
        ),
        definition(
            GET_MCP_SERVER_STATUS_TOOL_NAME,
            "Inspect runtime status for installed MCP servers.",
            json!({"type":"object","additionalProperties":false,"properties":{"server_id":{"type":"string"}}}),
        ),
        definition(
            SET_MCP_INSTRUCTIONS_TOOL_NAME,
            "Set or clear persistent connector instructions.",
            json!({"type":"object","required":["server_id","instructions"],"additionalProperties":false,"properties":{"server_id":{"type":"string","minLength":1},"instructions":{"type":"string"}}}),
        ),
        definition(
            RESTART_MCP_SERVERS_TOOL_NAME,
            "Restart installed MCP servers.",
            json!({"type":"object","additionalProperties":false}),
        ),
        definition(
            AUTHENTICATE_MCP_SERVER_TOOL_NAME,
            "Start or refresh authentication for an installed MCP server.",
            json!({"type":"object","required":["server_id"],"additionalProperties":false,"properties":{"server_id":{"type":"string","minLength":1},"force_reauth":{"type":"boolean"},"account_label":{"type":"string"}}}),
        ),
    ];
    if multi_account {
        tools.push(definition(
            REMOVE_MCP_ACCOUNT_TOOL_NAME,
            "Remove one account from an MCP server after user confirmation.",
            json!({"type":"object","required":["server_id","account_label"],"additionalProperties":false,"properties":{"server_id":{"type":"string","minLength":1},"account_label":{"type":"string","minLength":1}}}),
        ));
        tools.push(definition(
            RENAME_MCP_ACCOUNT_TOOL_NAME,
            "Rename one MCP account after user confirmation.",
            json!({"type":"object","required":["server_id","account_label","new_account_label"],"additionalProperties":false,"properties":{"server_id":{"type":"string","minLength":1},"account_label":{"type":"string","minLength":1},"new_account_label":{"type":"string","minLength":1}}}),
        ));
    }
    tools
}

fn definition(name: &str, description: &str, input_schema: Value) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: "fabushi-runner".into(),
        tool_name: name.into(),
        description: Some(description.into()),
        input_schema,
    }
}

fn server_id_schema() -> Value {
    json!({"type":"object","required":["server_id"],"additionalProperties":false,"properties":{"server_id":{"type":"string","minLength":1}}})
}

fn no_installed_server_message(token: &str) -> String {
    format!(
        "No installed MCP server \"{token}\". Run GetMcpServerStatus to list every server with its identifier."
    )
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, ProviderSessionError> {
    optional_string(object, field)?
        .ok_or_else(|| tool_error(format!("{field} is required")))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, ProviderSessionError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        Some(_) => Err(tool_error(format!("{field} must be a string"))),
    }
}

fn present_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, ProviderSessionError> {
    match object.get(field) {
        Some(Value::String(value)) => Ok(value.as_str()),
        Some(_) => Err(tool_error(format!("{field} must be a string"))),
        None => Err(tool_error(format!("{field} is required"))),
    }
}

fn optional_bool(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<bool>, ProviderSessionError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(tool_error(format!("{field} must be a boolean"))),
    }
}

fn optional_string_map(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<BTreeMap<String, String>>, ProviderSessionError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let Value::Object(values) = value else {
        return Err(tool_error(format!("{field} must be an object")));
    };
    let mut result = BTreeMap::new();
    for (key, value) in values {
        let Value::String(value) = value else {
            return Err(tool_error(format!("{field}.{key} must be a string")));
        };
        result.insert(key.clone(), value.clone());
    }
    Ok(Some(result))
}

fn tool_error(message: impl Into<String>) -> ProviderSessionError {
    ProviderSessionError::Tool(message.into())
}
