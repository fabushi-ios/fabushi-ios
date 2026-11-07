use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI Backend Errors & Warnings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliBackendAuthProfilePreparationError {
    pub message: String,
}

impl std::fmt::Display for CliBackendAuthProfilePreparationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CliBackendAuthProfilePreparationError: {}", self.message)
    }
}

impl std::error::Error for CliBackendAuthProfilePreparationError {}

pub fn warn_on_missing_cli_backend_model_provider(
    backend_id: &str,
    model_provider: Option<&str>,
) -> Option<String> {
    match model_provider {
        Some(mp) if !mp.trim().is_empty() => None,
        _ => Some(format!(
            "cli backend \"{}\" is missing modelProvider; provider-to-runtime binding will be skipped. Set modelProvider to the canonical model provider id to enable this binding.",
            backend_id.trim()
        )),
    }
}

// ---------------------------------------------------------------------------
// CLI Backend Types & Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliBackendConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_output_mode")]
    pub output: String, // "json" | "text" | "jsonl"
    #[serde(default = "default_input_mode")]
    pub input: String, // "arg" | "stdin"
    pub max_prompt_arg_chars: Option<usize>,
    pub model_arg: Option<String>,
    #[serde(default)]
    pub model_aliases: HashMap<String, String>,
    #[serde(default)]
    pub session_args: Vec<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    #[serde(default = "default_session_mode")]
    pub session_mode: String, // "always" | "existing" | "none"
}

