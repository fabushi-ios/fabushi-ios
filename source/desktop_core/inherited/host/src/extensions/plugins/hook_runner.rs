use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookName {
    BeforeInit,
    AfterInit,
    BeforeReply,
    AfterReply,
    BeforeCommand,
    AfterCommand,
    BeforeToolCall,
}

pub type HookHandler = Arc<dyn Fn(&Value) -> Result<Option<Value>, String> + Send + Sync>;

#[derive(Clone)]
pub struct RegisteredHook {
    pub plugin_id: String,
    pub handler: HookHandler,
    pub priority: i32,
}

#[derive(Clone, Default)]
pub struct PluginHookRunner {
    hooks: Arc<Mutex<HashMap<HookName, Vec<RegisteredHook>>>>,
}

impl PluginHookRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_hook<F>(&self, hook_name: HookName, plugin_id: &str, priority: i32, handler: F)
    where
        F: Fn(&Value) -> Result<Option<Value>, String> + Send + Sync + 'static,
    {
        let mut hooks = self.hooks.lock().unwrap();
        let list = hooks.entry(hook_name).or_default();
        list.push(RegisteredHook {
            plugin_id: plugin_id.to_string(),
            handler: Arc::new(handler),
            priority,
        });
        // Sort by priority descending (higher priority runs first)
        list.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn run_hooks(&self, hook_name: HookName, initial_payload: &Value) -> Result<Value, String> {
        let handlers = {
            let hooks = self.hooks.lock().unwrap();
            hooks.get(&hook_name).cloned().unwrap_or_default()
        };

        let mut current_payload = initial_payload.clone();

        for registered in handlers {
            match (registered.handler)(&current_payload) {
                Ok(Some(modified)) => {
                    current_payload = modified;
                }
                Ok(None) => {
                    // Retain current payload
                }
                Err(err) => {
                    // Fail-soft: record warning/error but continue pipeline unless payload is invalid
                    eprintln!(
                        "[plugin_hook] Error running hook {:?} for plugin '{}': {err}",
                        hook_name, registered.plugin_id
                    );
                }
            }
        }

        Ok(current_payload)
    }

    pub fn hook_count(&self, hook_name: HookName) -> usize {
        let hooks = self.hooks.lock().unwrap();
        hooks.get(&hook_name).map(|v| v.len()).unwrap_or(0)
    }
}

pub type ProviderExtensionHandler = Arc<dyn Fn(&str, &Value) -> Result<Value, String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct ProviderExtensionBridge {
    providers: Arc<Mutex<HashMap<String, ProviderExtensionHandler>>>,
}

impl ProviderExtensionBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider<F>(&self, provider_id: &str, handler: F)
    where
        F: Fn(&str, &Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let mut providers = self.providers.lock().unwrap();
        providers.insert(provider_id.to_ascii_lowercase(), Arc::new(handler));
    }

    pub fn invoke_provider(
        &self,
        provider_id: &str,
        operation: &str,
        params: &Value,
        _timeout: Duration,
    ) -> Result<Value, String> {
        let handler = {
            let providers = self.providers.lock().unwrap();
            providers
                .get(&provider_id.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| format!("Unknown provider extension: '{provider_id}'"))?
        };

        handler(operation, params)
    }
}


// Hook runner boundary guards & steering manager
pub use super::hook_runner_guards::*;

pub const MAX_DATE_TIMESTAMP_MS: u64 = 8_640_000_000_000_000;
pub const FILE_TYPE_SNIFF_MAX_BYTES: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct PluginRunContextLifecycle {
    contexts: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, String), serde_json::Value>>>,
}

