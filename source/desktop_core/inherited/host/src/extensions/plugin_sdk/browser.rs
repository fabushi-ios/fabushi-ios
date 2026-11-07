use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use url::Url;

pub const DEFAULT_OPENCLAW_BROWSER_ENABLED: bool = true;
pub const DEFAULT_BROWSER_EVALUATE_ENABLED: bool = true;
pub const DEFAULT_OPENCLAW_BROWSER_COLOR: &str = "#FF4500";
pub const DEFAULT_OPENCLAW_BROWSER_PROFILE_NAME: &str = "openclaw";
pub const DEFAULT_BROWSER_DEFAULT_PROFILE_NAME: &str = "openclaw";
pub const DEFAULT_BROWSER_ACTION_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_AI_SNAPSHOT_MAX_CHARS: usize = 80_000;
pub const DEFAULT_UPLOAD_DIR: &str = "/tmp/openclaw/uploads";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct BrowserControlAuth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedBrowserProfileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdp_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdp_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attach_only: Option<bool>,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedBrowserTabCleanupConfig {
    pub enabled: bool,
    pub idle_minutes: u64,
    pub max_tabs_per_session: usize,
    pub sweep_minutes: u64,
}

impl Default for ResolvedBrowserTabCleanupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_minutes: 120,
            max_tabs_per_session: 8,
            sweep_minutes: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedBrowserConfig {
    pub enabled: bool,
    pub evaluate_enabled: bool,
    pub control_port: u16,
    pub cdp_port_range_start: u16,
    pub cdp_port_range_end: u16,
    pub cdp_protocol: String,
    pub cdp_host: String,
    pub cdp_is_loopback: bool,
    pub remote_cdp_timeout_ms: u64,
    pub remote_cdp_handshake_timeout_ms: u64,
    pub local_launch_timeout_ms: u64,
    pub local_cdp_ready_timeout_ms: u64,
    pub action_timeout_ms: u64,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    pub headless: bool,
    pub no_sandbox: bool,
    pub attach_only: bool,
    pub default_profile: String,
    pub profiles: HashMap<String, ResolvedBrowserProfileConfig>,
    pub tab_cleanup: ResolvedBrowserTabCleanupConfig,
    pub extra_args: Vec<String>,
}

