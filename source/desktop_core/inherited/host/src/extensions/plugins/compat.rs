use std::collections::HashSet;
use std::sync::OnceLock;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCompatStatus {
    Active,
    Deprecated,
    RemovalPending,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCompatOwner {
    AgentRuntime,
    Channel,
    Config,
    Core,
    PluginExecution,
    Provider,
    Sdk,
    Setup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCompatRecord {
    pub code: String,
    pub status: PluginCompatStatus,
    pub owner: PluginCompatOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub introduced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_starts: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal_gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    pub docs_path: String,
    pub surfaces: Vec<String>,
    pub diagnostics: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_note: Option<String>,
}

static COMPAT_RECORDS: OnceLock<Vec<PluginCompatRecord>> = OnceLock::new();

pub fn list_plugin_compat_records() -> &'static [PluginCompatRecord] {
    COMPAT_RECORDS.get_or_init(|| {
        let raw = include_str!("plugin_compat_records.json");
        serde_json::from_str(raw).expect("plugin_compat_records.json must be valid JSON")
    })
}

pub fn get_compat_record(code: &str) -> Option<&'static PluginCompatRecord> {
    list_plugin_compat_records().iter().find(|r| r.code == code)
}

fn is_valid_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    parts[0].len() == 4
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1].len() == 2
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[2].len() == 2
        && parts[2].chars().all(|c| c.is_ascii_digit())
}

pub fn validate_record(record: &PluginCompatRecord) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if !record.docs_path.starts_with('/') {
        errors.push(format!("{}: docs_path must start with '/'", record.code));
    }

    let removal_date_pending_codes: HashSet<&str> = [
        "plugin-sdk-tool-plugin-public-demotion",
        "agent-harness-sdk-alias",
        "plugin-sdk-shipped-channel-setup-exports",
    ]
    .into_iter()
    .collect();

    if record.status == PluginCompatStatus::Deprecated {
        match &record.deprecated {
            Some(d) if is_valid_date(d) => {}
            _ => errors.push(format!("{}: invalid deprecated date", record.code)),
        }
        match &record.warning_starts {
            Some(w) if is_valid_date(w) => {}
            _ => errors.push(format!("{}: invalid warning_starts date", record.code)),
        }

        if let Some(gate) = &record.removal_gate {
            if gate != "stable-deprecation-period" {
                errors.push(format!("{}: removal_gate must be 'stable-deprecation-period'", record.code));
            }
            if record.remove_after.is_some() {
                errors.push(format!("{}: remove_after must be empty when removal_gate is set", record.code));
            }
        } else if removal_date_pending_codes.contains(record.code.as_str()) {
            if record.remove_after.is_some() {
                errors.push(format!("{}: remove_after must be empty for pending codes", record.code));
            }
        } else {
            match &record.remove_after {
                Some(r) if is_valid_date(r) => {}
                _ => errors.push(format!("{}: remove_after date must be present and valid", record.code)),
            }
        }

        match &record.replacement {
            Some(rep) if !rep.trim().is_empty() => {}
            _ => errors.push(format!("{}: replacement must be non-empty", record.code)),
        }
    }

    if record.surfaces.is_empty() || record.surfaces.iter().any(|s| s.trim().is_empty()) {
        errors.push(format!("{}: surfaces must contain non-empty strings", record.code));
    }

    if record.diagnostics.is_empty() || record.diagnostics.iter().any(|d| d.trim().is_empty()) {
        errors.push(format!("{}: diagnostics must contain non-empty strings", record.code));
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn validate_all_records() -> Result<(), Vec<String>> {
    let mut all_errors = Vec::new();
    for record in list_plugin_compat_records() {
        if let Err(errors) = validate_record(record) {
            all_errors.extend(errors);
        }
    }
    if all_errors.is_empty() {
        Ok(())
    } else {
        Err(all_errors)
    }
}

// --- Conversation Read Tools ---

const HOST_RESTRICTED_CONVERSATION_READ_TOOLS: &[&str] = &["feishu:feishu_chat"];

pub fn normalize_contract_name(value: &str) -> String {
    value.trim().to_lowercase()
}

pub fn is_host_restricted_conversation_read_tool(plugin_id: &str, tool_name: &str) -> bool {
    let key = format!("{}:{}", normalize_contract_name(plugin_id), normalize_contract_name(tool_name));
    HOST_RESTRICTED_CONVERSATION_READ_TOOLS.contains(&key.as_str())
}

pub fn registration_includes_host_restricted_conversation_read_tool(
    plugin_id: &str,
    names: &[String],
    declared_names: &[String],
) -> bool {
    let producible_names = if !names.is_empty() {
        names
    } else {
        declared_names
    };
    for tool_name in producible_names {
        if is_host_restricted_conversation_read_tool(plugin_id, tool_name) {
            return true;
        }
    }
    false
}

pub fn is_bundled_conversation_read_tool_registration(
    entry_is_bundled: bool,
    manifest_is_bundled: bool,
) -> bool {
    entry_is_bundled && manifest_is_bundled
}

// --- Deprecation Warnings Tracker ---

const SQLITE_REMOVAL_FAMILIES: &[&str] = &[
    "acp-session",
    "agent-end-side-effects",
    "agent-harness-host",
    "agent-harness-session-runtime-ownership",
    "agent-harness-tool-surface",
    "approval-event-publication",
    "approval-session-routing",
    "auth-profile-sync-operations",
    "auth-profiles",
    "channel-conversation-bindings",
    "channel-inbound-envelope",
    "channel-pairing-sync-allowlist",
    "codex-session-context",
    "command-authorization-sync",
    "conversation-bindings",
    "cron-store-path",
    "exec-approvals-sync-read",
    "github-publication",
    "memory-session",
    "memory-session-sync-inventory",
    "memory-status",
    "model-account-connect",
    "model-registry-native-read",
    "native-command-menu",
    "native-session-binding",
    "native-session-deletion",
    "node-host-sync-workspace",
    "openrouter-model-capabilities",
    "plugin-state-keyed-store",
    "provider-auth",
    "provider-selection",
    "provider-sync-configuration",
    "proxy-capture-reader",
    "proxy-capture-runtime",
    "proxy-capture-store",
    "reply-run-start",
    "reply-tool-authority-sync-preparation",
    "reply-visibility",
    "runtime-config",
    "session-catalog-identity",
    "session-catalog-title",
    "session-conversation",
    "session-deletion-transaction-callback",
    "session-entry-freshness",
    "session-input-completion",
    "session-model-override",
    "session-observer",
    "session-persistence",
    "session-store",
    "session-transcript",
    "session-transcript-key",
    "session-transcript-store",
    "session-upstream-links",
    "skill-command-discovery",
    "sqlite-runtime-native-access",
    "tool-surface",
    "tts",
    "watched-sessions-prompt",
    "watched-sessions-sync-harness-context",
    "web-search-providers",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDeprecationWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct PluginSdkDeprecationTracker {
    warned: HashSet<(Option<String>, String, String)>,
}

fn is_valid_plugin_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    let (scope, name) = if let Some(stripped) = id.strip_prefix('@') {
        if let Some((s, n)) = stripped.split_once('/') {
            (Some(s), n)
        } else {
            return false;
        }
    } else {
        (None, id)
    };

    if let Some(s) = scope {
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return false;
        }
    }

    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

impl PluginSdkDeprecationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn warn_deprecation(
        &mut self,
        family: &str,
        method: &str,
        replacement: &str,
        plugin_id: Option<&str>,
        compatibility: Option<&str>,
        code: Option<&str>,
    ) -> Option<PluginDeprecationWarning> {
        let valid_plugin_id = plugin_id.filter(|p| is_valid_plugin_id(p)).map(|p| p.to_string());
        let key = (valid_plugin_id.clone(), family.to_string(), method.to_string());
        if self.warned.contains(&key) {
            return None;
        }
        self.warned.insert(key);

        let removal_info = if SQLITE_REMOVAL_FAMILIES.contains(&family) {
            "Final compatibility day: 2026-12-31"
        } else {
            "Removal requires an announced ordinary stable release window"
        };

        let prefix = match &valid_plugin_id {
            Some(p) => format!("[{p}] "),
            None => String::new(),
        };

        let compat_msg = compatibility.unwrap_or("Synchronous calls retain their return values and commit before returning.");
        let message = format!("{prefix}{method} is deprecated; use {replacement}. {compat_msg} {removal_info}");

        Some(PluginDeprecationWarning {
            code: code.unwrap_or("DEP_SESSION_PERSISTENCE").to_string(),
            message,
        })
    }
}