impl PluginRunContextLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_run_context(&self, run_id: &str, key: &str, value: serde_json::Value) {
        let mut contexts = self.contexts.lock().unwrap();
        contexts.insert((run_id.to_string(), key.to_string()), value);
    }

    pub fn get_run_context(&self, run_id: &str, key: &str) -> Option<serde_json::Value> {
        let contexts = self.contexts.lock().unwrap();
        contexts.get(&(run_id.to_string(), key.to_string())).cloned()
    }

    pub fn clear_run_context(&self, run_id: &str) {
        let mut contexts = self.contexts.lock().unwrap();
        contexts.retain(|(r, _), _| r != run_id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTurnJob {
    pub id: String,
    pub session_key: String,
    pub timestamp_ms: u64,
    pub message: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PluginScheduledTurnsManager {
    jobs: std::sync::Arc<std::sync::Mutex<Vec<ScheduledTurnJob>>>,
}

impl PluginScheduledTurnsManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule_turn(
        &self,
        session_key: &str,
        timestamp_ms: u64,
        message: &str,
        tag: Option<&str>,
    ) -> Result<String, String> {
        if timestamp_ms > MAX_DATE_TIMESTAMP_MS {
            return Err(format!(
                "timestamp_ms {} exceeds MAX_DATE_TIMESTAMP_MS {}",
                timestamp_ms, MAX_DATE_TIMESTAMP_MS
            ));
        }

        let job_id = format!("job-{}-{}", session_key, timestamp_ms);
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(ScheduledTurnJob {
            id: job_id.clone(),
            session_key: session_key.to_string(),
            timestamp_ms,
            message: message.to_string(),
            tag: tag.map(|t| t.to_string()),
        });
        Ok(job_id)
    }

    pub fn unschedule_by_tag(&self, session_key: &str, tag: &str) -> usize {
        let mut jobs = self.jobs.lock().unwrap();
        let initial_len = jobs.len();
        jobs.retain(|j| !(j.session_key == session_key && j.tag.as_deref() == Some(tag)));
        initial_len - jobs.len()
    }
}

pub struct PluginSessionActionsManager;

impl PluginSessionActionsManager {
    pub fn execute_action(
        _session_key: &str,
        action: &str,
        scopes: &[String],
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if action == "admin_action" && !scopes.iter().any(|s| s == "operator.admin" || s == "admin") {
            return Err("Forbidden: operator.admin scope required".to_string());
        }
        Ok(serde_json::json!({
            "ok": true,
            "action": action,
            "data": payload
        }))
    }
}

pub struct PluginSessionAttachmentManager;

impl PluginSessionAttachmentManager {
    pub fn send_attachment(
        session_key: &str,
        mime: &str,
        data: &[u8],
        channel_route: &str,
    ) -> Result<String, String> {
        if data.is_empty() {
            return Err("Attachment data cannot be empty".to_string());
        }
        let sniffed_bytes = std::cmp::min(data.len(), FILE_TYPE_SNIFF_MAX_BYTES);
        let attachment_id = format!("att-{}-{}-{}", session_key, mime, sniffed_bytes);
        Ok(format!("sent:{}:to:{}", attachment_id, channel_route))
    }
}

pub struct PluginSessionEntryProjection;

impl PluginSessionEntryProjection {
    pub fn project_extension(
        entry_json: &mut serde_json::Value,
        plugin_id: &str,
        namespace: &str,
        state: serde_json::Value,
    ) {
        if !entry_json.is_object() {
            *entry_json = serde_json::json!({});
        }
        let root = entry_json.as_object_mut().unwrap();
        let plugins = root
            .entry("plugins")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap();
        let plugin_entry = plugins
            .entry(plugin_id)
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .unwrap();
        plugin_entry.insert(namespace.to_string(), state);
    }

    pub fn strip_extension(entry_json: &mut serde_json::Value, plugin_id: &str) {
        if let Some(root) = entry_json.as_object_mut() {
            if let Some(plugins) = root.get_mut("plugins").and_then(|p| p.as_object_mut()) {
                plugins.remove(plugin_id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Agent Prompt Surface Kind Normalization (from src/plugins/agent-prompt-surface-kind.ts)
// ---------------------------------------------------------------------------

pub fn normalize_agent_prompt_surface_kind(surface: &str) -> String {
    if surface.eq_ignore_ascii_case("pi_main") {
        "openclaw_main".to_string()
    } else {
        surface.to_string()
    }
}

pub fn is_openclaw_main_prompt_surface(surface: &str) -> bool {
    normalize_agent_prompt_surface_kind(surface) == "openclaw_main"
}

// ---------------------------------------------------------------------------
// Agent Executor Controller Deprecation Guard (from src/plugins/agent-executor-controller.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCapabilityRemovedError {
    pub capability: String,
    pub message: String,
}

impl std::fmt::Display for PluginCapabilityRemovedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.capability, self.message)
    }
}

impl std::error::Error for PluginCapabilityRemovedError {}

pub fn resolve_agent_executor_controller(plugin_id: &str) -> Result<(), PluginCapabilityRemovedError> {
    Err(PluginCapabilityRemovedError {
        capability: "api.registerAgentExecutorController".to_string(),
        message: format!(
            "Executor controller \"{}\" is unavailable. Retire or migrate its saved AgentsAPI sessions with the previous host before switching to an externally managed self_hosted environment.",
            plugin_id
        ),
    })
}

// ---------------------------------------------------------------------------
// Agent Tool Result Middleware Pipeline (from src/plugins/agent-tool-result-middleware.ts & loader.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentToolResultMiddlewareRuntime {
    OpenClaw,
    Codex,
    AgentsApi,
}

impl AgentToolResultMiddlewareRuntime {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "openclaw" => Some(Self::OpenClaw),
            "codex" => Some(Self::Codex),
            "agentsapi" => Some(Self::AgentsApi),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenClaw => "openclaw",
            Self::Codex => "codex",
            Self::AgentsApi => "agentsapi",
        }
    }
}

