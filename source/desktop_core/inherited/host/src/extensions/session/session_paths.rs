use std::collections::BTreeMap;
use std::env;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::extensions::session::session_diagnostics::{
    SessionDiagnostic, report_session_diagnostic,
};
use crate::host_paths::get_sand_root_dir_with;
use crate::storage::agent_paths::{
    SandInvalidAgentIdError, assert_valid_sand_agent_id, get_sand_agents_root_dir,
};

pub const STORE_FILENAME: &str = "store.db";
pub const CONVERSATION_BLOBS_FILENAME: &str = "conversation-blobs.db";
pub const SAND_CONVERSATION_ROOT_SLOT_ID: &[u8] = b"sand-live-conversation-root-v1__";
pub const STALE_ROOT_CLEANUP_VERSION: u32 = 1;
pub const ACTIVE_AGENT_FILENAME: &str = "active-agent.json";
pub const NATIVE_CONVERSATIONS_DIRNAME: &str = ".conversations";
pub const HIDDEN_ENTRY_REPAIR_VERSION: u32 = 1;
pub const LEGACY_GROUP_MEMBERS_DIRNAME: &str = "members";
pub const CONNECTOR_SECRETS_DIRNAME: &str = "connector-secrets";
pub const AGENT_HARNESS_SESSION_KEY_RESERVED_MESSAGE: &str =
    "Session key namespace is reserved for agent harness-owned sessions.";

static PINNED_STALE_ROOT_GC_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn pin_stale_root_gc(enabled: bool) {
    PINNED_STALE_ROOT_GC_ENABLED.store(enabled, Ordering::Release);
}

pub fn is_stale_root_gc_enabled_from_env(environment: &BTreeMap<String, String>) -> bool {
    match environment
        .get("SAND_STALE_ROOT_GC")
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("1" | "true" | "on") => true,
        Some("0" | "false" | "off") => false,
        _ => PINNED_STALE_ROOT_GC_ENABLED.load(Ordering::Acquire),
    }
}

pub fn is_stale_root_gc_enabled() -> bool {
    is_stale_root_gc_enabled_from_env(&env::vars().collect())
}

fn default_home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_sand_transcripts_dir_with(
    home_dir: &Path,
    argv: &[String],
    environment: &BTreeMap<String, String>,
    cwd: &Path,
) -> PathBuf {
    get_sand_root_dir_with(home_dir, argv, environment, cwd).join("agent-transcripts")
}

pub fn get_sand_transcripts_dir(home_dir: Option<&Path>) -> PathBuf {
    let owned_home;
    let home = match home_dir {
        Some(home) => home,
        None => {
            owned_home = default_home_dir();
            &owned_home
        }
    };
    let environment = env::vars().collect::<BTreeMap<_, _>>();
    let argv = env::args().collect::<Vec<_>>();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    get_sand_transcripts_dir_with(home, &argv, &environment, &cwd)
}

pub fn get_agent_db_path(
    root_dir: &Path,
    agent_id: &str,
) -> Result<PathBuf, SandInvalidAgentIdError> {
    assert_valid_sand_agent_id(agent_id)?;
    Ok(root_dir.join(agent_id).join(STORE_FILENAME))
}

pub fn get_native_conversations_root(root_dir: &Path) -> PathBuf {
    root_dir.join(NATIVE_CONVERSATIONS_DIRNAME)
}

pub fn get_native_conversation_db_path(
    root_dir: &Path,
    conversation_id: &str,
) -> Result<PathBuf, SandInvalidAgentIdError> {
    assert_valid_sand_agent_id(conversation_id)?;
    Ok(get_native_conversations_root(root_dir)
        .join(conversation_id)
        .join(STORE_FILENAME))
}

pub fn get_connector_secrets_root(agents_root_dir: Option<&Path>) -> PathBuf {
    let owned_root;
    let root = match agents_root_dir {
        Some(root) => root,
        None => {
            owned_root = get_sand_agents_root_dir(None);
            &owned_root
        }
    };
    root.parent()
        .unwrap_or_else(|| Path::new(""))
        .join(CONNECTOR_SECRETS_DIRNAME)
}

