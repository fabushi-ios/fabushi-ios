use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ActivationTrigger {
    Command(String),
    Provider(String),
    AgentHarness(String),
    Channel(String),
    Route(String),
    Capability(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifestRecord {
    pub plugin_id: String,
    pub commands: Vec<String>,
    pub providers: Vec<String>,
    pub channels: Vec<String>,
    pub capabilities: Vec<String>,
    pub is_bundled: bool,
    pub is_trusted: bool,
}

impl PluginManifestRecord {
    pub fn matches_trigger(&self, trigger: &ActivationTrigger) -> bool {
        match trigger {
            ActivationTrigger::Command(cmd) => {
                self.commands.iter().any(|c| c.eq_ignore_ascii_case(cmd))
            }
            ActivationTrigger::Provider(prov) => {
                self.providers.iter().any(|p| p.eq_ignore_ascii_case(prov))
            }
            ActivationTrigger::AgentHarness(harness) => {
                self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(harness))
            }
            ActivationTrigger::Channel(chan) => {
                self.channels.iter().any(|c| c.eq_ignore_ascii_case(chan))
            }
            ActivationTrigger::Route(route) => {
                self.channels.iter().any(|c| c.eq_ignore_ascii_case(route))
            }
            ActivationTrigger::Capability(cap) => {
                self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(cap))
            }
        }
    }

    pub fn is_activatable(&self) -> bool {
        self.is_bundled || self.is_trusted
    }
}

#[derive(Clone, Default)]
pub struct ActivationPlanner {
    plugins: HashMap<String, PluginManifestRecord>,
}

impl ActivationPlanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: PluginManifestRecord) {
        self.plugins.insert(manifest.plugin_id.clone(), manifest);
    }

    pub fn plan_activation(&self, triggers: &[ActivationTrigger]) -> Vec<String> {
        let mut activated_plugins = Vec::new();
        let mut seen = HashSet::new();

        for trigger in triggers {
            for plugin in self.plugins.values() {
                if plugin.matches_trigger(trigger) && plugin.is_activatable() {
                    if seen.insert(plugin.plugin_id.clone()) {
                        activated_plugins.push(plugin.plugin_id.clone());
                    }
                }
            }
        }

        activated_plugins.sort();
        activated_plugins
    }

    pub fn plan_activation_scoped(
        &self,
        triggers: &[ActivationTrigger],
        only_plugin_ids: Option<&[String]>,
    ) -> Vec<String> {
        if let Some(scope) = only_plugin_ids {
            if scope.is_empty() {
                return Vec::new();
            }
        }
        let mut activated = Vec::new();
        let mut seen = HashSet::new();

        let allowed_scope: Option<HashSet<&str>> =
            only_plugin_ids.map(|s| s.iter().map(|id| id.as_str()).collect());

        for trigger in triggers {
            for plugin in self.plugins.values() {
                if let Some(scope) = &allowed_scope {
                    if !scope.contains(plugin.plugin_id.as_str()) {
                        continue;
                    }
                }
                if plugin.matches_trigger(trigger) && plugin.is_activatable() {
                    if seen.insert(plugin.plugin_id.clone()) {
                        activated.push(plugin.plugin_id.clone());
                    }
                }
            }
        }

        activated.sort();
        activated
    }
}

// ---------------------------------------------------------------------------
// Activation Context & Source Configuration (from src/plugins/activation-source-config.ts & activation-context.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginActivationSourceResolver;

impl PluginActivationSourceResolver {
    pub fn resolve_source_config(
        config: Option<&serde_json::Value>,
        activation_source_config: Option<&serde_json::Value>,
        runtime_config_capture: Option<&serde_json::Value>,
    ) -> serde_json::Value {
        if let Some(source) = activation_source_config {
            return source.clone();
        }
        if let Some(captured) = runtime_config_capture {
            return captured.clone();
        }
        config.cloned().unwrap_or_else(|| serde_json::json!({}))
    }
}

// ---------------------------------------------------------------------------
// Active Payload Verification Filter (from src/plugins/active-payload-verification.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallRecordStub {
    pub plugin_id: String,
    pub version: String,
    pub is_operator_managed: bool,
    pub official_npm_spec: Option<String>,
    pub official_clawhub_spec: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ActivePayloadVerification;

