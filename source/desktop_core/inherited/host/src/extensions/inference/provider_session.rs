use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::future::Future;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::{Client as AsyncClient, Response as AsyncResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;
use tokio::runtime::{Builder as TokioRuntimeBuilder, Runtime};
use tokio::time::timeout;
use uuid::Uuid;

use crate::host_paths::get_sand_root_dir;
use crate::extensions::session::typed_replay::TypedReplayEntry;

use super::cursor_inference_transport::{
    CursorCheckpoint, CursorCheckpointMessage, CursorInferenceAuth, CursorInferenceTransport,
};
use super::codex_direct_responses::{
    CodexDirectCheckpoint, CodexDirectError, CodexDirectOptions, CodexDirectTool,
    CodexProviderRefusal,
    CodexDirectTransport, CodexDirectUsage, run_codex_direct_responses_with_cancel,
    parse_codex_provider_refusal, run_codex_direct_responses_with_lifecycle_and_tool_step_reminder,
};
use super::typed_replay_serialization::{
    serialize_chat_replay, serialize_cursor_replay, serialize_responses_replay,
};

pub const FABUSHI_ROUTER_SYSTEM_PROMPT: &str =
    "You are Fabushi, a warm, concise desktop assistant.\n\
You are running inside Fabushi, not inside Codex CLI or Claude Code.\n\
The tools supplied with this request are Fabushi's already-connected plugins and accounts. Use them whenever they are relevant instead of claiming that a plugin is unavailable or asking the user to reconnect it.\n\
Never ask for an API key for an already-connected plugin. Respond directly to the user in natural language after completing any necessary tool calls.";


fn assembled_provider_system_prompt(messages: &[ProviderMessage]) -> String {
    let additions = messages
        .iter()
        .filter(|message| message.role == "system")
        .map(|message| message.content.trim())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>();
    if additions.is_empty() {
        FABUSHI_ROUTER_SYSTEM_PROMPT.to_string()
    } else {
        format!("{FABUSHI_ROUTER_SYSTEM_PROMPT}\n\n{}", additions.join("\n\n"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutedProvider {
    Fabushi,
    Cursor,
    Codex,
    ClaudeCode,
    OpenRouter,
}

impl RoutedProvider {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fabushi" => Some(Self::Fabushi),
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "claude-code" => Some(Self::ClaudeCode),
            "openrouter" => Some(Self::OpenRouter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabushi => "fabushi",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: Option<u64>,
}

impl From<CodexDirectUsage> for ProviderTokenUsage {
    fn from(value: CodexDirectUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cache_read_tokens: value.cache_read_tokens,
            cache_write_tokens: value.cache_write_tokens,
            reasoning_tokens: None,
        }
    }
}

pub fn merge_provider_token_usage(
    current: Option<ProviderTokenUsage>,
    next: ProviderTokenUsage,
) -> ProviderTokenUsage {
    match current {
        None => next,
        Some(current) => ProviderTokenUsage {
            input_tokens: current.input_tokens.saturating_add(next.input_tokens),
            output_tokens: current.output_tokens.saturating_add(next.output_tokens),
            cache_read_tokens: current.cache_read_tokens.saturating_add(next.cache_read_tokens),
            cache_write_tokens: current.cache_write_tokens.saturating_add(next.cache_write_tokens),
            reasoning_tokens: match (current.reasoning_tokens, next.reasoning_tokens) {
                (None, None) => None,
                (left, right) => Some(
                    left.unwrap_or_default().saturating_add(right.unwrap_or_default()),
                ),
            },
        },
    }
}

pub fn provider_token_usage_from_openrouter_event(
    event: &Value,
) -> Option<ProviderTokenUsage> {
    let usage = event.get("usage")?.as_object()?;
    Some(ProviderTokenUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_read_tokens: usage
            .get("prompt_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        cache_write_tokens: 0,
        reasoning_tokens: usage
            .get("completion_tokens_details")
            .and_then(Value::as_object)
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedToolDefinition {
    pub name: String,
    pub provider_identifier: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutedMcpMetaToolDefinition {
    pub tool: RoutedToolDefinition,
    pub plugin: Option<Value>,
    pub marketplace: Option<Value>,
    pub plugin_id: Option<String>,
    pub marketplace_id: Option<String>,
}

impl From<RoutedToolDefinition> for RoutedMcpMetaToolDefinition {
    fn from(tool: RoutedToolDefinition) -> Self {
        Self {
            tool,
            plugin: None,
            marketplace: None,
            plugin_id: None,
            marketplace_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderPartialToolCall {
    pub tool: RoutedToolDefinition,
    pub tool_call_id: String,
    pub raw_arguments: String,
    pub model_call_id: Option<String>,
}

pub type ProviderToolStepReminderHook =
    dyn Fn(&[(String, Value)]) -> Option<String> + Send + Sync;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", content = "checkpoint", rename_all = "kebab-case")]
pub enum RoutedProviderCheckpoint {
    Fabushi(CodexDirectCheckpoint),
    Cursor(CursorCheckpoint),
    Codex(CodexDirectCheckpoint),
    OpenRouter(OpenRouterCheckpoint),
}

impl RoutedProviderCheckpoint {
    pub fn emitted_text_bytes(&self) -> usize {
        match self {
            Self::Fabushi(checkpoint) => checkpoint.text.len(),
            Self::Cursor(checkpoint) => checkpoint.text.len(),
            Self::Codex(checkpoint) => checkpoint.text.len(),
            Self::OpenRouter(checkpoint) => checkpoint.text.len(),
        }
    }

    pub fn tool_calls_completed(&self) -> usize {
        match self {
            Self::Fabushi(checkpoint) => checkpoint.tool_calls_completed,
            Self::Cursor(checkpoint) => checkpoint.tool_calls_completed,
            Self::Codex(checkpoint) => checkpoint.tool_calls_completed,
            Self::OpenRouter(checkpoint) => checkpoint.tool_calls_completed,
        }
    }

    pub fn provider(&self) -> RoutedProvider {
        match self {
            Self::Fabushi(_) => RoutedProvider::Fabushi,
            Self::Cursor(_) => RoutedProvider::Cursor,
            Self::Codex(_) => RoutedProvider::Codex,
            Self::OpenRouter(_) => RoutedProvider::OpenRouter,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProviderSessionError {
    #[error("{0}")]
    Configuration(String),
    #[error("{0}")]
    Authentication(String),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Tool(String),
    #[error("provider refusal: {0:?}")]
    ProviderRefusal(CodexProviderRefusal),
    #[error("{0}")]
    Cancelled(String),
}

impl From<CodexDirectError> for ProviderSessionError {
    fn from(error: CodexDirectError) -> Self {
        match error {
            CodexDirectError::Transport(message) => Self::Transport(message),
            CodexDirectError::Protocol(message) => Self::Protocol(message),
            CodexDirectError::Tool(message) => Self::Tool(message),
            CodexDirectError::ProviderRefusal(refusal) => Self::ProviderRefusal(refusal),
            CodexDirectError::Cancelled(message) => Self::Cancelled(message),
        }
    }
}

pub const PROVIDER_IO_CANCEL_POLL_MS: u64 = 50;

static PROVIDER_IO_RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
static RESPONSES_HTTP_CLIENT: OnceLock<Result<AsyncClient, String>> = OnceLock::new();

fn provider_io_runtime() -> Result<&'static Runtime, ProviderSessionError> {
    PROVIDER_IO_RUNTIME
        .get_or_init(|| {
            TokioRuntimeBuilder::new_multi_thread()
                .worker_threads(1)
                .thread_name("mahayana-provider-io")
                .enable_io()
                .enable_time()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| ProviderSessionError::Transport(format!(
            "Could not create provider I/O runtime: {error}"
        )))
}

fn responses_http_client() -> Result<&'static AsyncClient, ProviderSessionError> {
    RESPONSES_HTTP_CLIENT
        .get_or_init(|| {
            AsyncClient::builder()
                .pool_idle_timeout(Duration::from_secs(90))
                .tcp_keepalive(Duration::from_secs(60))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| ProviderSessionError::Transport(format!(
            "Could not create shared Responses HTTP client: {error}"
        )))
}

async fn await_cancelable_provider_io<F, T>(
    future: F,
    should_cancel: &dyn Fn() -> bool,
    label: &str,
) -> Result<T, ProviderSessionError>
where
    F: Future<Output = T>,
{
    let mut future = Box::pin(future);
    loop {
        if should_cancel() {
            return Err(ProviderSessionError::Cancelled(format!(
                "Runner cancelled {label}"
            )));
        }
        match timeout(
            Duration::from_millis(PROVIDER_IO_CANCEL_POLL_MS),
            future.as_mut(),
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(_) => continue,
        }
    }
}

async fn await_reqwest<T, F>(
    future: F,
    should_cancel: &dyn Fn() -> bool,
    label: &str,
) -> Result<T, ProviderSessionError>
where
    F: Future<Output = Result<T, reqwest::Error>>,
{
    await_cancelable_provider_io(future, should_cancel, label)
        .await?
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))
}

fn codex_provider_error(error: ProviderSessionError) -> CodexDirectError {
    match error {
        ProviderSessionError::Cancelled(message) => CodexDirectError::Cancelled(message),
        ProviderSessionError::Protocol(message) => CodexDirectError::Protocol(message),
        ProviderSessionError::Tool(message) => CodexDirectError::Tool(message),
        ProviderSessionError::ProviderRefusal(refusal) => CodexDirectError::ProviderRefusal(refusal),
        other => CodexDirectError::Transport(other.to_string()),
    }
}

#[derive(Debug, Clone)]
struct CodexCredentials {
    access_token: String,
    refresh_token: String,
    id_token: String,
    account_id: String,
    path: PathBuf,
    document: Value,
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn codex_home() -> PathBuf {
    env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

fn configured_fabushi_model() -> String {
    env::var("FABUSHI_RESPONSES_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "deepseek-chat".into())
}

fn configured_codex_model() -> String {
    env::var("SAND_CODEX_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            fs::read_to_string(codex_home().join("config.toml"))
                .ok()
                .and_then(|config| toml_string_setting(&config, "model"))
        })
        .unwrap_or_else(|| "gpt-5.4".into())
}

fn configured_codex_reasoning_effort() -> Option<String> {
    env::var("SAND_CODEX_REASONING_EFFORT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            matches!(
                value.as_str(),
                "minimal" | "low" | "medium" | "high" | "xhigh"
            )
        })
        .or_else(|| {
            fs::read_to_string(codex_home().join("config.toml"))
                .ok()
                .and_then(|config| toml_string_setting(&config, "model_reasoning_effort"))
                .filter(|value| {
                    matches!(
                        value.as_str(),
                        "minimal" | "low" | "medium" | "high" | "xhigh"
                    )
                })
        })
}

fn toml_string_setting(config: &str, key: &str) -> Option<String> {
    config.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (name, value) = line.split_once('=')?;
        if name.trim() != key {
            return None;
        }
        let value = value.trim();
        if value.len() < 2 {
            return None;
        }
        let quote = value.as_bytes()[0];
        if (quote != b'"' && quote != b'\'')
            || value.as_bytes()[value.len() - 1] != quote
        {
            return None;
        }
        let value = value[1..value.len() - 1].trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

pub fn configured_routed_provider(settings_path: &Path) -> Option<RoutedProvider> {
    let value: Value =
        serde_json::from_str(&fs::read_to_string(settings_path).ok()?).ok()?;
    [
        value.get("inferenceProvider"),
        value.get("inference_provider"),
        value.get("router").and_then(|router| router.get("provider")),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_str().and_then(RoutedProvider::parse))
}

fn required_token(
    tokens: Option<&Map<String, Value>>,
    name: &str,
) -> Option<String> {
    tokens?
        .get(name)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn read_codex_credentials(path: &Path) -> Result<CodexCredentials, ProviderSessionError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ProviderSessionError::Authentication(
            "Codex is not signed in with ChatGPT. Run `codex login`, then reopen Fabushi."
                .into(),
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ProviderSessionError::Authentication(
            "Codex login credentials must be a private direct regular file.".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ProviderSessionError::Authentication(
                "Codex login credentials must be a private direct regular file.".into(),
            ));
        }
    }

    let document: Value =
        serde_json::from_str(&fs::read_to_string(path).map_err(|error| {
            ProviderSessionError::Authentication(format!(
                "Could not read Codex login: {error}"
            ))
        })?)
        .map_err(|error| {
            ProviderSessionError::Authentication(format!(
                "Codex login JSON is invalid: {error}"
            ))
        })?;
    if document.get("auth_mode").and_then(Value::as_str) != Some("chatgpt") {
        return Err(ProviderSessionError::Authentication(
            "Codex is not signed in with ChatGPT. Run `codex login`, then reopen Fabushi."
                .into(),
        ));
    }
    let tokens = document.get("tokens").and_then(Value::as_object);
    let Some(access_token) = required_token(tokens, "access_token") else {
        return Err(ProviderSessionError::Authentication(
            "Codex login is missing an access token. Run `codex login` again.".into(),
        ));
    };
    let Some(refresh_token) = required_token(tokens, "refresh_token") else {
        return Err(ProviderSessionError::Authentication(
            "Codex login is missing a refresh token. Run `codex login` again.".into(),
        ));
    };
    let Some(id_token) = required_token(tokens, "id_token") else {
        return Err(ProviderSessionError::Authentication(
            "Codex login is missing an ID token. Run `codex login` again.".into(),
        ));
    };
    let Some(account_id) = required_token(tokens, "account_id") else {
        return Err(ProviderSessionError::Authentication(
            "Codex login is missing an account ID. Run `codex login` again.".into(),
        ));
    };
    Ok(CodexCredentials {
        access_token,
        refresh_token,
        id_token,
        account_id,
        path: path.to_path_buf(),
        document,
    })
}

fn jwt_audience(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    match value.get("aud")? {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => values
            .iter()
            .find_map(|value| value.as_str().map(str::to_string)),
        _ => None,
    }
}

fn form_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn persist_refreshed_credentials(
    current: &CodexCredentials,
    access_token: String,
    refresh_token: String,
    id_token: String,
) -> Result<CodexCredentials, ProviderSessionError> {
    let mut document = current.document.clone();
    let root = document.as_object_mut().ok_or_else(|| {
        ProviderSessionError::Protocol("Codex auth document was not an object.".into())
    })?;
    let tokens = root
        .entry("tokens")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            ProviderSessionError::Protocol("Codex auth tokens were not an object.".into())
        })?;
    tokens.insert(
        "access_token".into(),
        Value::String(access_token.clone()),
    );
    tokens.insert(
        "refresh_token".into(),
        Value::String(refresh_token.clone()),
    );
    tokens.insert("id_token".into(), Value::String(id_token.clone()));

    let temporary = PathBuf::from(format!(
        "{}.{}.tmp",
        current.path.to_string_lossy(),
        Uuid::new_v4()
    ));
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&document)
            .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?,
    )
    .map_err(|error| {
        ProviderSessionError::Authentication(format!(
            "Could not persist refreshed Codex login: {error}"
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                ProviderSessionError::Authentication(format!(
                    "Could not secure refreshed Codex login: {error}"
                ))
            },
        )?;
    }
    fs::rename(&temporary, &current.path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ProviderSessionError::Authentication(format!(
            "Could not replace refreshed Codex login: {error}"
        ))
    })?;

    Ok(CodexCredentials {
        access_token,
        refresh_token,
        id_token,
        account_id: current.account_id.clone(),
        path: current.path.clone(),
        document,
    })
}

async fn refresh_codex_credentials(
    client: &AsyncClient,
    current: &CodexCredentials,
    should_cancel: &dyn Fn() -> bool,
) -> Result<CodexCredentials, ProviderSessionError> {
    let client_id = jwt_audience(&current.id_token).ok_or_else(|| {
        ProviderSessionError::Authentication(
            "Codex login expired and its refresh identity is invalid. Run `codex login` again."
                .into(),
        )
    })?;
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        form_encode(&current.refresh_token),
        form_encode(&client_id),
    );
    let response = await_reqwest(
        client
            .post("https://auth.openai.com/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send(),
        should_cancel,
        "the Codex login refresh request",
    )
    .await?;
    if !response.status().is_success() {
        return Err(ProviderSessionError::Authentication(
            "Codex login expired and could not be refreshed. Run `codex login` again."
                .into(),
        ));
    }
    let payload: Value = await_reqwest(
        response.json::<Value>(),
        should_cancel,
        "the Codex login refresh response",
    )
    .await
    .map_err(|error| match error {
        ProviderSessionError::Cancelled(message) => ProviderSessionError::Cancelled(message),
        other => ProviderSessionError::Protocol(format!(
            "Codex refresh response was invalid: {other}"
        )),
    })?;
    let access_token = payload
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderSessionError::Authentication(
                "Codex returned an invalid refreshed login. Run `codex login` again."
                    .into(),
            )
        })?
        .to_string();
    let refresh_token = payload
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(&current.refresh_token)
        .to_string();
    let id_token = payload
        .get("id_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(&current.id_token)
        .to_string();
    persist_refreshed_credentials(
        current,
        access_token,
        refresh_token,
        id_token,
    )
}

fn flush_sse_data<F>(
    data: &mut Vec<String>,
    on_event: &mut F,
) -> Result<(), ProviderSessionError>
where
    F: FnMut(Value) -> Result<(), ProviderSessionError> + ?Sized,
{
    if data.is_empty() {
        return Ok(());
    }
    let payload = data.join("\n");
    data.clear();
    if payload == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(&payload).map_err(|_| {
        ProviderSessionError::Protocol(
            "Provider stream contained malformed SSE JSON.".into(),
        )
    })?;
    on_event(value)
}

fn push_sse_line<F>(
    line: &str,
    data: &mut Vec<String>,
    on_event: &mut F,
) -> Result<(), ProviderSessionError>
where
    F: FnMut(Value) -> Result<(), ProviderSessionError> + ?Sized,
{
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        flush_sse_data(data, on_event)?;
    } else if let Some(payload) = trimmed.strip_prefix("data:") {
        data.push(payload.trim_start().to_string());
    }
    Ok(())
}

pub fn decode_sse_stream<R, F>(
    reader: R,
    mut on_event: F,
) -> Result<(), ProviderSessionError>
where
    R: Read,
    F: FnMut(Value) -> Result<(), ProviderSessionError>,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut data = Vec::<String>::new();

    loop {
        line.clear();
        let count = reader
            .read_line(&mut line)
            .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
        if count == 0 {
            break;
        }
        push_sse_line(&line, &mut data, &mut on_event)?;
    }
    flush_sse_data(&mut data, &mut on_event)
}

async fn decode_async_sse_stream<F>(
    mut response: AsyncResponse,
    should_cancel: &dyn Fn() -> bool,
    on_event: &mut F,
) -> Result<(), ProviderSessionError>
where
    F: FnMut(Value) -> Result<(), ProviderSessionError> + ?Sized,
{
    let mut pending = Vec::<u8>::new();
    let mut data = Vec::<String>::new();

    loop {
        let chunk = await_reqwest(
            response.chunk(),
            should_cancel,
            "the provider response stream",
        )
        .await?;
        let Some(chunk) = chunk else {
            break;
        };
        pending.extend_from_slice(&chunk);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=newline).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line).map_err(|_| {
                ProviderSessionError::Protocol(
                    "Provider stream contained invalid UTF-8.".into(),
                )
            })?;
            push_sse_line(line, &mut data, on_event)?;
        }
    }

    if !pending.is_empty() {
        let line = std::str::from_utf8(&pending).map_err(|_| {
            ProviderSessionError::Protocol(
                "Provider stream contained invalid UTF-8.".into(),
            )
        })?;
        push_sse_line(line, &mut data, on_event)?;
    }
    flush_sse_data(&mut data, on_event)
}

const FABUSHI_RESPONSES_URL_ENV: &str = "FABUSHI_RESPONSES_URL";
const FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV: &str = "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE";
const DEFAULT_FABUSHI_NATIVE_ISSUER: &str = "https://api.ombhrum.com";
const CI_ISOLATED_FABUSHI_NATIVE_ISSUER: &str = "https://fabushi-ci-isolated-auth.bhrumom.workers.dev";

#[derive(Debug, Clone)]
enum ResponsesHttpCredentials {
    ChatGpt(CodexCredentials),
    Fabushi {
        access_token: String,
        endpoint: String,
        credential_path: PathBuf,
    },
}

fn validate_fabushi_responses_url(raw: &str) -> Result<String, ProviderSessionError> {
    let url = url::Url::parse(raw).map_err(|error| {
        ProviderSessionError::Configuration(format!(
            "Fabushi Responses URL is invalid: {error}"
        ))
    })?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(loopback && url.scheme() == "http") {
        return Err(ProviderSessionError::Configuration(
            "Fabushi Responses URL must use HTTPS outside loopback development.".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(ProviderSessionError::Configuration(
            "Fabushi Responses URL must not contain credentials, query, or fragment.".into(),
        ));
    }
    let origin = url.origin().ascii_serialization();
    if !loopback && origin != DEFAULT_FABUSHI_NATIVE_ISSUER
        && origin != CI_ISOLATED_FABUSHI_NATIVE_ISSUER {
        return Err(ProviderSessionError::Configuration(
            "Fabushi Responses URL must use an approved Native issuer origin.".into(),
        ));
    }
    Ok(url.to_string())
}

fn credential_issuer(document: &Value) -> Result<String, ProviderSessionError> {
    let raw = document.get("issuer").and_then(Value::as_str)
        .unwrap_or(DEFAULT_FABUSHI_NATIVE_ISSUER);
    let url = url::Url::parse(raw).map_err(|_| ProviderSessionError::Authentication(
        "Fabushi account credential issuer is invalid.".into(),
    ))?;
    let origin = url.origin().ascii_serialization();
    if !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some()
        || (url.scheme() != "https" && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
        || (!matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
            && origin != DEFAULT_FABUSHI_NATIVE_ISSUER
            && origin != CI_ISOLATED_FABUSHI_NATIVE_ISSUER) {
        return Err(ProviderSessionError::Authentication(
            "Fabushi account credential issuer is unsupported.".into(),
        ));
    }
    Ok(origin)
}

fn ensure_responses_issuer_matches(endpoint: &str, issuer: &str) -> Result<(), ProviderSessionError> {
    let endpoint_url = url::Url::parse(endpoint).map_err(|_| ProviderSessionError::Configuration(
        "Fabushi Responses URL is invalid.".into(),
    ))?;
    if endpoint_url.origin().ascii_serialization() != issuer {
        return Err(ProviderSessionError::Authentication(
            "Fabushi account credential issuer does not match the inference service origin.".into(),
        ));
    }
    Ok(())
}

fn read_fabushi_responses_credentials(
    endpoint: &str,
    path: &Path,
) -> Result<ResponsesHttpCredentials, ProviderSessionError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ProviderSessionError::Authentication(format!(
            "Fabushi account-bound inference credential is unavailable: {error}"
        ))
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ProviderSessionError::Authentication(
            "Fabushi account-bound inference credential must be a private regular file.".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ProviderSessionError::Authentication(
                "Fabushi account-bound inference credential must be private to the current user.".into(),
            ));
        }
    }
    let document: Value = serde_json::from_str(
        &fs::read_to_string(path).map_err(|error| {
            ProviderSessionError::Authentication(format!(
                "Could not read Fabushi account-bound inference credential: {error}"
            ))
        })?,
    )
    .map_err(|error| {
        ProviderSessionError::Authentication(format!(
            "Fabushi account-bound inference credential JSON is invalid: {error}"
        ))
    })?;
    let issuer = credential_issuer(&document)?;
    ensure_responses_issuer_matches(endpoint, &issuer)?;
    let access_token = document
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderSessionError::Authentication(
                "Fabushi account-bound inference credential is missing an access token.".into(),
            )
        })?
        .to_string();
    Ok(ResponsesHttpCredentials::Fabushi {
        access_token,
        endpoint: endpoint.to_string(),
        credential_path: path.to_path_buf(),
    })
}

fn configured_fabushi_responses_credentials() -> Result<ResponsesHttpCredentials, ProviderSessionError> {
    let endpoint = env::var(FABUSHI_RESPONSES_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProviderSessionError::Configuration(
            "Fabushi Responses transport requires the first-party Responses URL.".into(),
        ))?;
    let endpoint = validate_fabushi_responses_url(&endpoint)?;
    let credential_path = env::var_os(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| ProviderSessionError::Configuration(
            "Fabushi Responses transport requires the account-bound credential file.".into(),
        ))?;
    read_fabushi_responses_credentials(&endpoint, &credential_path)
}

struct CodexHttpTransport {
    client: AsyncClient,
    credentials: ResponsesHttpCredentials,
}

impl CodexHttpTransport {
    fn with_credentials(credentials: ResponsesHttpCredentials) -> Result<Self, ProviderSessionError> {
        let client = responses_http_client()?.clone();
        Ok(Self { client, credentials })
    }

    fn new_codex(auth_path: &Path) -> Result<Self, ProviderSessionError> {
        Self::with_credentials(ResponsesHttpCredentials::ChatGpt(
            read_codex_credentials(auth_path)?,
        ))
    }

    fn new_fabushi() -> Result<Self, ProviderSessionError> {
        Self::with_credentials(configured_fabushi_responses_credentials()?)
    }
}

async fn send_codex_request(
    client: &AsyncClient,
    credentials: &ResponsesHttpCredentials,
    request: &Value,
    should_cancel: &dyn Fn() -> bool,
) -> Result<AsyncResponse, ProviderSessionError> {
    let request_builder = match credentials {
        ResponsesHttpCredentials::ChatGpt(credentials) => client
            .post("https://chatgpt.com/backend-api/codex/responses")
            .header(
                "authorization",
                format!("Bearer {}", credentials.access_token),
            )
            .header("ChatGPT-Account-Id", credentials.account_id.clone()),
        ResponsesHttpCredentials::Fabushi {
            access_token,
            endpoint,
            ..
        } => client
            .post(endpoint)
            .header("authorization", format!("Bearer {access_token}")),
    };
    await_reqwest(
        request_builder
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("user-agent", "fabushi-router/1")
            .json(request)
            .send(),
        should_cancel,
        "the Responses provider request",
    )
    .await
}

async fn refresh_responses_credentials(
    client: &AsyncClient,
    current: &ResponsesHttpCredentials,
    should_cancel: &dyn Fn() -> bool,
) -> Result<ResponsesHttpCredentials, ProviderSessionError> {
    match current {
        ResponsesHttpCredentials::ChatGpt(credentials) => Ok(
            ResponsesHttpCredentials::ChatGpt(
                refresh_codex_credentials(client, credentials, should_cancel).await?,
            ),
        ),
        ResponsesHttpCredentials::Fabushi {
            endpoint,
            credential_path,
            ..
        } => read_fabushi_responses_credentials(endpoint, credential_path),
    }
}

impl CodexDirectTransport for CodexHttpTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), CodexDirectError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), CodexDirectError> {
        let runtime = provider_io_runtime().map_err(codex_provider_error)?;
        let client = self.client.clone();
        let mut credentials = self.credentials.clone();
        let result = runtime.block_on(async {
            let mut response = send_codex_request(
                &client,
                &credentials,
                request,
                should_cancel,
            )
            .await
            .map_err(codex_provider_error)?;

            if response.status().as_u16() == 401 {
                credentials = refresh_responses_credentials(
                    &client,
                    &credentials,
                    should_cancel,
                )
                .await
                .map_err(codex_provider_error)?;
                response = send_codex_request(
                    &client,
                    &credentials,
                    request,
                    should_cancel,
                )
                .await
                .map_err(codex_provider_error)?;
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = await_reqwest(
                    response.text(),
                    should_cancel,
                    "the Responses provider error response",
                )
                .await
                .map_err(codex_provider_error)?;
                if let Ok(payload) = serde_json::from_str::<Value>(&body) {
                    if let Some(refusal) = parse_codex_provider_refusal(&payload) {
                        return Err(CodexDirectError::ProviderRefusal(refusal));
                    }
                }
                let detail = body.chars().take(4096).collect::<String>();
                return Err(CodexDirectError::Transport(format!(
                    "Responses request failed ({status}{}).",
                    if detail.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", detail.trim())
                    }
                )));
            }

            decode_async_sse_stream(
                response,
                should_cancel,
                &mut |event| {
                    on_event(event).map_err(ProviderSessionError::from)
                },
            )
            .await
            .map_err(codex_provider_error)
        });
        self.credentials = credentials;
        result
    }
}

