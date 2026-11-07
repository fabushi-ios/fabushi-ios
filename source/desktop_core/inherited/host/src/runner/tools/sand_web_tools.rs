use std::net::IpAddr;
use std::sync::Arc;

use serde_json::{Value, json};
use url::Url;

use crate::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedMcpMetaToolDefinition,
    RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";
pub const WEB_FETCH_TOOL_NAME: &str = "WebFetch";
pub const WEB_TOOL_PROVIDER: &str = "mahayana-web";
const WEB_FETCH_MAX_CONTENT_CHARS: usize = 100_000;
const WEB_SEARCH_MAX_INLINE_CHARS: usize = 30_000;

pub trait WebToolExecutor: Send + Sync {
    fn search(
        &self,
        search_term: &str,
        explanation: Option<&str>,
    ) -> Result<Value, ProviderSessionError>;
    fn fetch(&self, url: &str) -> Result<Value, ProviderSessionError>;
}

pub struct SandWebToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    executor: Arc<dyn WebToolExecutor>,
}

impl SandWebToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        executor: Arc<dyn WebToolExecutor>,
    ) -> Self {
        Self { delegate, executor }
    }
}

impl RoutedToolBridge for SandWebToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.extend(web_tool_definitions());
        Ok(tools)
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        self.delegate.list_mcp_meta_tools()
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
        let effective = if tool.tool_name.trim().is_empty() {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        match effective {
            WEB_SEARCH_TOOL_NAME => {
                let search_term = required_string_alias(
                    &args,
                    &["search_term", "query", "searchTerm"],
                    WEB_SEARCH_TOOL_NAME,
                )?;
                let explanation = optional_string(&args, "explanation");
                self.executor.search(search_term, explanation)
            }
            WEB_FETCH_TOOL_NAME => {
                let raw_url = required_string_alias(
                    &args,
                    &["url"],
                    WEB_FETCH_TOOL_NAME,
                )?;
                let url = validate_web_fetch_url(raw_url)?;
                self.executor.fetch(url.as_str())
            }
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}

fn web_tool_definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: WEB_SEARCH_TOOL_NAME.into(),
            provider_identifier: WEB_TOOL_PROVIDER.into(),
            tool_name: WEB_SEARCH_TOOL_NAME.into(),
            description: Some(
                "Search the web for up-to-date information and return snippets and URLs.".into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["search_term"],
                "properties": {
                    "search_term": {
                        "type": "string",
                        "description": "The search term to look up on the web."
                    },
                    "explanation": {
                        "type": "string",
                        "description": "One sentence explaining why this search contributes to the goal."
                    }
                }
            }),
        },
        RoutedToolDefinition {
            name: WEB_FETCH_TOOL_NAME.into(),
            provider_identifier: WEB_TOOL_PROVIDER.into(),
            tool_name: WEB_FETCH_TOOL_NAME.into(),
            description: Some(
                "Fetch a public http(s) URL and return its readable content. Localhost and private-network hosts are rejected.".into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "A fully-formed http:// or https:// URL."
                    }
                }
            }),
        },
    ]
}

pub fn render_web_search_result(answer: Option<&str>, documents: &[(&str, &str, &str)]) -> Value {
    let mut references = Vec::new();
    if let Some(answer) = answer.filter(|value| !value.is_empty()) {
        references.push(json!({
            "title": "Web search results",
            "url": "",
            "chunk": answer
        }));
    }
    for (title, url, text) in documents {
        if answer.is_some_and(|value| !value.is_empty()) {
            continue;
        }
        let chunk = text.chars().take(WEB_SEARCH_MAX_INLINE_CHARS).collect::<String>();
        references.push(json!({
            "title": title,
            "url": url,
            "chunk": chunk
        }));
    }
    json!({
        "success": {
            "references": references
        }
    })
}

pub fn render_web_fetch_result(
    url: &str,
    content: Option<&str>,
    error: Option<&str>,
    is_timeout: Option<bool>,
) -> Value {
    if let Some(error) = error {
        return json!({
            "error": {
                "url": url,
                "error": error,
                "isTimeout": is_timeout.unwrap_or(false)
            }
        });
    }
    let markdown = content
        .unwrap_or_default()
        .chars()
        .take(WEB_FETCH_MAX_CONTENT_CHARS)
        .collect::<String>();
    json!({
        "success": {
            "url": url,
            "markdown": markdown
        }
    })
}

fn required_string_alias<'a>(
    args: &'a Value,
    keys: &[&str],
    tool_name: &str,
) -> Result<&'a str, ProviderSessionError> {
    keys.iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderSessionError::Tool(format!(
                "{tool_name} requires {}",
                keys.first().copied().unwrap_or("an argument")
            ))
        })
}

fn optional_string<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn validate_web_fetch_url(raw: &str) -> Result<Url, ProviderSessionError> {
    let mut url = Url::parse(raw)
        .map_err(|_| ProviderSessionError::Tool(
            "Invalid URL: must include http:// or https://".into(),
        ))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProviderSessionError::Tool(format!(
            "Invalid URL protocol: {} (must be http or https)",
            url.scheme()
        )));
    }
    url.set_username("").map_err(|_| {
        ProviderSessionError::Tool("could not strip URL username".into())
    })?;
    url.set_password(None).map_err(|_| {
        ProviderSessionError::Tool("could not strip URL password".into())
    })?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(ProviderSessionError::Tool(format!(
            "Cannot fetch from localhost ({host}) because this tool runs from an isolated server."
        )));
    }
    let clean_host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = clean_host.parse::<IpAddr>()
        && is_non_public_ip(ip)
    {
        return Err(ProviderSessionError::Tool(format!(
            "Cannot fetch from private or loopback IP ({host}) because this tool runs from an isolated server."
        )));
    }
    Ok(url)
}

fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
        }
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local(),
    }
}