pub fn normalize_agent_tool_result_middleware_runtimes(runtimes: &[String]) -> Vec<AgentToolResultMiddlewareRuntime> {
    let mut out = Vec::new();
    for r in runtimes {
        if let Some(parsed) = AgentToolResultMiddlewareRuntime::parse(r) {
            if !out.contains(&parsed) {
                out.push(parsed);
            }
        }
    }
    out
}

#[derive(Clone)]
pub struct AgentToolResultMiddlewareRegistration {
    pub plugin_id: String,
    pub runtimes: Vec<AgentToolResultMiddlewareRuntime>,
    pub tool_names: Option<Vec<String>>,
    pub transform: Arc<dyn Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync>,
}

#[derive(Default, Clone)]
pub struct AgentToolResultMiddlewarePipeline {
    registrations: Arc<Mutex<Vec<AgentToolResultMiddlewareRegistration>>>,
}

impl AgentToolResultMiddlewarePipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, reg: AgentToolResultMiddlewareRegistration) {
        let mut list = self.registrations.lock().unwrap();
        list.push(reg);
    }

    pub fn apply(
        &self,
        runtime: AgentToolResultMiddlewareRuntime,
        tool_name: &str,
        raw_result: serde_json::Value,
    ) -> serde_json::Value {
        let list = self.registrations.lock().unwrap();
        let mut current = raw_result;
        for reg in list.iter() {
            if !reg.runtimes.contains(&runtime) {
                continue;
            }
            if let Some(tools) = &reg.tool_names {
                if !tools.iter().any(|t| t.eq_ignore_ascii_case(tool_name)) {
                    continue;
                }
            }
            current = (reg.transform)(tool_name, &current);
        }
        current
    }
}

// ---------------------------------------------------------------------------
// Before Agent Reply Hook Pipeline & Silent Reply (from src/plugins/before-agent-reply.ts)
// ---------------------------------------------------------------------------

pub const SILENT_REPLY_TOKEN: &str = "NO_REPLY";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginReplyPayload {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

pub fn build_handled_before_agent_reply_payloads(
    reply: Option<PluginReplyPayload>,
) -> Vec<PluginReplyPayload> {
    vec![reply.unwrap_or_else(|| PluginReplyPayload {
        text: SILENT_REPLY_TOKEN.to_string(),
        metadata: None,
    })]
}