fn to_codex_tool(tool: &RoutedToolDefinition) -> CodexDirectTool {
    CodexDirectTool {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
        source: json!({
            "name": tool.name,
            "providerIdentifier": tool.provider_identifier,
            "toolName": tool.tool_name,
            "description": tool.description,
            "inputSchema": tool.input_schema,
        }),
    }
}

fn responses_input_for_turn(
    messages: &[ProviderMessage],
    typed_replay: Option<&[TypedReplayEntry]>,
) -> Result<Vec<Value>, ProviderSessionError> {
    let mut input = match typed_replay {
        Some(entries) => serialize_responses_replay(entries)
            .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?,
        None => Vec::new(),
    };
    if typed_replay.is_none() {
        input.extend(messages.iter().filter(|message| message.role != "system").map(
            |message| {
                json!({
                    "role": if message.role == "assistant" { "assistant" } else { "user" },
                    "content": message.content,
                })
            },
        ));
        return Ok(input);
    }
    let message = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .ok_or_else(|| {
            ProviderSessionError::Protocol(
                "typed compacted replay requires a current user message".into(),
            )
        })?;
    input.push(json!({
        "role": "user",
        "content": message.content,
    }));
    Ok(input)
}

pub fn run_codex_provider_text(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
) -> Result<String, ProviderSessionError> {
    let mut ignore_checkpoint =
        |_checkpoint: &CodexDirectCheckpoint| Ok(());
    run_codex_provider_text_with_lifecycle(
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        None,
        &mut ignore_checkpoint,
    )
}

