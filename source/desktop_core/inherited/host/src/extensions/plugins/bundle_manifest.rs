use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CODEX_BUNDLE_MANIFEST_RELATIVE_PATH: &str = ".codex-plugin/plugin.json";
pub const CLAUDE_BUNDLE_MANIFEST_RELATIVE_PATH: &str = ".claude-plugin/plugin.json";
pub const CURSOR_BUNDLE_MANIFEST_RELATIVE_PATH: &str = ".cursor-plugin/plugin.json";
pub const AGENT_BUNDLE_MANIFEST_RELATIVE_PATH: &str = "plugin.json";

pub const DEFAULT_MAX_ARCHIVE_BYTES_ZIP: u64 = 50 * 1024 * 1024; // 50MB
pub const DEFAULT_MAX_EXTRACTED_BYTES: u64 = 250 * 1024 * 1024; // 250MB
pub const DEFAULT_MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginBundleFormat {
    Codex,
    Claude,
    Cursor,
    Agent,
}

impl PluginBundleFormat {
    pub fn manifest_relative_path(&self) -> &'static str {
        match self {
            Self::Codex => CODEX_BUNDLE_MANIFEST_RELATIVE_PATH,
            Self::Claude => CLAUDE_BUNDLE_MANIFEST_RELATIVE_PATH,
            Self::Cursor => CURSOR_BUNDLE_MANIFEST_RELATIVE_PATH,
            Self::Agent => AGENT_BUNDLE_MANIFEST_RELATIVE_PATH,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClawHubPluginSpec {
    pub scope: Option<String>,
    pub name: String,
    pub version: String,
}

pub fn parse_clawhub_plugin_spec(input: &str) -> Result<ClawHubPluginSpec, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Plugin spec cannot be empty".to_string());
    }

    let (package_part, version) = if let Some(idx) = trimmed.rfind('@') {
        if idx > 0 {
            // Version specified after '@'
            let pkg = &trimmed[..idx];
            let ver = &trimmed[idx + 1..];
            if ver.is_empty() {
                return Err("Empty version in plugin spec".to_string());
            }
            (pkg, ver.to_string())
        } else {
            // Leading '@' indicating scope, e.g. @scope/name
            (trimmed, "latest".to_string())
        }
    } else {
        (trimmed, "latest".to_string())
    };

    if package_part.starts_with('@') {
        let parts: Vec<&str> = package_part.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].len() <= 1 || parts[1].is_empty() {
            return Err(format!("Invalid scoped package name: '{package_part}'"));
        }
        let scope = parts[0][1..].to_string();
        let name = parts[1].to_string();
        Ok(ClawHubPluginSpec {
            scope: Some(scope),
            name,
            version,
        })
    } else {
        Ok(ClawHubPluginSpec {
            scope: None,
            name: package_part.to_string(),
            version,
        })
    }
}

pub fn verify_archive_limits(
    archive_bytes: u64,
    extracted_bytes: u64,
    entry_count: usize,
) -> Result<(), String> {
    if archive_bytes > DEFAULT_MAX_ARCHIVE_BYTES_ZIP {
        return Err(format!(
            "Archive size {archive_bytes} exceeds maximum allowed {DEFAULT_MAX_ARCHIVE_BYTES_ZIP}"
        ));
    }
    if extracted_bytes > DEFAULT_MAX_EXTRACTED_BYTES {
        return Err(format!(
            "Extracted size {extracted_bytes} exceeds maximum allowed {DEFAULT_MAX_EXTRACTED_BYTES}"
        ));
    }
    if entry_count > DEFAULT_MAX_ENTRIES {
        return Err(format!(
            "Entry count {entry_count} exceeds maximum allowed {DEFAULT_MAX_ENTRIES}"
        ));
    }
    Ok(())
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn verify_sha256_integrity(data: &[u8], expected_digest: &str) -> Result<(), String> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();

    let clean_expected = expected_digest.trim();
    let expected_hex = if let Some(stripped) = clean_expected.strip_prefix("sha256-") {
        // Could be base64
        if let Ok(decoded) = STANDARD.decode(stripped) {
            encode_hex(&decoded)
        } else {
            stripped.to_ascii_lowercase()
        }
    } else {
        clean_expected.to_ascii_lowercase()
    };

    let actual_hex = encode_hex(&result);
    if actual_hex.eq_ignore_ascii_case(&expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "Integrity mismatch: expected '{expected_hex}', actual '{actual_hex}'"
        ))
    }
}