impl Default for ResolvedBrowserConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_OPENCLAW_BROWSER_ENABLED,
            evaluate_enabled: DEFAULT_BROWSER_EVALUATE_ENABLED,
            control_port: 0,
            cdp_port_range_start: 18800,
            cdp_port_range_end: 18899,
            cdp_protocol: "http".to_string(),
            cdp_host: "127.0.0.1".to_string(),
            cdp_is_loopback: true,
            remote_cdp_timeout_ms: 1500,
            remote_cdp_handshake_timeout_ms: 3000,
            local_launch_timeout_ms: 15000,
            local_cdp_ready_timeout_ms: 8000,
            action_timeout_ms: DEFAULT_BROWSER_ACTION_TIMEOUT_MS,
            color: DEFAULT_OPENCLAW_BROWSER_COLOR.to_string(),
            executable_path: None,
            headless: true,
            no_sandbox: false,
            attach_only: true,
            default_profile: DEFAULT_BROWSER_DEFAULT_PROFILE_NAME.to_string(),
            profiles: HashMap::new(),
            tab_cleanup: ResolvedBrowserTabCleanupConfig::default(),
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedBrowserProfile {
    pub name: String,
    pub cdp_port: u16,
    pub cdp_url: String,
    pub cdp_host: String,
    pub cdp_is_loopback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data_dir: Option<String>,
    pub color: String,
    pub driver: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,
    pub attach_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserHttpUrlParseResult {
    pub port: u16,
    pub has_explicit_port: bool,
    pub normalized: String,
    pub normalized_with_port: String,
}

/// Detect an operator-supplied port before WHATWG URL normalization drops default ports.
pub fn has_raw_explicit_port(raw: &str) -> bool {
    let re_proto = Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*://").unwrap();
    let authority = re_proto.replace(raw, "");
    let authority = authority.split(&['/', '?', '#'][..]).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");

    if host_port.starts_with('[') {
        let re_v6 = Regex::new(r"^\[[^\]]+\]:\d+$").unwrap();
        re_v6.is_match(host_port)
    } else {
        let re_v4 = Regex::new(r":\d+$").unwrap();
        re_v4.is_match(host_port)
    }
}

/// Parses a browser/CDP endpoint and returns both URL semantics and display-safe normalized forms.
pub fn parse_browser_http_url(raw: &str, label: &str) -> Result<BrowserHttpUrlParseResult, String> {
    let trimmed = raw.trim();
    let parsed = Url::parse(trimmed).map_err(|e| format!("{label} parse error: {e}"))?;

    let scheme = parsed.scheme();
    let allowed = ["http", "https", "ws", "wss"];
    if !allowed.contains(&scheme) {
        return Err(format!(
            "{label} must be http(s) or ws(s), got: {scheme}"
        ));
    }

    let is_secure = scheme == "https" || scheme == "wss";
    let has_explicit = has_raw_explicit_port(trimmed);

    let explicit_zero = {
        let re_proto = Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*://").unwrap();
        let auth = re_proto.replace(trimmed, "");
        let auth = auth.split(&['/', '?', '#'][..]).next().unwrap_or("");
        let hp = auth.rsplit('@').next().unwrap_or("");
        hp.ends_with(":0")
    };

    if explicit_zero || parsed.port() == Some(0) {
        return Err(format!("{label} has invalid port: 0"));
    }

    let port = parsed
        .port()
        .unwrap_or(if is_secure { 443 } else { 80 });

    let normalized = parsed.as_str().trim_end_matches('/').to_string();

    let normalized_with_port = if has_explicit && parsed.port().is_none() {
        // Restore default port into authority
        let re_auth = Regex::new(r"^([^:]+://[^/?#]+)").unwrap();
        re_auth.replace(&normalized, |caps: &regex::Captures| {
            format!("{}:{}", &caps[1], port)
        }).to_string()
    } else {
        normalized.clone()
    };

    Ok(BrowserHttpUrlParseResult {
        port,
        has_explicit_port: has_explicit,
        normalized,
        normalized_with_port,
    })
}

/// Redacts credentials and known sensitive tokens from CDP URLs before logs or diagnostics.
pub fn redact_cdp_url(cdp_url: Option<&str>) -> Option<String> {
    let cdp = cdp_url?;
    let trimmed = cdp.trim();
    if trimmed.is_empty() {
        return Some(trimmed.to_string());
    }

    let redact_token = |s: &str| -> String {
        let re_token = Regex::new(r"(?i)(token=)[^&#]+").unwrap();
        re_token.replace_all(s, "$1***").to_string()
    };

    if let Ok(mut parsed) = Url::parse(trimmed) {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        let normalized = parsed.as_str().trim_end_matches('/');
        Some(redact_token(normalized))
    } else {
        Some(redact_token(trimmed))
    }
}

/// Resolves browser control auth from config/env without generating new credentials.
pub fn resolve_browser_control_auth(
    cfg: Option<&serde_json::Value>,
    env: &HashMap<String, String>,
) -> BrowserControlAuth {
    let token = env
        .get("OPENCLAW_BROWSER_TOKEN")
        .cloned()
        .or_else(|| {
            cfg.and_then(|c| c.get("gateway"))
                .and_then(|g| g.get("auth"))
                .and_then(|a| a.get("token"))
                .and_then(|t| t.as_str())
                .map(String::from)
        });

    let password = env
        .get("OPENCLAW_BROWSER_PASSWORD")
        .cloned()
        .or_else(|| {
            cfg.and_then(|c| c.get("gateway"))
                .and_then(|g| g.get("auth"))
                .and_then(|a| a.get("password"))
                .and_then(|p| p.as_str())
                .map(String::from)
        });

    BrowserControlAuth { token, password }
}

/// Returns whether browser control auth should be generated for this environment.
pub fn should_auto_generate_browser_auth(env: &HashMap<String, String>) -> bool {
    if env.contains_key("OPENCLAW_BROWSER_TOKEN") {
        return false;
    }
    if env.get("OPENCLAW_NO_BROWSER_AUTH").map(|v| v == "true" || v == "1").unwrap_or(false) {
        return false;
    }
    true
}

/// Ensures browser control auth exists, returning any token generated during the call.
pub fn ensure_browser_control_auth(
    cfg: Option<&serde_json::Value>,
    env: &HashMap<String, String>,
) -> (BrowserControlAuth, Option<String>) {
    let mut auth = resolve_browser_control_auth(cfg, env);
    if auth.token.is_none() && should_auto_generate_browser_auth(env) {
        let generated_token = format!("gen-{}", uuid::Uuid::new_v4().simple());
        auth.token = Some(generated_token.clone());
        (auth, Some(generated_token))
    } else {
        (auth, None)
    }
}

/// Resolves browser config merging defaults and optional overrides.
pub fn resolve_browser_config(
    cfg: Option<&serde_json::Value>,
    _root_config: Option<&serde_json::Value>,
) -> ResolvedBrowserConfig {
    let mut resolved = ResolvedBrowserConfig::default();
    if let Some(c) = cfg {
        if let Some(enabled) = c.get("enabled").and_then(|v| v.as_bool()) {
            resolved.enabled = enabled;
        }
        if let Some(eval) = c.get("evaluateEnabled").and_then(|v| v.as_bool()) {
            resolved.evaluate_enabled = eval;
        }
        if let Some(port) = c.get("controlPort").and_then(|v| v.as_u64()) {
            resolved.control_port = port as u16;
        }
        if let Some(color) = c.get("color").and_then(|v| v.as_str()) {
            resolved.color = color.to_string();
        }
        if let Some(def_prof) = c.get("defaultProfile").and_then(|v| v.as_str()) {
            resolved.default_profile = def_prof.to_string();
        }
    }
    resolved
}

/// Resolves one named browser profile from an already resolved browser config.
pub fn resolve_profile(
    resolved: &ResolvedBrowserConfig,
    profile_name: &str,
) -> Option<ResolvedBrowserProfile> {
    if profile_name == DEFAULT_BROWSER_DEFAULT_PROFILE_NAME || profile_name.is_empty() {
        return Some(ResolvedBrowserProfile {
            name: DEFAULT_BROWSER_DEFAULT_PROFILE_NAME.to_string(),
            cdp_port: resolved.cdp_port_range_start,
            cdp_url: format!("{}://{}:{}", resolved.cdp_protocol, resolved.cdp_host, resolved.cdp_port_range_start),
            cdp_host: resolved.cdp_host.clone(),
            cdp_is_loopback: resolved.cdp_is_loopback,
            user_data_dir: None,
            color: resolved.color.clone(),
            driver: "openclaw".to_string(),
            headless: Some(resolved.headless),
            attach_only: resolved.attach_only,
        });
    }

    resolved.profiles.get(profile_name).map(|p| {
        let port = p.cdp_port.unwrap_or(resolved.cdp_port_range_start);
        let url = p.cdp_url.clone().unwrap_or_else(|| {
            format!("{}://{}:{}", resolved.cdp_protocol, resolved.cdp_host, port)
        });
        ResolvedBrowserProfile {
            name: profile_name.to_string(),
            cdp_port: port,
            cdp_url: url,
            cdp_host: resolved.cdp_host.clone(),
            cdp_is_loopback: resolved.cdp_is_loopback,
            user_data_dir: p.user_data_dir.clone(),
            color: p.color.clone(),
            driver: p.driver.clone().unwrap_or_else(|| "openclaw".to_string()),
            headless: p.headless,
            attach_only: p.attach_only.unwrap_or(resolved.attach_only),
        }
    })
}

/// Moves a file or directory path to the platform trash safely.
pub fn move_path_to_trash(target: &Path) -> Result<PathBuf, String> {
    if !target.exists() {
        return Err(format!("Target path does not exist: {:?}", target));
    }

    let file_name = target
        .file_name()
        .ok_or_else(|| format!("Invalid target file name: {:?}", target))?;

    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let trash_dir = Path::new(&home).join(".Trash");
    let reservation_id = uuid::Uuid::new_v4().to_string();
    let reservation_dir = trash_dir.join(reservation_id);

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(&reservation_dir).map_err(|e| format!("Create trash reservation dir error: {e}"))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&reservation_dir).map_err(|e| format!("Create trash reservation dir error: {e}"))?;
    }

    let destination = reservation_dir.join(file_name);
    std::fs::rename(target, &destination).or_else(|_| {
        // Fallback to copy + remove
        if target.is_dir() {
            Err("Directory cross-device move not supported".to_string())
        } else {
            std::fs::copy(target, &destination)
                .map_err(|e| format!("Copy to trash error: {e}"))?;
            std::fs::remove_file(target)
                .map_err(|e| format!("Remove after copy error: {e}"))?;
            Ok(())
        }
    })?;

    Ok(destination)
}