pub fn run_codex_provider_text_with_lifecycle(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_usage = |_usage: ProviderTokenUsage| {};
    run_codex_provider_text_with_lifecycle_reporting_usage(
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        &mut ignore_usage,
    )
}

pub fn run_codex_provider_text_with_lifecycle_reporting_usage(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
) -> Result<String, ProviderSessionError> {
    run_codex_provider_text_with_lifecycle_reporting_usage_with_max_steps(
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        8,
        None,
    )
}

pub fn run_codex_provider_text_with_lifecycle_reporting_usage_with_max_steps(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    run_codex_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
        messages,
        None,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        max_steps,
        tool_step_reminder,
    )
}

fn run_codex_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
    messages: &[ProviderMessage],
    typed_replay: Option<&[TypedReplayEntry]>,
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    let mut transport = CodexHttpTransport::new_codex(&codex_home().join("auth.json"))?;
    let system_prompt = assembled_provider_system_prompt(messages);
    let input = responses_input_for_turn(
        messages,
        resume_from.is_none().then_some(typed_replay).flatten(),
    )?;
    let mut request = CodexDirectOptions::new(
        configured_codex_model(),
        system_prompt,
        input,
    );
    request.reasoning_effort = configured_codex_reasoning_effort();
    request.max_steps = max_steps.max(1);
    request.tools = tools.iter().map(to_codex_tool).collect();

    let tool_index = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<BTreeMap<_, _>>();
    let result = run_codex_direct_responses_with_lifecycle_and_tool_step_reminder(
        &mut transport,
        &request,
        resume_from,
        &mut |tool, args, tool_call_id| {
            let selected = tool_index.get(&tool.name).copied().ok_or_else(|| {
                CodexDirectError::Tool(format!(
                    "Unknown Fabushi tool: {}",
                    tool.name
                ))
            })?;
            execute_tool(selected, args, tool_call_id)
                .map_err(|error| CodexDirectError::Tool(error.to_string()))
        },
        on_text_delta,
        &mut |checkpoint| {
            on_checkpoint(checkpoint)
                .map_err(|error| CodexDirectError::Transport(error.to_string()))
        },
        should_cancel,
        tool_step_reminder,
    )?;
    on_usage(result.usage.into());
    Ok(result.text)
}