pub fn stat_if_exists(path: &Path) -> Option<Metadata> {
    match fs::metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                let agent_id = path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_default();
                report_session_diagnostic(&SessionDiagnostic {
                    family: "store_db".into(),
                    kind: "path_stat_failed".into(),
                    metadata: BTreeMap::from([
                        ("agentId".into(), Value::String(agent_id)),
                        (
                            "errorClass".into(),
                            Value::String(format!("{:?}", error.kind())),
                        ),
                    ]),
                });
            }
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Cron,
    Direct,
    Group,
    Global,
    SpawnChild,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicSessionRouteError {
    EmptySelector,
    HarnessNamespaceReserved,
    EmptyAgentIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalSessionRouteError {
    MissingSelector,
    EmptyAgentIdentity,
    SelectorAgentMismatch,
    Public(PublicSessionRouteError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSessionRoute {
    pub raw_key: String,
    pub agent_id: String,
    pub kind: SessionKind,
}

/// Classify session kind equivalent to OpenClaw classifySessionKind.
pub fn classify_session_kind(
    key: &str,
    chat_type: Option<&str>,
    spawned_by: Option<&str>,
) -> SessionKind {
    if key == "global" {
        return SessionKind::Global;
    }
    if key == "unknown" {
        return SessionKind::Unknown;
    }
    let is_canonical_cron = parse_agent_session_key(key)
        .is_some_and(|(_agent_id, rest)| rest.to_ascii_lowercase().starts_with("cron:"));
    if is_canonical_cron {
        return SessionKind::Cron;
    }
    // Match the upstream `entry?.spawnedBy` truthiness rule: an absent or
    // empty-string value does not establish spawn ancestry.
    if spawned_by.is_some_and(|value| !value.is_empty()) {
        return SessionKind::SpawnChild;
    }
    if chat_type == Some("group")
        || chat_type == Some("channel")
        || key.contains(":group:")
        || key.contains(":channel:")
    {
        return SessionKind::Group;
    }
    SessionKind::Direct
}

/// Parse agent-scoped session key parts into (agent_id, rest) matching OpenClaw parseAgentSessionKey.
/// Format: agent:<agentId>:<rest>
pub fn parse_agent_session_key(session_key: &str) -> Option<(&str, &str)> {
    let trimmed = session_key.trim();
    let Some(prefix) = trimmed.get(..6) else {
        return None;
    };
    if !prefix.eq_ignore_ascii_case("agent:") {
        return None;
    }
    let after_agent = &trimmed[6..];
    let colon_idx = after_agent.find(':')?;
    let agent_id = after_agent[..colon_idx].trim();
    let rest = &after_agent[colon_idx + 1..];
    if agent_id.is_empty() || rest.is_empty() || rest.starts_with(':') {
        None
    } else {
        Some((agent_id, rest))
    }
}

/// Return whether a selector names the undeletable global main session.
///
/// Structural agent wrappers are case-insensitive at the public selector
/// boundary, so the deletion guard must use the same rule.
pub fn is_root_main_session_key(session_key: &str) -> bool {
    let trimmed = session_key.trim();
    if trimmed.eq_ignore_ascii_case("global") {
        return true;
    }
    parse_agent_session_key(trimmed).is_some_and(|(agent_id, rest)| {
        agent_id.eq_ignore_ascii_case("global") && rest.eq_ignore_ascii_case("main")
    })
}

/// Agent harness session key namespace check equivalent to OpenClaw isAgentHarnessSessionKey.
/// Evaluates whether the session key (or its rest segment if agent-wrapped) starts with "harness:".
pub fn is_agent_harness_session_key(session_key: &str) -> bool {
    let trimmed = session_key.trim();
    let rest = if let Some((_agent_id, rest)) = parse_agent_session_key(trimmed) {
        rest
    } else {
        trimmed
    };
    rest.to_ascii_lowercase().starts_with("harness:")
}

/// Public session commands must not create or mutate rows in the harness-owned namespace.
/// Harness runtimes use a separate trusted owner path to access those sessions.
pub fn reject_public_agent_harness_session_key(session_key: &str) -> Result<(), &'static str> {
    if is_agent_harness_session_key(session_key) {
        Err(AGENT_HARNESS_SESSION_KEY_RESERVED_MESSAGE)
    } else {
        Ok(())
    }
}

/// Resolve the one public session selector boundary used by Gateway session
/// and chat methods. This function is deliberately pure: downstream session,
/// transcript, lifecycle, and runner owners remain responsible for state and
/// execution.
pub fn resolve_public_session_route(
    session_key: &str,
    chat_type: Option<&str>,
    spawned_by: Option<&str>,
) -> Result<PublicSessionRoute, PublicSessionRouteError> {
    let raw_key = session_key.trim();
    if raw_key.is_empty() {
        return Err(PublicSessionRouteError::EmptySelector);
    }
    if is_agent_harness_session_key(raw_key) {
        return Err(PublicSessionRouteError::HarnessNamespaceReserved);
    }

    let agent_id = if let Some((agent_id, _rest)) = parse_agent_session_key(raw_key) {
        // OpenClaw normalizes the structural agent wrapper before returning
        // ParsedAgentSessionKey. Agent IDs are structural, while legacy raw
        // selectors remain opaque below.
        agent_id.to_ascii_lowercase()
    } else if raw_key.to_ascii_lowercase().starts_with("agent:") {
        return Err(PublicSessionRouteError::EmptyAgentIdentity);
    } else {
        raw_key.to_owned()
    };
    if agent_id.trim().is_empty() {
        return Err(PublicSessionRouteError::EmptyAgentIdentity);
    }

    Ok(PublicSessionRoute {
        raw_key: raw_key.to_owned(),
        agent_id,
        kind: classify_session_kind(raw_key, chat_type, spawned_by),
    })
}

/// Resolve a trusted producer's send target through the same selector owner as
/// public Gateway methods. A direct-main fallback is intentionally explicit;
/// background producers with a richer session kind must provide their key.
pub fn resolve_internal_send_route(
    agent_id: &str,
    session_key: Option<&str>,
    chat_type: Option<&str>,
    spawned_by: Option<&str>,
    allow_direct_main_fallback: bool,
) -> Result<PublicSessionRoute, InternalSessionRouteError> {
    let supplied_agent_id = agent_id.trim();
    if supplied_agent_id.is_empty() {
        return Err(InternalSessionRouteError::EmptyAgentIdentity);
    }
    let selector = match session_key.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_owned(),
        None if allow_direct_main_fallback => format!("agent:{supplied_agent_id}:main"),
        None => return Err(InternalSessionRouteError::MissingSelector),
    };
    let route = resolve_public_session_route(&selector, chat_type, spawned_by)
        .map_err(InternalSessionRouteError::Public)?;
    let matches_agent = if parse_agent_session_key(&selector).is_some() {
        route.agent_id.eq_ignore_ascii_case(supplied_agent_id)
    } else {
        route.agent_id == supplied_agent_id
    };
    if !matches_agent {
        return Err(InternalSessionRouteError::SelectorAgentMismatch);
    }
    Ok(route)
}