fn default_output_mode() -> String {
    "json".to_string()
}
fn default_input_mode() -> String {
    "arg".to_string()
}
fn default_session_mode() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliBackendPluginDescriptor {
    pub id: String,
    pub model_provider: Option<String>,
    pub config: CliBackendConfig,
    pub subscription_auth_dispatch: bool,
    pub plugin_id: String,
    pub built_with_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliBackendMetadata {
    pub id: String,
    pub model_provider: Option<String>,
    pub subscription_auth_dispatch: bool,
    pub plugin_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliModelBinding {
    pub provider: String,
    pub runtime: String,
}

// ---------------------------------------------------------------------------
// CLI Backend Runtime Registry
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct CliBackendRuntimeRegistry {
    backends: Mutex<HashMap<String, CliBackendPluginDescriptor>>,
    active_owners: Mutex<HashSet<String>>, // active plugin_ids
}

impl CliBackendRuntimeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, descriptor: CliBackendPluginDescriptor) {
        let mut b = self.backends.lock().unwrap();
        let mut o = self.active_owners.lock().unwrap();
        o.insert(descriptor.plugin_id.clone());
        b.insert(descriptor.id.to_ascii_lowercase(), descriptor);
    }

    pub fn unregister_owner(&self, plugin_id: &str) {
        let mut o = self.active_owners.lock().unwrap();
        o.remove(plugin_id);
    }

    pub fn is_cli_provider(&self, provider_or_runtime: &str) -> bool {
        let lower = provider_or_runtime.trim().to_ascii_lowercase();
        let b = self.backends.lock().unwrap();
        b.contains_key(&lower)
    }

    pub fn resolve_canonical_provider(&self, runtime_id: &str) -> Option<String> {
        let lower = runtime_id.trim().to_ascii_lowercase();
        let b = self.backends.lock().unwrap();
        b.get(&lower).and_then(|desc| desc.model_provider.clone())
    }

    pub fn list_bindings(&self) -> Vec<CliModelBinding> {
        let b = self.backends.lock().unwrap();
        let mut bindings = Vec::new();
        for desc in b.values() {
            if let Some(ref mp) = desc.model_provider {
                bindings.push(CliModelBinding {
                    provider: mp.clone(),
                    runtime: desc.id.clone(),
                });
            }
        }
        bindings.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.runtime.cmp(&b.runtime)));
        bindings
    }

    pub fn resolve_config(&self, runtime_id: &str) -> Result<CliBackendConfig, String> {
        let lower = runtime_id.trim().to_ascii_lowercase();
        let b = self.backends.lock().unwrap();
        let desc = b.get(&lower).ok_or_else(|| format!("Unknown CLI runtime '{runtime_id}'"))?;
        let o = self.active_owners.lock().unwrap();
        if !o.contains(&desc.plugin_id) {
            return Err("Plugin CLI backend instance was retired or disabled".to_string());
        }
        Ok(desc.config.clone())
    }

    pub fn resolve_model_id(&self, runtime_id: &str, model_id: &str) -> Result<String, String> {
        let lower = runtime_id.trim().to_ascii_lowercase();
        let b = self.backends.lock().unwrap();
        let desc = b.get(&lower).ok_or_else(|| format!("Unknown CLI runtime '{runtime_id}'"))?;
        let o = self.active_owners.lock().unwrap();
        if !o.contains(&desc.plugin_id) {
            return Err("Plugin CLI backend instance was retired or disabled".to_string());
        }
        if let Some(alias) = desc.config.model_aliases.get(model_id) {
            Ok(alias.clone())
        } else if let Some(ref mp) = desc.model_provider {
            Ok(format!("{mp}:{model_id}"))
        } else {
            Ok(model_id.to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// CLI Gateway Nodes Runtime
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliGatewayNodeEntry {
    pub node_id: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliNodeInvokeParams {
    pub node_id: String,
    pub command: String,
    pub params: Option<serde_json::Value>,
    pub timeout_ms: Option<u64>,
    pub idempotency_key: Option<String>,
    pub session_key: Option<String>,
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliNodeInvokeResult {
    pub ok: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

pub struct CliGatewayNodesRuntime;
impl CliGatewayNodesRuntime {
    pub fn filter_nodes(
        nodes: Vec<CliGatewayNodeEntry>,
        connected_only: bool,
    ) -> Vec<CliGatewayNodeEntry> {
        if connected_only {
            nodes.into_iter().filter(|n| n.connected).collect()
        } else {
            nodes
        }
    }

    pub fn prepare_node_invoke_request(
        mut params: CliNodeInvokeParams,
        is_bundled_or_official: bool,
    ) -> CliNodeInvokeParams {
        if params.idempotency_key.is_none() || params.idempotency_key.as_deref() == Some("") {
            params.idempotency_key = Some(Uuid::new_v4().to_string());
        }
        if !is_bundled_or_official {
            params.scopes = None;
        }
        params
    }

    pub fn open_duplex() -> Result<(), String> {
        Err("Node duplex is unavailable in the CLI; run this plugin inside the Gateway.".to_string())
    }
}

// ---------------------------------------------------------------------------
// CLI Command Descriptors & Registration Policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliRootCommandDescriptor {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub machine_output: bool,
}

pub struct CliCommandRegistry;
impl CliCommandRegistry {
    pub fn collect_unique_command_descriptors(
        groups: Vec<Vec<CliRootCommandDescriptor>>,
    ) -> Vec<CliRootCommandDescriptor> {
        let mut map: HashMap<String, CliRootCommandDescriptor> = HashMap::new();
        for group in groups {
            for desc in group {
                let name = desc.name.trim().to_string();
                if name.is_empty() {
                    continue;
                }
                map.entry(name).or_insert(desc);
            }
        }
        let mut descriptors: Vec<CliRootCommandDescriptor> = map.into_values().collect();
        descriptors.sort_by(|a, b| a.name.cmp(&b.name));
        descriptors
    }

    pub fn can_register_lazily(
        command_names: &[String],
        placeholders: &[CliRootCommandDescriptor],
    ) -> bool {
        if placeholders.is_empty() {
            return false;
        }
        let placeholder_names: HashSet<&str> = placeholders.iter().map(|p| p.name.as_str()).collect();
        command_names.iter().all(|c| placeholder_names.contains(c.as_str()))
    }
}