fn run_fabushi_provider_text_with_lifecycle_reporting_usage_with_max_steps(
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    run_fabushi_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
        messages,
        None,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        max_steps,
        tool_step_reminder,
    )
}

fn run_fabushi_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
    messages: &[ProviderMessage],
    typed_replay: Option<&[TypedReplayEntry]>,
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&CodexDirectCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    let mut transport = CodexHttpTransport::new_fabushi()?;
    let model = configured_fabushi_model();
    let system_prompt = format!(
        "{}\n\nRuntime inference metadata: provider=Fabushi first-party Responses API; model={model}. If the user asks which provider or model is active, answer from this runtime metadata and do not infer a different vendor identity.",
        assembled_provider_system_prompt(messages),
    );
    let input = responses_input_for_turn(
        messages,
        resume_from.is_none().then_some(typed_replay).flatten(),
    )?;
    let mut request = CodexDirectOptions::new(
        model,
        system_prompt,
        input,
    );
    request.max_steps = max_steps.max(1);
    request.tools = tools.iter().map(to_codex_tool).collect();

    let tool_index = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<BTreeMap<_, _>>();
    let result = run_codex_direct_responses_with_lifecycle_and_tool_step_reminder(
        &mut transport,
        &request,
        resume_from,
        &mut |tool, args, tool_call_id| {
            let selected = tool_index.get(&tool.name).copied().ok_or_else(|| {
                CodexDirectError::Tool(format!(
                    "Unknown Fabushi tool: {}",
                    tool.name
                ))
            })?;
            execute_tool(selected, args, tool_call_id)
                .map_err(|error| CodexDirectError::Tool(error.to_string()))
        },
        on_text_delta,
        &mut |checkpoint| {
            on_checkpoint(checkpoint)
                .map_err(|error| CodexDirectError::Transport(error.to_string()))
        },
        should_cancel,
        tool_step_reminder,
    )?;
    on_usage(result.usage.into());
    Ok(result.text)
}