impl ActivePayloadVerification {
    pub fn filter_records_to_active(
        records: &[PluginInstallRecordStub],
        disabled_ids: &HashSet<String>,
        denied_ids: &HashSet<String>,
        source_bundled_ids: &HashSet<String>,
    ) -> Vec<PluginInstallRecordStub> {
        let mut filtered = Vec::new();

        for record in records {
            let id = &record.plugin_id;
            // 1. Deny list drops record immediately
            if denied_ids.contains(id) {
                continue;
            }
            // 2. Source-bundled or operator-managed are excluded from payload verification
            if source_bundled_ids.contains(id) || record.is_operator_managed {
                continue;
            }
            // 3. Disabled plugins: retained only if trusted official npm/clawhub linked
            if disabled_ids.contains(id) {
                if record.official_npm_spec.is_some() || record.official_clawhub_spec.is_some() {
                    filtered.push(record.clone());
                }
                continue;
            }
            // 4. Enabled plugins are retained
            filtered.push(record.clone());
        }

        filtered.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        filtered
    }
}

// ---------------------------------------------------------------------------
// Active Runtime Registry & Compatibility Resolution (from src/plugins/active-runtime-registry.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRegistryIdentity {
    pub cache_key: String,
    pub workspace_dir: Option<String>,
    pub loaded_plugin_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ActiveRuntimeRegistryResolver;

impl ActiveRuntimeRegistryResolver {
    pub fn is_compatible(
        active: Option<&ActiveRegistryIdentity>,
        requested_key: &str,
        requested_workspace_dir: Option<&str>,
    ) -> bool {
        let active = match active {
            Some(a) => a,
            None => return false,
        };

        // Workspace directory must match if specified
        if active.workspace_dir.as_deref() != requested_workspace_dir {
            return false;
        }

        active.cache_key == requested_key
    }

    pub fn list_runtime_plugin_ids(identity: &ActiveRegistryIdentity) -> Vec<String> {
        let mut ids = identity.loaded_plugin_ids.clone();
        ids.sort();
        ids.dedup();
        ids
    }
}

// Plugin shape inspection & compatibility matrix (from src/plugins/inspect-shape.ts)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginInspectShape {
    HookOnly,
    PlainCapability,
    HybridCapability,
    NonCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityMode {
    None,
    Plain,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilityEntry {
    pub kind: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginShapeSummary {
    pub shape: PluginInspectShape,
    pub capability_mode: CapabilityMode,
    pub capability_count: usize,
    pub capabilities: Vec<PluginCapabilityEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct PluginShapeParams {
    pub plugin_id: String,
    pub commands: Vec<String>,
    pub cli_commands: Vec<String>,
    pub services: Vec<String>,
    pub http_routes_count: usize,
    pub has_hooks: bool,
    pub has_tools: bool,
    pub has_gateway_methods: bool,
    pub capabilities: Vec<PluginCapabilityEntry>,
}

pub fn build_plugin_shape_summary(params: &PluginShapeParams) -> PluginShapeSummary {
    let non_empty_caps: Vec<PluginCapabilityEntry> = params
        .capabilities
        .iter()
        .filter(|c| !c.ids.is_empty())
        .cloned()
        .collect();
    let capability_count = non_empty_caps.len();

    let shape = if capability_count > 1 {
        PluginInspectShape::HybridCapability
    } else if capability_count == 1 {
        PluginInspectShape::PlainCapability
    } else {
        let has_only_hooks = params.commands.is_empty()
            && params.cli_commands.is_empty()
            && params.services.is_empty()
            && params.http_routes_count == 0
            && params.has_hooks
            && !params.has_tools
            && !params.has_gateway_methods;
        if has_only_hooks {
            PluginInspectShape::HookOnly
        } else {
            PluginInspectShape::NonCapability
        }
    };

    let capability_mode = match capability_count {
        0 => CapabilityMode::None,
        1 => CapabilityMode::Plain,
        _ => CapabilityMode::Hybrid,
    };

    PluginShapeSummary {
        shape,
        capability_mode,
        capability_count,
        capabilities: non_empty_caps,
    }
}