// Package manifest validation conforming to package-manifest.contract.test.ts

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenClawMetadata {
    pub extensions: Vec<String>,
    #[serde(default)]
    pub runtime_extensions: Vec<String>,
    #[serde(default)]
    pub setup_entry: Option<String>,
    #[serde(default)]
    pub runtime_setup_entry: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageExtensionEntriesResult {
    pub status: String,
    pub entries: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn resolve_package_extension_entries(raw_manifest: &serde_json::Value) -> PackageExtensionEntriesResult {
    let openclaw = match raw_manifest.get("openclaw") {
        Some(val) if val.is_object() => val,
        Some(_) => {
            return PackageExtensionEntriesResult {
                status: "invalid".to_string(),
                entries: vec![],
                error: Some("package.json openclaw must be an object".to_string()),
            };
        }
        None => {
            return PackageExtensionEntriesResult {
                status: "invalid".to_string(),
                entries: vec![],
                error: Some("missing openclaw object in package.json".to_string()),
            };
        }
    };

    let extensions = match openclaw.get("extensions") {
        Some(val) if val.is_array() => val.as_array().unwrap(),
        Some(_) => {
            return PackageExtensionEntriesResult {
                status: "invalid".to_string(),
                entries: vec![],
                error: Some("package.json openclaw.extensions must be an array".to_string()),
            };
        }
        None => {
            return PackageExtensionEntriesResult {
                status: "ok".to_string(),
                entries: vec![],
                error: None,
            };
        }
    };

    let mut result = Vec::new();
    for (idx, item) in extensions.iter().enumerate() {
        if let Some(s) = item.as_str() {
            if s.trim().is_empty() {
                return PackageExtensionEntriesResult {
                    status: "invalid".to_string(),
                    entries: vec![],
                    error: Some(format!("package.json openclaw.extensions[{idx}] must be a non-empty string")),
                };
            }
            result.push(s.to_string());
        } else {
            return PackageExtensionEntriesResult {
                status: "invalid".to_string(),
                entries: vec![],
                error: Some(format!("package.json openclaw.extensions[{idx}] must be a string")),
            };
        }
    }

    PackageExtensionEntriesResult {
        status: "ok".to_string(),
        entries: result,
        error: None,
    }
}

pub fn validate_package_extension_entries_for_install(
    openclaw: &OpenClawMetadata,
) -> Result<(), String> {
    if !openclaw.runtime_extensions.is_empty()
        && openclaw.runtime_extensions.len() != openclaw.extensions.len()
    {
        return Err(format!(
            "package.json openclaw.runtimeExtensions length ({}) must match openclaw.extensions length ({})",
            openclaw.runtime_extensions.len(),
            openclaw.extensions.len()
        ));
    }

    if openclaw.runtime_setup_entry.is_some() && openclaw.setup_entry.is_none() {
        return Err("package.json openclaw.runtimeSetupEntry requires openclaw.setupEntry".to_string());
    }

    Ok(())
}

// Tool manifest contracts & fixture execution (from plugin-tool-contracts.test.ts & plugin-testing-harness.contract.test.ts)

pub fn validate_plugin_manifest_tool_contracts(
    plugin_id: &str,
    declared_tools: &[String],
    registered_tool_names: &[String],
) -> Result<(), String> {
    if registered_tool_names.is_empty() {
        return Ok(());
    }

    if declared_tools.is_empty() {
        return Err(format!(
            "{}: registers agent tools but has no contracts.tools",
            plugin_id
        ));
    }

    let declared_set: std::collections::HashSet<&str> =
        declared_tools.iter().map(|s| s.as_str()).collect();
    let mut missing = Vec::new();
    for name in registered_tool_names {
        if !declared_set.contains(name.as_str()) {
            missing.push(name.clone());
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        missing.sort();
        Err(format!(
            "{}: missing contracts.tools for {}",
            plugin_id,
            missing.join(", ")
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualToolCallOutcome {
    pub text: String,
    pub home: String,
    pub message: String,
}

pub struct VirtualTestPluginHarness;

impl VirtualTestPluginHarness {
    pub fn execute_fixture_echo_tool(
        prefix: &str,
        home: &str,
        _tool_call_id: &str,
        message: &str,
    ) -> VirtualToolCallOutcome {
        VirtualToolCallOutcome {
            text: format!("{}:{}", prefix, message),
            home: home.to_string(),
            message: message.to_string(),
        }
    }
}

// Bundled Plugin Roots, Host Hook Fixtures, and Subpath Bundle Validator
// (from host-hook-fixture.ts, test-helpers/host-hook-ui-projection.ts,
//  test-helpers/bundled-plugin-roots.ts, and plugin-sdk-index.bundle.test.ts)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostHookUiDescriptor {
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub surface: String,
    pub label: String,
    pub required_scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostHookUiProjection {
    pub ok: bool,
    pub generation: usize,
    pub methods: Vec<String>,
    pub control_ui_tabs: Vec<String>,
    pub control_ui_widget_kinds: Vec<(String, String, String)>,
    pub descriptors: Vec<HostHookUiDescriptor>,
}

impl HostHookUiProjection {
    pub fn build(generation: usize) -> Self {
        Self {
            ok: true,
            generation,
            methods: vec![
                "plugins.uiDescriptors".to_string(),
                "plugins.sessionAction".to_string(),
            ],
            control_ui_tabs: Vec::new(),
            control_ui_widget_kinds: vec![
                ("session".to_string(), "session:report".to_string(), "Report".to_string()),
                ("session".to_string(), "session:progress".to_string(), "Session progress".to_string()),
                ("session".to_string(), "session:website".to_string(), "Website".to_string()),
            ],
            descriptors: vec![
                HostHookUiDescriptor {
                    id: "admin-panel".to_string(),
                    plugin_id: "host-hook-fixture".to_string(),
                    plugin_name: "Host Hook Fixture".to_string(),
                    surface: "settings".to_string(),
                    label: "Admin panel".to_string(),
                    required_scopes: vec!["operator.admin".to_string()],
                },
                HostHookUiDescriptor {
                    id: "approval-panel".to_string(),
                    plugin_id: "host-hook-fixture".to_string(),
                    plugin_name: "Host Hook Fixture".to_string(),
                    surface: "session".to_string(),
                    label: "Approval panel".to_string(),
                    required_scopes: Vec::new(),
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostHookFixturePlugin {
    pub session_extensions: Vec<String>,
    pub tool_metadata_names: Vec<String>,
    pub control_ui_descriptors: Vec<String>,
    pub lifecycle_hooks: Vec<String>,
    pub agent_event_subscriptions: Vec<String>,
    pub session_scheduler_jobs: Vec<String>,
    pub command_names: Vec<String>,
}

impl HostHookFixturePlugin {
    pub fn register_fixture_defaults() -> Self {
        Self {
            session_extensions: vec!["workflow".to_string()],
            tool_metadata_names: vec!["approval_fixture_tool".to_string()],
            control_ui_descriptors: vec!["workflow-card".to_string()],
            lifecycle_hooks: vec!["workflow-cleanup".to_string()],
            agent_event_subscriptions: vec!["workflow-events".to_string()],
            session_scheduler_jobs: vec!["workflow-nudge".to_string()],
            command_names: vec!["host-hook-fixture".to_string()],
        }
    }

    pub fn execute_command(&self, args: Option<&str>) -> (String, bool) {
        (format!("fixture:{}", args.unwrap_or("empty")), true)
    }

    pub fn turn_prepare_context() -> &'static str {
        "fixture turn context"
    }

    pub fn heartbeat_prompt_contribution() -> &'static str {
        "fixture heartbeat context"
    }
}

pub struct BundledPluginRootsManager {
    roots: std::collections::HashMap<String, String>,
}

impl BundledPluginRootsManager {
    pub fn new(roots: &[(&str, &str)]) -> Self {
        let mut map = std::collections::HashMap::new();
        for (id, path) in roots {
            map.insert(id.to_string(), path.to_string());
        }
        Self { roots: map }
    }

    pub fn resolve_file(&self, plugin_id: &str, rel_path: &str) -> Result<String, String> {
        let root = self
            .roots
            .get(plugin_id)
            .ok_or_else(|| format!("missing bundled plugin root for {}", plugin_id))?;
        Ok(format!("{}/{}", root.trim_end_matches('/'), rel_path.trim_start_matches('/')))
    }
}

pub struct BundledPluginSubpathValidator;

impl BundledPluginSubpathValidator {
    pub fn never_bundle_dependencies() -> &'static [&'static str] {
        &[
            "@lancedb/lancedb",
            "@matrix-org/matrix-sdk-crypto-nodejs",
            "matrix-js-sdk",
        ]
    }

    pub fn assert_no_bare_matrix_sdk_imports(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let mut offenders = Vec::new();
        for (path, content) in sources {
            if content.contains("from \"matrix-js-sdk\"")
                || content.contains("from 'matrix-js-sdk'")
                || content.contains("require(\"matrix-js-sdk\")")
                || content.contains("require('matrix-js-sdk')")
            {
                offenders.push(path.to_string());
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

// Config Footprint, Extension Package Boundaries, Runtime Dependencies, and Plugin Entry Guardrails
// (from config-footprint-guardrails, extension-package-project-boundaries,
//  extension-runtime-dependencies, and plugin-entry-guardrails contract test suites)

pub struct ConfigFootprintGuardrail;

impl ConfigFootprintGuardrail {
    pub fn assert_no_retired_legacy_paths(schema_paths: &[&str]) -> Result<(), Vec<String>> {
        let retired_patterns = [
            "talk.voiceId",
            "talk.voiceAliases",
            "talk.modelId",
            "talk.outputFormat",
            "talk.apiKey",
            "talk.providers.",
            "agents.defaults.sandbox.perSession",
            "hooks.internal.handlers",
            "channels.telegram.streamMode",
            "channels.slack.streamMode",
            "channels.discord.streamMode",
            "channels.googlechat.streamMode",
        ];

        let mut offenders = Vec::new();
        for path in schema_paths {
            for pattern in &retired_patterns {
                if path.contains(pattern) {
                    offenders.push(path.to_string());
                }
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }

    pub fn assert_canonical_private_network_paths(metadata_paths: &[&str]) -> Result<(), Vec<String>> {
        let mut offenders = Vec::new();
        for path in metadata_paths {
            if path.contains(".allowPrivateNetwork") && !path.contains("dangerouslyAllowPrivateNetwork") {
                offenders.push(path.to_string());
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

pub struct ExtensionPackageBoundaryGuard;

impl ExtensionPackageBoundaryGuard {
    pub fn assert_allowed_cross_package_imports(
        extension_path: &str,
        source: &str,
    ) -> Result<(), Vec<String>> {
        let forbidden_private_entrypoints = [
            "openclaw/plugin-sdk/private/",
            "openclaw/plugin-sdk/internal/",
        ];
        let mut offenders = Vec::new();
        for entry in &forbidden_private_entrypoints {
            if source.contains(entry) {
                offenders.push(format!("{}: imports forbidden private entrypoint '{}'", extension_path, entry));
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

pub struct ExtensionRuntimeDependenciesGuard;

impl ExtensionRuntimeDependenciesGuard {
    pub fn is_whitelisted_optional_peer(extension: &str, module: &str) -> bool {
        match (extension, module) {
            ("extensions/canvas", "@a2ui/markdown-it") => true,
            ("extensions/discord", "@discordjs/opus") => true,
            ("extensions/qa-lab", "crabline") => true,
            ("extensions/browser", "zod") => true,
            ("extensions/whatsapp", "audio-decode") => true,
            ("extensions/memory-lancedb", "apache-arrow") => true,
            _ => false,
        }
    }
}

pub struct PluginEntryGuardrails;

impl PluginEntryGuardrails {
    pub fn assert_no_testing_imports_in_entrypoint(
        entrypoint_file: &str,
        source: &str,
    ) -> Result<(), Vec<String>> {
        let forbidden_patterns = [
            "vitest",
            "openclaw/plugin-sdk/testing",
            "/test-api",
            "__tests__",
            "/test-support",
            ".test.ts",
            ".test.js",
            "test-harness",
            "test-helper",
        ];

        let mut offenders = Vec::new();
        for pattern in &forbidden_patterns {
            if source.contains(pattern) {
                offenders.push(format!("{}: imports test module or harness pattern '{}'", entrypoint_file, pattern));
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

// Plugin SDK Package Contract Guardrails, Runtime API Guards, and Subpaths
// (from plugin-sdk-package-contract-guardrails, plugin-sdk-runtime-api-guardrails,
//  and plugin-sdk-subpaths contract test suites)

pub struct PluginSdkPackageContractGuard;

impl PluginSdkPackageContractGuard {
    pub fn public_plugin_sdk_entrypoints() -> &'static [&'static str] {
        &[
            "account-id",
            "allow-from",
            "channel-contract",
            "channel-core",
            "channel-outbound",
            "channel-pairing",
            "channel-status",
            "config-contracts",
            "core",
            "media-runtime",
            "plugin-entry",
            "reply-payload",
            "runtime",
            "runtime-store",
            "ssrf-runtime",
        ]
    }

    pub fn private_local_only_entrypoints() -> &'static [&'static str] {
        &[
            "browser-maintenance",
            "channel-secret-owner-runtime",
            "channel-secret-tts-runtime",
        ]
    }

    pub fn assert_no_private_entrypoint_in_public_subpaths(
        subpaths: &[&str],
    ) -> Result<(), Vec<String>> {
        let private = Self::private_local_only_entrypoints();
        let mut offenders = Vec::new();
        for s in subpaths {
            for p in private {
                if s.ends_with(p) || s.contains(p) {
                    offenders.push(s.to_string());
                }
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

pub struct PluginSdkRuntimeApiGuardrails;

impl PluginSdkRuntimeApiGuardrails {
    pub fn assert_runtime_api_exports_allowed(
        plugin_id: &str,
        exports: &[&str],
    ) -> Result<(), Vec<String>> {
        let mut offenders = Vec::new();
        for exp in exports {
            if exp.contains("test-api") || exp.contains("vitest") || exp.contains("testing") {
                offenders.push(format!("{}: exports internal test module '{}'", plugin_id, exp));
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

// ---------------------------------------------------------------------------
// Bundled MCP Server & LSP Server Validators (from src/plugins/bundle-mcp.ts & bundle-lsp.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BundledMcpTransport {
    Stdio,
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledMcpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub transport: BundledMcpTransport,
}

pub struct BundledMcpServerValidator;

impl BundledMcpServerValidator {
    pub fn validate(config: &BundledMcpServerConfig) -> Result<(), String> {
        // Name check: 1-64 chars, no path separators or spaces
        if config.name.is_empty()
            || config.name.len() > 64
            || config.name.contains('/')
            || config.name.contains('\\')
            || config.name.contains(' ')
        {
            return Err(format!("Invalid MCP server name: '{}'", config.name));
        }

        // Command check: cannot escape bundle root
        if config.command.contains("../") || config.command.contains("..\\") {
            return Err(format!(
                "MCP server command cannot escape bundle root: '{}'",
                config.command
            ));
        }

        // Arguments check: cannot contain relative escape sequences
        for arg in &config.args {
            if arg.starts_with("../") || arg.contains("/../") {
                return Err(format!(
                    "MCP server argument cannot escape bundle root: '{}'",
                    arg
                ));
            }
        }

        Ok(())
    }
}

pub struct BundledLspServerValidator;

impl BundledLspServerValidator {
    pub fn validate(server_name: &str, language_id: &str, command: &str) -> Result<(), String> {
        if server_name.is_empty() {
            return Err("LSP server name cannot be empty".to_string());
        }
        if language_id.is_empty() {
            return Err("LSP language ID cannot be empty".to_string());
        }
        if command.is_empty() || command.contains("../") {
            return Err(format!("Invalid LSP server command: '{}'", command));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bundle Capability Support & Command Scopes (from src/plugins/bundle-capability-support.ts & bundle-commands)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct BundleCapabilitySupport {
    pub has_mcp: bool,
    pub has_lsp: bool,
    pub has_tools: bool,
    pub has_skills: bool,
    pub has_channels: bool,
}

impl BundleCapabilitySupport {
    pub fn from_manifest(manifest: &serde_json::Value) -> Self {
        let has_mcp = manifest.get("mcpServers").map(|v| v.is_object() && !v.as_object().unwrap().is_empty()).unwrap_or(false);
        let has_lsp = manifest.get("lspServers").map(|v| v.is_object() && !v.as_object().unwrap().is_empty()).unwrap_or(false);
        let has_tools = manifest.get("tools").map(|v| v.is_array() && !v.as_array().unwrap().is_empty()).unwrap_or(false);
        let has_skills = manifest.get("skills").map(|v| v.is_array() && !v.as_array().unwrap().is_empty()).unwrap_or(false);
        let has_channels = manifest.get("channels").map(|v| v.is_array() && !v.as_array().unwrap().is_empty()).unwrap_or(false);

        Self {
            has_mcp,
            has_lsp,
            has_tools,
            has_skills,
            has_channels,
        }
    }
}

pub struct BundleCommandScopeValidator;

impl BundleCommandScopeValidator {
    pub fn assert_command_scope_authorized(
        command_name: &str,
        required_scope: Option<&str>,
        user_scopes: &[&str],
    ) -> Result<(), String> {
        if let Some(scope) = required_scope {
            if !user_scopes.contains(&scope) && !user_scopes.contains(&"operator.admin") {
                return Err(format!(
                    "Command '{}' requires scope '{}', but caller has scopes: {:?}",
                    command_name, scope, user_scopes
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bundled Plugin Manifest Model & Validation (OCRM-RESP-PLUGINS-BUNDLE-MANIFEST-001)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
}

impl PluginManifest {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            ..Default::default()
        }
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.name.trim().is_empty() {
            errors.push("Plugin name cannot be empty".to_string());
        }
        if self.version.trim().is_empty() {
            errors.push("Plugin version cannot be empty".to_string());
        }
        if let Some(entry) = &self.entry {
            if entry.contains("../") || entry.contains("..\\") {
                errors.push(format!("Entry point cannot traverse outside plugin root: '{entry}'"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}