pub struct RoutedProviderOptions<'a> {
    pub data_dir: &'a Path,
    pub typed_replay: Option<&'a [TypedReplayEntry]>,
    pub cursor_auth: Option<Arc<dyn CursorInferenceAuth>>,
    pub tools: &'a [RoutedToolDefinition],
    pub mcp_server_url: Option<&'a str>,
    pub execute_tool: &'a mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    pub on_text_delta: &'a mut dyn FnMut(&str, &str),
    pub on_partial_tool_call: Option<&'a mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>>,
    pub should_cancel: &'a dyn Fn() -> bool,
}

pub fn run_routed_provider_text(
    provider: RoutedProvider,
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_checkpoint =
        |_checkpoint: &RoutedProviderCheckpoint| Ok(());
    run_routed_provider_text_with_lifecycle(
        provider,
        messages,
        options,
        None,
        &mut ignore_checkpoint,
    )
}

pub fn run_routed_provider_text_with_lifecycle(
    provider: RoutedProvider,
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
    resume_from: Option<&RoutedProviderCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &RoutedProviderCheckpoint,
    ) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_usage = |_usage: ProviderTokenUsage| {};
    run_routed_provider_text_with_lifecycle_reporting_usage(
        provider,
        messages,
        options,
        resume_from,
        on_checkpoint,
        &mut ignore_usage,
    )
}

pub fn run_routed_provider_text_with_lifecycle_reporting_usage(
    provider: RoutedProvider,
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
    resume_from: Option<&RoutedProviderCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &RoutedProviderCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
) -> Result<String, ProviderSessionError> {
    run_routed_provider_text_with_lifecycle_reporting_usage_with_max_steps(
        provider,
        messages,
        options,
        resume_from,
        on_checkpoint,
        on_usage,
        8,
        None,
    )
}

pub fn run_routed_provider_text_with_lifecycle_reporting_usage_with_max_steps(
    provider: RoutedProvider,
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
    resume_from: Option<&RoutedProviderCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &RoutedProviderCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    let max_steps = max_steps.max(1);
    if options.typed_replay.is_some()
        && !matches!(
            provider,
            RoutedProvider::Cursor
                | RoutedProvider::Codex
                | RoutedProvider::Fabushi
                | RoutedProvider::OpenRouter
                | RoutedProvider::ClaudeCode
        )
    {
        return Err(ProviderSessionError::Protocol(
            "typed compacted replay reached provider serialization before provider-specific message merge was enabled".into(),
        ));
    }
    if (options.should_cancel)() {
        return Err(ProviderSessionError::Cancelled(
            "Runner cancelled before provider dispatch".into(),
        ));
    }
    if let Some(checkpoint) = resume_from {
        if checkpoint.provider() != provider {
            return Err(ProviderSessionError::Protocol(format!(
                "Runner checkpoint belongs to {} but {} was selected",
                checkpoint.provider().as_str(),
                provider.as_str()
            )));
        }
    }

    let result = match provider {
        RoutedProvider::Fabushi => {
            let resume = match resume_from {
                Some(RoutedProviderCheckpoint::Fabushi(checkpoint)) => Some(checkpoint),
                _ => None,
            };
            let mut fabushi_checkpoint = |checkpoint: &CodexDirectCheckpoint| {
                on_checkpoint(&RoutedProviderCheckpoint::Fabushi(checkpoint.clone()))
            };
            run_fabushi_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
                messages,
                options.typed_replay,
                options.tools,
                options.execute_tool,
                options.on_text_delta,
                options.should_cancel,
                resume,
                &mut fabushi_checkpoint,
                on_usage,
                max_steps,
                tool_step_reminder,
            )
        }
        RoutedProvider::Cursor => {
            let auth = options.cursor_auth.clone().ok_or_else(|| {
                ProviderSessionError::Configuration(
                    "Cursor inference requires the typed Host auth binding.".into(),
                )
            })?;
            let resume = match resume_from {
                Some(RoutedProviderCheckpoint::Cursor(checkpoint)) => Some(checkpoint),
                _ => None,
            };
            let transport = CursorInferenceTransport::new(Arc::clone(&auth));
            let mut cursor_checkpoint = |checkpoint: &CursorCheckpoint| {
                on_checkpoint(&RoutedProviderCheckpoint::Cursor(checkpoint.clone()))
            };
            let mut ignore_partial = |_partial: ProviderPartialToolCall| Ok(());
            let on_partial_tool_call = options
                .on_partial_tool_call
                .as_deref_mut()
                .unwrap_or(&mut ignore_partial);
            let initial_conversation = resume
                .is_none()
                .then_some(options.typed_replay)
                .flatten()
                .map(serialize_cursor_replay)
                .transpose()
                .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?
                .map(|replay| {
                    let latest_user = messages
                        .iter()
                        .rev()
                        .find(|message| message.role == "user")
                        .cloned();
                    let mut merged = messages
                        .iter()
                        .filter(|message| message.role == "system")
                        .map(|message| CursorCheckpointMessage::Text {
                            role: message.role.clone(),
                            text: message.content.clone(),
                        })
                        .collect::<Vec<_>>();
                    merged.extend(replay);
                    if let Some(message) = latest_user {
                        merged.push(CursorCheckpointMessage::Text {
                            role: message.role,
                            text: message.content,
                        });
                    }
                    merged
                });
            let result = super::cursor_inference_transport::run_cursor_with_transport_reporting_usage_and_partials_with_initial_conversation(
                &transport,
                messages,
                initial_conversation.as_deref(),
                options.tools,
                options.execute_tool,
                options.on_text_delta,
                options.should_cancel,
                resume,
                &mut cursor_checkpoint,
                on_usage,
                on_partial_tool_call,
                max_steps,
                tool_step_reminder,
            );
            result
        }
        RoutedProvider::Codex => {
            let resume = match resume_from {
                Some(RoutedProviderCheckpoint::Codex(checkpoint)) => {
                    Some(checkpoint)
                }
                _ => None,
            };
            let mut codex_checkpoint = |checkpoint: &CodexDirectCheckpoint| {
                on_checkpoint(&RoutedProviderCheckpoint::Codex(
                    checkpoint.clone(),
                ))
            };
            run_codex_provider_text_with_lifecycle_reporting_usage_with_max_steps_and_replay(
                messages,
                options.typed_replay,
                options.tools,
                options.execute_tool,
                options.on_text_delta,
                options.should_cancel,
                resume,
                &mut codex_checkpoint,
                on_usage,
                max_steps,
                tool_step_reminder,
            )
        }
        RoutedProvider::OpenRouter => {
            let resume = match resume_from {
                Some(RoutedProviderCheckpoint::OpenRouter(checkpoint)) => {
                    Some(checkpoint)
                }
                _ => None,
            };
            let mut openrouter_checkpoint =
                |checkpoint: &OpenRouterCheckpoint| {
                    on_checkpoint(&RoutedProviderCheckpoint::OpenRouter(
                        checkpoint.clone(),
                    ))
                };
            run_openrouter_provider_text_with_lifecycle_reporting_usage(
                messages,
                options,
                resume,
                &mut openrouter_checkpoint,
                on_usage,
                max_steps,
                tool_step_reminder,
            )
        }
        RoutedProvider::ClaudeCode => {
            if resume_from.is_some() {
                Err(ProviderSessionError::Protocol(
                    "Claude Code does not emit a resumable tool-boundary checkpoint."
                        .into(),
                ))
            } else {
                run_claude_code_provider_text(messages, options)
            }
        }
    };
    if (options.should_cancel)() {
        return Err(ProviderSessionError::Cancelled(
            "Runner cancelled the provider request".into(),
        ));
    }
    result
}