pub fn warn_mention_inbox_deprecation(
    tracker: &mut PluginSdkDeprecationTracker,
    plugin_id: Option<&str>,
    method: &str,
) -> Option<PluginDeprecationWarning> {
    tracker.warn_deprecation(
        "mention-inbox",
        &format!("mentionInbox.{method}"),
        &format!("mentionInbox.{method}Async"),
        plugin_id,
        Some("Legacy recording queues work after commit; await recordCommittedInputAsync for completion. Synchronous reads, dismissals, and invalidation reject calls with async migration guidance."),
        Some("DEP_SESSION_PERSISTENCE"),
    )
}

pub fn warn_model_account_connect_deprecation(
    tracker: &mut PluginSdkDeprecationTracker,
    plugin_id: Option<&str>,
    method: &str,
) -> Option<PluginDeprecationWarning> {
    tracker.warn_deprecation(
        "model-account-connect",
        &format!("modelAccountConnectService.{method}"),
        &format!("modelAccountConnectService.{method}Async"),
        plugin_id,
        None,
        Some("DEP_SESSION_PERSISTENCE"),
    )
}

pub fn warn_session_transcript_preparation_deprecation(
    tracker: &mut PluginSdkDeprecationTracker,
    plugin_id: Option<&str>,
    method: &str,
    replacement: &str,
) -> Option<PluginDeprecationWarning> {
    tracker.warn_deprecation(
        "session-transcript",
        method,
        replacement,
        plugin_id,
        None,
        Some("DEP_SESSION_PERSISTENCE"),
    )
}