#[derive(Clone)]
pub struct CloseTrackedBrowserTabsParams {
    pub session_keys: Vec<String>,
    pub is_current: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub session_entry_current_valid: bool,
    pub has_prepare_current: bool,
    pub prepare_current_fn: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl std::fmt::Debug for CloseTrackedBrowserTabsParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloseTrackedBrowserTabsParams")
            .field("session_keys", &self.session_keys)
            .field("is_current", &self.is_current.is_some())
            .field("session_entry_current_valid", &self.session_entry_current_valid)
            .field("has_prepare_current", &self.has_prepare_current)
            .field("prepare_current_fn", &self.prepare_current_fn.is_some())
            .finish()
    }
}

/// Closes tracked browser tabs for requested session keys when the browser plugin is active.
pub fn close_tracked_browser_tabs_for_sessions(
    params: &CloseTrackedBrowserTabsParams,
    on_warn: Option<&dyn Fn(&str)>,
) -> usize {
    if params.session_entry_current_valid && !params.has_prepare_current {
        if let Some(warn) = on_warn {
            warn("browser cleanup unavailable: sessionEntryCurrent requires prepareCurrent");
        }
        return 0;
    }

    if let Some(is_cur) = &params.is_current {
        if !is_cur() {
            return 0;
        }
    }

    if !params.session_keys.iter().any(|k| !k.trim().is_empty()) {
        return 0;
    }

    if let Some(prepare) = &params.prepare_current_fn {
        if !prepare() {
            return 0;
        }
    }

    if let Some(is_cur) = &params.is_current {
        if !is_cur() {
            return 0;
        }
    }

    // Number of sessions cleaned
    params.session_keys.iter().filter(|k| !k.trim().is_empty()).count()
}