fn provider_prompt(messages: &[ProviderMessage]) -> String {
    let system_prompt = assembled_provider_system_prompt(messages);
    let rendered = messages
        .iter()
        .filter(|message| message.role != "system")
        .map(|message| {
            format!(
                "{}: {}",
                message.role.to_ascii_uppercase(),
                message.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "{system_prompt}\n\nContinue this Fabushi conversation.\n\n{rendered}"
    )
}

fn openrouter_api_key(data_dir: &Path) -> Result<String, ProviderSessionError> {
    if let Some(value) = env::var("OPENROUTER_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(value);
    }
    for path in [
        data_dir.join("box-secrets.json"),
        get_sand_root_dir().join("box-secrets.json"),
    ] {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(key) = value
            .get("secrets")
            .and_then(Value::as_object)
            .and_then(|secrets| secrets.get("OPENROUTER_API_KEY"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(key.to_string());
        }
    }
    Err(ProviderSessionError::Authentication(
        "OpenRouter needs OPENROUTER_API_KEY. Add it in Settings -> Router.".into(),
    ))
}

fn openrouter_tools(tools: &[RoutedToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description.clone().unwrap_or_else(|| {
                        format!("{} via {}", tool.tool_name, tool.provider_identifier)
                    }),
                    "parameters": tool.input_schema,
                }
            })
        })
        .collect()
}

#[derive(Default)]
struct PartialOpenRouterToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRouterCheckpoint {
    pub conversation: Vec<Value>,
    pub text: String,
    pub completed_steps: usize,
    pub tool_calls_completed: usize,
}

pub trait OpenRouterTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), ProviderSessionError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), ProviderSessionError>;
}

struct OpenRouterHttpTransport {
    client: AsyncClient,
    runtime: &'static Runtime,
    api_key: String,
}

impl OpenRouterHttpTransport {
    fn new(api_key: String) -> Result<Self, ProviderSessionError> {
        Ok(Self {
            client: AsyncClient::builder()
                .build()
                .map_err(|error| ProviderSessionError::Transport(error.to_string()))?,
            runtime: provider_io_runtime()?,
            api_key,
        })
    }
}

impl OpenRouterTransport for OpenRouterHttpTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), ProviderSessionError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), ProviderSessionError> {
        let response = self.runtime.block_on(await_reqwest(
            self.client
                .post("https://openrouter.ai/api/v1/chat/completions")
                .header("authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .header(
                    "HTTP-Referer",
                    "https://github.com/bhrumom/fabushi-desktop",
                )
                .header("X-Title", "Fabushi")
                .json(request)
                .send(),
            should_cancel,
            "the OpenRouter provider request",
        ))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = self
                .runtime
                .block_on(await_reqwest(
                    response.text(),
                    should_cancel,
                    "the OpenRouter provider error response",
                ))?
                .chars()
                .take(4096)
                .collect::<String>();
            return Err(ProviderSessionError::Transport(format!(
                "OpenRouter request failed ({status}{}).",
                if detail.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.trim())
                }
            )));
        }
        self.runtime.block_on(decode_async_sse_stream(
            response,
            should_cancel,
            on_event,
        ))
    }
}

pub fn run_openrouter_with_transport(
    transport: &mut dyn OpenRouterTransport,
    model: &str,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_usage = |_usage: ProviderTokenUsage| {};
    run_openrouter_with_transport_reporting_usage(
        transport,
        model,
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        &mut ignore_usage,
    )
}

pub fn run_openrouter_with_transport_reporting_usage(
    transport: &mut dyn OpenRouterTransport,
    model: &str,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
) -> Result<String, ProviderSessionError> {
    let mut ignore_partial = |_partial: ProviderPartialToolCall| Ok(());
    run_openrouter_with_transport_reporting_usage_and_partials(
        transport, model, messages, tools, execute_tool, on_text_delta,
        should_cancel, resume_from, on_checkpoint, on_usage, &mut ignore_partial,
    )
}

pub fn run_openrouter_with_transport_reporting_usage_and_partials(
    transport: &mut dyn OpenRouterTransport,
    model: &str,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    run_openrouter_with_transport_reporting_usage_and_partials_with_max_steps(
        transport,
        model,
        messages,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        on_partial_tool_call,
        8,
        None,
    )
}

pub fn run_openrouter_with_transport_reporting_usage_and_partials_with_max_steps(
    transport: &mut dyn OpenRouterTransport,
    model: &str,
    messages: &[ProviderMessage],
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    run_openrouter_with_transport_reporting_usage_and_partials_with_initial_conversation(
        transport,
        model,
        messages,
        None,
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        on_partial_tool_call,
        max_steps,
        tool_step_reminder,
    )
}

pub fn run_openrouter_with_transport_reporting_usage_and_partials_with_initial_conversation(
    transport: &mut dyn OpenRouterTransport,
    model: &str,
    messages: &[ProviderMessage],
    initial_conversation: Option<&[Value]>,
    tools: &[RoutedToolDefinition],
    execute_tool: &mut dyn FnMut(
        &RoutedToolDefinition,
        Value,
        &str,
    ) -> Result<Value, ProviderSessionError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    on_partial_tool_call: &mut dyn FnMut(ProviderPartialToolCall) -> Result<(), ProviderSessionError>,
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    let max_steps = max_steps.max(1);
    let tool_index = tools
        .iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<BTreeMap<_, _>>();
    let declared_tools = openrouter_tools(tools);
    let (
        mut conversation,
        mut text,
        mut completed_steps,
        mut tool_calls_completed,
    ) = match resume_from {
        Some(checkpoint) => (
            checkpoint.conversation.clone(),
            checkpoint.text.clone(),
            checkpoint.completed_steps,
            checkpoint.tool_calls_completed,
        ),
        None => {
            let system_prompt = assembled_provider_system_prompt(messages);
            let mut conversation = vec![json!({
                "role": "system",
                "content": system_prompt
            })];
            if let Some(initial_conversation) = initial_conversation {
                conversation.extend(initial_conversation.iter().cloned());
            } else {
                conversation.extend(
                    messages
                        .iter()
                        .filter(|message| message.role != "system")
                        .map(|message| {
                            json!({
                                "role": if message.role == "assistant" {
                                    "assistant"
                                } else {
                                    "user"
                                },
                                "content": message.content,
                            })
                        }),
                );
            }
            (conversation, String::new(), 0, 0)
        }
    };

    if completed_steps >= max_steps {
        return Err(ProviderSessionError::Protocol(format!(
            "OpenRouter resume checkpoint already exhausted Fabushi's {max_steps}-step tool limit."
        )));
    }

    for _step in completed_steps..max_steps {
        if should_cancel() {
            return Err(ProviderSessionError::Cancelled(
                "Runner cancelled the OpenRouter request".into(),
            ));
        }
        let mut request = json!({
            "model": model,
            "messages": conversation,
            "stream": true,
            "usage": { "include": true },
        });
        if !declared_tools.is_empty() {
            request["tools"] = Value::Array(declared_tools.clone());
            request["tool_choice"] = Value::String("auto".into());
        }

        let mut partial_calls =
            BTreeMap::<usize, PartialOpenRouterToolCall>::new();
        let mut step_text = String::new();
        let mut step_usage = None::<ProviderTokenUsage>;
        let mut step_model_call_id = String::new();
        transport.stream_response(
            &request,
            &mut |event| {
                if should_cancel() {
                    return Err(ProviderSessionError::Cancelled(
                        "Runner cancelled the OpenRouter stream".into(),
                    ));
                }
                if let Some(error) = event.get("error") {
                    return Err(ProviderSessionError::Protocol(format!(
                        "OpenRouter stream failed: {error}"
                    )));
                }
                if let Some(id) = event.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
                    step_model_call_id = id.to_string();
                }
                if let Some(usage) = provider_token_usage_from_openrouter_event(&event) {
                    step_usage = Some(usage);
                }
                let Some(delta) = event
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|choices| choices.first())
                    .and_then(|choice| choice.get("delta"))
                    .and_then(Value::as_object)
                else {
                    return Ok(());
                };
                if let Some(content) =
                    delta.get("content").and_then(Value::as_str)
                {
                    step_text.push_str(content);
                    text.push_str(content);
                    on_text_delta(content, &text);
                }
                if let Some(calls) =
                    delta.get("tool_calls").and_then(Value::as_array)
                {
                    for call in calls {
                        let index = call
                            .get("index")
                            .and_then(Value::as_u64)
                            .unwrap_or(0) as usize;
                        let partial =
                            partial_calls.entry(index).or_default();
                        if let Some(id) =
                            call.get("id").and_then(Value::as_str)
                        {
                            partial.id.push_str(id);
                        }
                        if let Some(function) =
                            call.get("function").and_then(Value::as_object)
                        {
                            if let Some(name) =
                                function.get("name").and_then(Value::as_str)
                            {
                                partial.name.push_str(name);
                            }
                            if let Some(arguments) =
                                function.get("arguments").and_then(Value::as_str)
                            {
                                partial.arguments.push_str(arguments);
                            }
                        }
                        if !partial.id.is_empty() && !partial.name.is_empty() {
                            if let Some(tool) = tool_index.get(&partial.name).copied() {
                                on_partial_tool_call(ProviderPartialToolCall {
                                    tool: tool.clone(),
                                    tool_call_id: partial.id.clone(),
                                    raw_arguments: partial.arguments.clone(),
                                    model_call_id: (!step_model_call_id.is_empty())
                                        .then(|| step_model_call_id.clone()),
                                })?;
                            }
                        }
                    }
                }
                Ok(())
            },
            should_cancel,
        )?;
        if let Some(usage) = step_usage {
            on_usage(usage);
        }

        if partial_calls.is_empty() {
            return Ok(text);
        }

        let calls_in_step = partial_calls.len();
        let tool_calls = partial_calls
            .iter()
            .map(|(index, call)| {
                json!({
                    "index": index,
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments
                    }
                })
            })
            .collect::<Vec<_>>();
        conversation.push(json!({
            "role": "assistant",
            "content": if step_text.is_empty() {
                Value::Null
            } else {
                Value::String(step_text)
            },
            "tool_calls": tool_calls,
        }));

        let mut observed_tool_calls = Vec::with_capacity(calls_in_step);
        for (_index, call) in partial_calls {
            if should_cancel() {
                return Err(ProviderSessionError::Cancelled(
                    "Runner cancelled before OpenRouter tool execution".into(),
                ));
            }
            let tool_call_id = if call.id.is_empty() {
                Uuid::new_v4().to_string()
            } else {
                call.id
            };
            let args = serde_json::from_str::<Value>(&call.arguments)
                .unwrap_or_else(|_| json!({}));
            observed_tool_calls.push((call.name.clone(), args.clone()));
            let result = match tool_index.get(&call.name).copied() {
                None => json!({
                    "isError": true,
                    "error": format!("Unknown Fabushi tool: {}", call.name)
                }),
                Some(tool) => match execute_tool(tool, args, &tool_call_id) {
                    Ok(value) => value,
                    Err(error) => json!({
                        "isError": true,
                        "error": error.to_string()
                    }),
                },
            };
            conversation.push(json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": serde_json::to_string(&result)
                    .unwrap_or_else(|_| "null".into()),
            }));
        }
        if let Some(reminder) =
            tool_step_reminder.and_then(|hook| hook(&observed_tool_calls))
        {
            conversation.push(json!({
                "role": "user",
                "content": reminder,
            }));
        }

        completed_steps = completed_steps.saturating_add(1);
        tool_calls_completed =
            tool_calls_completed.saturating_add(calls_in_step);
        on_checkpoint(&OpenRouterCheckpoint {
            conversation: conversation.clone(),
            text: text.clone(),
            completed_steps,
            tool_calls_completed,
        })?;
    }

    Err(ProviderSessionError::Protocol(format!(
        "OpenRouter exceeded Fabushi's {max_steps}-step tool limit."
    )))
}

fn run_openrouter_provider_text(
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_checkpoint =
        |_checkpoint: &OpenRouterCheckpoint| Ok(());
    run_openrouter_provider_text_with_lifecycle(
        messages,
        options,
        None,
        &mut ignore_checkpoint,
    )
}

fn run_openrouter_provider_text_with_lifecycle(
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
) -> Result<String, ProviderSessionError> {
    let mut ignore_usage = |_usage: ProviderTokenUsage| {};
    run_openrouter_provider_text_with_lifecycle_reporting_usage(
        messages,
        options,
        resume_from,
        on_checkpoint,
        &mut ignore_usage,
        8,
        None,
    )
}

fn run_openrouter_provider_text_with_lifecycle_reporting_usage(
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
    resume_from: Option<&OpenRouterCheckpoint>,
    on_checkpoint: &mut dyn FnMut(
        &OpenRouterCheckpoint,
    ) -> Result<(), ProviderSessionError>,
    on_usage: &mut dyn FnMut(ProviderTokenUsage),
    max_steps: usize,
    tool_step_reminder: Option<&ProviderToolStepReminderHook>,
) -> Result<String, ProviderSessionError> {
    let api_key = openrouter_api_key(options.data_dir)?;
    let model = env::var("SAND_OPENROUTER_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "openai/gpt-5.2".into());
    let tools = options.tools;
    let should_cancel = options.should_cancel;
    let execute_tool = &mut *options.execute_tool;
    let on_text_delta = &mut *options.on_text_delta;
    let mut transport = OpenRouterHttpTransport::new(api_key)?;
    let mut ignore_partial = |_partial: ProviderPartialToolCall| Ok(());
    let on_partial_tool_call = options
        .on_partial_tool_call
        .as_deref_mut()
        .unwrap_or(&mut ignore_partial);
    let initial_conversation = options
        .typed_replay
        .filter(|_| resume_from.is_none())
        .map(serialize_chat_replay)
        .transpose()
        .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?
        .map(|mut replay| {
            let message = messages
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .ok_or_else(|| {
                    ProviderSessionError::Protocol(
                        "typed compacted replay requires a current user message".into(),
                    )
                })?;
            replay.push(json!({
                "role": "user",
                "content": message.content,
            }));
            Ok::<_, ProviderSessionError>(replay)
        })
        .transpose()?;
    run_openrouter_with_transport_reporting_usage_and_partials_with_initial_conversation(
        &mut transport,
        &model,
        messages,
        initial_conversation.as_deref(),
        tools,
        execute_tool,
        on_text_delta,
        should_cancel,
        resume_from,
        on_checkpoint,
        on_usage,
        on_partial_tool_call,
        max_steps,
        tool_step_reminder,
    )
}

fn resolve_claude_cli_path() -> Option<PathBuf> {
    let home = home_dir();
    let mut candidates = vec![
        env::var_os("CLAUDE_CODE_PATH").map(PathBuf::from),
        Some(home.join(".local").join("bin").join("claude")),
        Some(home.join(".claude").join("local").join("claude")),
    ];
    if let Some(path) = env::var_os("PATH") {
        candidates.extend(
            env::split_paths(&path)
                .map(|directory| Some(directory.join("claude"))),
        );
    }
    candidates.extend([
        Some(PathBuf::from("/opt/homebrew/bin/claude")),
        Some(PathBuf::from("/usr/local/bin/claude")),
    ]);
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
}

fn run_claude_code_provider_text(
    messages: &[ProviderMessage],
    options: &mut RoutedProviderOptions<'_>,
) -> Result<String, ProviderSessionError> {
    let prompt = claude_prompt_for_turn(messages, options.typed_replay)?;
    let executable = resolve_claude_cli_path().ok_or_else(|| {
        ProviderSessionError::Configuration(
            "Claude Code is not installed. Install and sign in to Claude Code, then reopen Fabushi."
                .into(),
        )
    })?;
    let mut command = Command::new(executable);
    command
        .arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--max-turns")
        .arg(if options.mcp_server_url.is_some() {
            "8"
        } else {
            "1"
        });

    let mut mcp_config_path = None;
    if let Some(url) = options.mcp_server_url {
        let path = env::temp_dir().join(format!(
            "fabushi-claude-mcp-{}.json",
            Uuid::new_v4()
        ));
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "mcpServers": {
                    "fabushi_plugins": {
                        "type": "http",
                        "url": url
                    }
                }
            }))
            .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?,
        )
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
        command
            .arg("--mcp-config")
            .arg(&path)
            .arg("--strict-mcp-config")
            .arg("--allowedTools")
            .arg("mcp__fabushi_plugins__*");
        mcp_config_path = Some(path);
    }
    if let Some(model) = env::var("SAND_CLAUDE_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        command.arg("--model").arg(model);
    }

    if (options.should_cancel)() {
        if let Some(path) = mcp_config_path {
            let _ = fs::remove_file(path);
        }
        return Err(ProviderSessionError::Cancelled(
            "Runner cancelled before Claude Code dispatch".into(),
        ));
    }

    let stdout_path = env::temp_dir().join(format!(
        "fabushi-claude-stdout-{}.json",
        Uuid::new_v4()
    ));
    let stderr_path = env::temp_dir().join(format!(
        "fabushi-claude-stderr-{}.txt",
        Uuid::new_v4()
    ));
    let stdout_file = fs::File::create(&stdout_path)
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
    let stderr_file = fs::File::create(&stderr_path)
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
    command
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    let mut child = command.spawn().map_err(|error| {
        ProviderSessionError::Transport(format!(
            "Could not run Claude Code: {error}"
        ))
    })?;
    let status = loop {
        if (options.should_cancel)() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(path) = mcp_config_path.as_ref() {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_file(&stdout_path);
            let _ = fs::remove_file(&stderr_path);
            return Err(ProviderSessionError::Cancelled(
                "Runner cancelled Claude Code".into(),
            ));
        }
        match child
            .try_wait()
            .map_err(|error| ProviderSessionError::Transport(error.to_string()))?
        {
            Some(status) => break status,
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    if let Some(path) = mcp_config_path {
        let _ = fs::remove_file(path);
    }
    let stdout_bytes = fs::read(&stdout_path)
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
    let stderr_bytes = fs::read(&stderr_path)
        .map_err(|error| ProviderSessionError::Transport(error.to_string()))?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    if !status.success() {
        return Err(ProviderSessionError::Transport(format!(
            "Claude Code failed: {}",
            String::from_utf8_lossy(&stderr_bytes).trim()
        )));
    }
    let stdout = String::from_utf8(stdout_bytes)
        .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?;
    let payload = serde_json::from_str::<Value>(stdout.trim())
        .ok()
        .or_else(|| {
            stdout
                .lines()
                .rev()
                .find_map(|line| serde_json::from_str::<Value>(line).ok())
        })
        .ok_or_else(|| {
            ProviderSessionError::Protocol(
                "Claude Code returned invalid JSON.".into(),
            )
        })?;
    if payload
        .get("subtype")
        .and_then(Value::as_str)
        .is_some_and(|value| value != "success")
    {
        return Err(ProviderSessionError::Protocol(format!(
            "Claude Code failed: {}",
            payload
                .get("errors")
                .cloned()
                .unwrap_or_else(|| Value::String("unknown failure".into()))
        )));
    }
    let text = payload
        .get("result")
        .and_then(Value::as_str)
        .or_else(|| payload.get("content").and_then(Value::as_str))
        .ok_or_else(|| {
            ProviderSessionError::Protocol(
                "Claude Code ended without a result.".into(),
            )
        })?
        .to_string();
    if !text.is_empty() {
        (options.on_text_delta)(&text, &text);
    }
    Ok(text)
}

fn claude_prompt_for_turn(
    messages: &[ProviderMessage],
    typed_replay: Option<&[TypedReplayEntry]>,
) -> Result<String, ProviderSessionError> {
    let Some(entries) = typed_replay else {
        return Ok(provider_prompt(messages));
    };
    let replay = super::typed_replay_serialization::serialize_claude_replay(entries)
        .map_err(|error| ProviderSessionError::Protocol(error.to_string()))?;
    let current_user = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .ok_or_else(|| {
            ProviderSessionError::Protocol(
                "typed compacted replay requires a current user message".into(),
            )
        })?;
    Ok(format!(
        "{}\n\nContinue this Fabushi conversation.\n\n{}\n\nUSER: {}",
        assembled_provider_system_prompt(messages),
        replay,
        current_user.content
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CI_ISOLATED_FABUSHI_NATIVE_ISSUER, DEFAULT_FABUSHI_NATIVE_ISSUER,
        FABUSHI_ROUTER_SYSTEM_PROMPT, RoutedProvider, await_cancelable_provider_io,
        credential_issuer, ensure_responses_issuer_matches, jwt_audience,
        provider_io_runtime, responses_http_client,
        toml_string_setting, validate_fabushi_responses_url,
    };
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use std::cell::Cell;
    use std::future::pending;

    #[test]
    fn fabushi_router_prompt_declares_product_identity() {
        assert!(FABUSHI_ROUTER_SYSTEM_PROMPT.contains("You are Fabushi"));
        assert!(FABUSHI_ROUTER_SYSTEM_PROMPT.contains("Fabushi's already-connected plugins"));
    }

    #[test]
    fn fabushi_inference_rejects_cross_issuer_targets_before_bearer_dispatch() {
        let production = validate_fabushi_responses_url("https://api.ombhrum.com/responses").unwrap();
        let isolated = validate_fabushi_responses_url(CI_ISOLATED_FABUSHI_NATIVE_ISSUER).unwrap();
        assert_eq!(credential_issuer(&serde_json::json!({})).unwrap(), DEFAULT_FABUSHI_NATIVE_ISSUER);
        assert_eq!(
            credential_issuer(&serde_json::json!({"issuer": CI_ISOLATED_FABUSHI_NATIVE_ISSUER})).unwrap(),
            CI_ISOLATED_FABUSHI_NATIVE_ISSUER,
        );
        assert!(ensure_responses_issuer_matches(&production, DEFAULT_FABUSHI_NATIVE_ISSUER).is_ok());
        assert!(ensure_responses_issuer_matches(&isolated, CI_ISOLATED_FABUSHI_NATIVE_ISSUER).is_ok());
        assert!(ensure_responses_issuer_matches(&production, CI_ISOLATED_FABUSHI_NATIVE_ISSUER).is_err());
        assert!(ensure_responses_issuer_matches(&isolated, DEFAULT_FABUSHI_NATIVE_ISSUER).is_err());
        assert!(validate_fabushi_responses_url("https://untrusted.example/responses").is_err());
        assert!(credential_issuer(&serde_json::json!({"issuer": "https://untrusted.example"})).is_err());
    }

    #[test]
    fn parses_reference_provider_names() {
        assert_eq!(RoutedProvider::parse("fabushi"), Some(RoutedProvider::Fabushi));
        assert_eq!(RoutedProvider::parse("cursor"), Some(RoutedProvider::Cursor));
        assert_eq!(RoutedProvider::parse("codex"), Some(RoutedProvider::Codex));
        assert_eq!(
            RoutedProvider::parse("claude-code"),
            Some(RoutedProvider::ClaudeCode)
        );
        assert_eq!(
            RoutedProvider::parse("openrouter"),
            Some(RoutedProvider::OpenRouter)
        );
        assert_eq!(RoutedProvider::parse("other"), None);
    }

    #[test]
    fn reads_quoted_codex_settings_without_accepting_comments() {
        assert_eq!(
            toml_string_setting("model = \"gpt-5.4\"\n", "model").as_deref(),
            Some("gpt-5.4")
        );
        assert_eq!(toml_string_setting("# model = \"x\"\n", "model"), None);
    }

    #[test]
    fn extracts_oauth_client_id_from_jwt_audience() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"aud":"client-123"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(jwt_audience(&token).as_deref(), Some("client-123"));
    }
    #[test]
    fn provider_io_runtime_is_process_shared() {
        let first = provider_io_runtime().expect("first provider I/O runtime");
        let second = provider_io_runtime().expect("second provider I/O runtime");
        assert!(
            std::ptr::eq(first, second),
            "provider turns must reuse one process-owned Tokio runtime so HTTP keep-alive survives between turns"
        );
    }

    #[test]
    fn responses_http_client_is_process_shared() {
        let first = responses_http_client().expect("first Responses HTTP client");
        let second = responses_http_client().expect("second Responses HTTP client");
        assert!(
            std::ptr::eq(first, second),
            "provider turns must reuse one process-owned HTTP client and connection pool"
        );
    }

    #[test]
    fn cancellable_provider_io_returns_while_the_io_future_is_silent() {
        let checks = Cell::new(0_u32);
        let should_cancel = || {
            let seen = checks.get();
            checks.set(seen.saturating_add(1));
            seen >= 1
        };
        let runtime = provider_io_runtime().expect("provider I/O runtime");
        let error = runtime
            .block_on(await_cancelable_provider_io(
                pending::<()>(),
                &should_cancel,
                "a silent provider probe",
            ))
            .expect_err("silent I/O must be interruptible by Runner cancellation");
        assert!(matches!(error, super::ProviderSessionError::Cancelled(_)));
        assert!(
            checks.get() >= 2,
            "cancellation must be re-checked without requiring provider output"
        );
    }

}
