use crate::net::proxy::ClientProxyConfig;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);


pub const DEFAULT_SERVER_URL: &str = "http://34.46.77.133:8080";
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 10;
pub const DEFAULT_WS_PING_INTERVAL_SECS: u64 = 25;
pub const DEFAULT_MAX_RECONNECT_ATTEMPTS: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConfigSource {
    CliArgument,
    EnvironmentVariable,
    ConfigFile(PathBuf),
    #[default]
    DefaultFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientConfig {
    pub server_url: String,
    pub http_timeout_secs: u64,
    pub ws_ping_interval_secs: u64,
    pub max_reconnect_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ClientProxyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_feed_url: Option<String>,
    #[serde(skip)]
    pub config_source: ConfigSource,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS,
            ws_ping_interval_secs: DEFAULT_WS_PING_INTERVAL_SECS,
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            proxy: None,
            update_feed_url: None,
            config_source: ConfigSource::DefaultFallback,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfigFile {
    pub server_url: Option<String>,
    pub http_timeout_secs: Option<u64>,
    pub ws_ping_interval_secs: Option<u64>,
    pub max_reconnect_attempts: Option<u32>,
    #[serde(default)]
    pub proxy: Option<ClientProxyConfig>,
    #[serde(default)]
    pub update_feed_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredSession {
    pub server_url: String,
    pub user_id: u64,
    pub username: String,
    pub token: String,
    pub session_id: String,
    pub created_at_ms: i64,
}

impl ClientConfig {
    /// Resolves configuration from environment arguments, environment variables,
    /// user config file (~/.config/gpui_telegram/config.json), or fallback default.
    pub fn resolve() -> Self {
        let args: Vec<String> = env::args().collect();
        Self::resolve_with_args(&args)
    }

    /// Resolves configuration with explicit command-line arguments.
    /// Follows the 4-tier precedence:
    /// 1. CLI arguments (`--server-url <url>`, `--server-url=<url>`, `-s <url>`)
    /// 2. Environment variable `GPUI_TELEGRAM_SERVER_URL`
    /// 3. Configuration file `~/.config/gpui_telegram/config.json`
    /// 4. Fallback default (`http://127.0.0.1:8080`)
    pub fn resolve_with_args(args: &[String]) -> Self {
        let mut cli_url: Option<String> = None;
        let mut cli_proxy: Option<String> = None;
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--server-url" || args[i] == "-s" {
                if i + 1 < args.len() {
                    cli_url = Some(args[i + 1].clone());
                    i += 1;
                }
            } else if let Some(stripped) = args[i].strip_prefix("--server-url=") {
                cli_url = Some(stripped.to_string());
            } else if let Some(stripped) = args[i].strip_prefix("-s=") {
                cli_url = Some(stripped.to_string());
            } else if args[i] == "--proxy-url" {
                if i + 1 < args.len() {
                    cli_proxy = Some(args[i + 1].clone());
                    i += 1;
                }
            } else if let Some(stripped) = args[i].strip_prefix("--proxy-url=") {
                cli_proxy = Some(stripped.to_string());
            }
            i += 1;
        }

        // Layer 4: Base Default
        let mut final_url = DEFAULT_SERVER_URL.to_string();
        let mut http_timeout_secs = DEFAULT_HTTP_TIMEOUT_SECS;
        let mut ws_ping_interval_secs = DEFAULT_WS_PING_INTERVAL_SECS;
        let mut max_reconnect_attempts = DEFAULT_MAX_RECONNECT_ATTEMPTS;
        let mut proxy_cfg: Option<ClientProxyConfig> = None;
        let mut update_feed_url: Option<String> = None;
        let mut source = ConfigSource::DefaultFallback;

        // Layer 3: User Config File (~/.config/gpui_telegram/config.json)
        let config_path = Self::get_config_path();
        if config_path.is_file() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(cfg_file) = serde_json::from_str::<UserConfigFile>(&content) {
                    if let Some(url) = cfg_file.server_url {
                        let trimmed = url.trim();
                        if !trimmed.is_empty() {
                            final_url = Self::normalize_url(trimmed);
                            source = ConfigSource::ConfigFile(config_path.clone());
                        }
                    }
                    if let Some(to) = cfg_file.http_timeout_secs {
                        http_timeout_secs = to;
                    }
                    if let Some(pi) = cfg_file.ws_ping_interval_secs {
                        ws_ping_interval_secs = pi;
                    }
                    if let Some(ra) = cfg_file.max_reconnect_attempts {
                        max_reconnect_attempts = ra;
                    }
                    if let Some(p) = cfg_file.proxy {
                        proxy_cfg = Some(p);
                    }
                    if let Some(feed) = cfg_file.update_feed_url {
                        let trimmed = feed.trim();
                        if !trimmed.is_empty() {
                            update_feed_url = Some(trimmed.to_string());
                        }
                    }
                }
            }
        }

        // Layer 2: Environment Variable (FABUSHI_SERVER_URL, GPUI_TELEGRAM_SERVER_URL, GPUI_TELEGRAM_PROXY_URL)
        for env_key in ["FABUSHI_SERVER_URL", "GPUI_TELEGRAM_SERVER_URL"] {
            if let Ok(env_url) = env::var(env_key) {
                let trimmed = env_url.trim();
                if !trimmed.is_empty() {
                    final_url = Self::normalize_url(trimmed);
                    source = ConfigSource::EnvironmentVariable;
                    break;
                }
            }
        }
        if let Ok(env_proxy) = env::var("GPUI_TELEGRAM_PROXY_URL") {
            if !env_proxy.trim().is_empty() {
                if let Ok(p) = ClientProxyConfig::from_url(&env_proxy) {
                    proxy_cfg = Some(p);
                }
            }
        }
        for env_key in ["FABUSHI_UPDATE_FEED_URL", "GPUI_TELEGRAM_UPDATE_FEED_URL"] {
            if let Ok(env_feed) = env::var(env_key) {
                let trimmed = env_feed.trim();
                if !trimmed.is_empty() {
                    update_feed_url = Some(trimmed.to_string());
                    break;
                }
            }
        }

        // Layer 1: CLI Argument (--server-url, --proxy-url)
        if let Some(raw_cli) = cli_url {
            let trimmed = raw_cli.trim();
            if !trimmed.is_empty() {
                final_url = Self::normalize_url(trimmed);
                source = ConfigSource::CliArgument;
            }
        }
        if let Some(raw_cli_proxy) = cli_proxy {
            if !raw_cli_proxy.trim().is_empty() {
                if let Ok(p) = ClientProxyConfig::from_url(&raw_cli_proxy) {
                    proxy_cfg = Some(p);
                }
            }
        }

        Self {
            server_url: final_url,
            http_timeout_secs,
            ws_ping_interval_secs,
            max_reconnect_attempts,
            proxy: proxy_cfg,
            update_feed_url,
            config_source: source,
        }
    }

    /// Normalizes URL by trimming whitespace, stripping trailing slashes,
    /// and ensuring standard http:// or https:// scheme prefix.
    pub fn normalize_url(raw: &str) -> String {
        let trimmed = raw.trim().trim_end_matches('/');
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else {
            format!("http://{}", trimmed)
        }
    }

    fn resolve_app_dir(config_dir: Option<&str>, home: Option<&str>) -> PathBuf {
        if let Some(dir) = config_dir.map(str::trim).filter(|s| !s.is_empty()) {
            return PathBuf::from(dir);
        }
        if let Some(home_dir) = home.map(str::trim).filter(|s| !s.is_empty()) {
            #[cfg(target_os = "ios")]
            {
                return PathBuf::from(home_dir)
                    .join("Library")
                    .join("Application Support")
                    .join("Fabushi");
            }
            #[cfg(not(target_os = "ios"))]
            {
                return PathBuf::from(home_dir)
                    .join(".config")
                    .join("gpui_telegram");
            }
        }
        PathBuf::from(".config").join("gpui_telegram")
    }

    /// Returns base application configuration directory (`FABUSHI_APP_SUPPORT_DIR`, `GPUI_TELEGRAM_CONFIG_DIR`, or sandbox/home default).
    pub fn get_app_dir() -> PathBuf {
        let config_dir = env::var("FABUSHI_APP_SUPPORT_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| env::var("GPUI_TELEGRAM_CONFIG_DIR").ok());
        let home = env::var("HOME").ok();
        Self::resolve_app_dir(config_dir.as_deref(), home.as_deref())
    }

    /// Returns path to user configuration file (~/.config/gpui_telegram/config.json).
    pub fn get_config_path() -> PathBuf {
        Self::get_app_dir().join("config.json")
    }

    /// Persists user-configured server URL to ~/.config/gpui_telegram/config.json.
    pub fn save_server_url(raw_url: &str) -> Result<String, std::io::Error> {
        let normalized = Self::normalize_url(raw_url);
        let dir = Self::get_app_dir();
        fs::create_dir_all(&dir)?;
        let config_path = Self::get_config_path();
        let mut existing = if config_path.is_file() {
            fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| serde_json::from_str::<UserConfigFile>(&s).ok())
                .unwrap_or_default()
        } else {
            UserConfigFile::default()
        };
        existing.server_url = Some(normalized.clone());
        let json = serde_json::to_string_pretty(&existing)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&config_path, json.as_bytes())?;
        Ok(normalized)
    }

    /// Returns path to session persistence file (~/.config/gpui_telegram/session.json).
    pub fn get_session_path() -> PathBuf {
        Self::get_app_dir().join("session.json")
    }

    /// Persists session data atomically with strictly enforced POSIX 0600 mode at creation time.
    pub fn save_session(session: &StoredSession) -> Result<(), std::io::Error> {
        let dir = Self::get_app_dir();
        fs::create_dir_all(&dir)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }

        let target_path = Self::get_session_path();
        let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tid_raw = format!("{:?}", std::thread::current().id());
        let tid = tid_raw
            .strip_prefix("ThreadId(")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or(&tid_raw);
        let tmp_path = dir.join(format!(
            ".session.tmp.{}.{}.{}",
            std::process::id(),
            tid,
            counter
        ));

        let json = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let write_result = (|| -> Result<(), std::io::Error> {
            use std::io::Write;

            let mut open_opts = fs::OpenOptions::new();
            open_opts.write(true).create(true).truncate(true);

            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open_opts.mode(0o600); // Created atomically with 0600 permissions
            }

            let mut file = open_opts.open(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?; // Flush to disk blocks
            drop(file);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
            }

            fs::rename(&tmp_path, &target_path)?;
            Ok(())
        })();

        if let Err(e) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        Ok(())
    }

    /// Returns path to secondary XDG Data Home session persistence file (~/.local/share/gpui_telegram/session.json).
    pub fn get_xdg_data_session_path() -> PathBuf {
        if let Ok(data_home) = env::var("XDG_DATA_HOME") {
            PathBuf::from(data_home)
                .join("gpui_telegram")
                .join("session.json")
        } else if let Ok(home) = env::var("HOME") {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("gpui_telegram")
                .join("session.json")
        } else {
            PathBuf::from(".local")
                .join("share")
                .join("gpui_telegram")
                .join("session.json")
        }
    }

    /// Loads persisted session from ~/.config/gpui_telegram/session.json or ~/.local/share/gpui_telegram/session.json
    pub fn load_session() -> Option<StoredSession> {
        let candidates = [
            Self::get_session_path(),
            Self::get_xdg_data_session_path(),
        ];
        for path in candidates {
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(sess) = serde_json::from_str::<StoredSession>(&content) {
                        return Some(sess);
                    }
                }
            }
        }
        None
    }

    /// Clears any cached session from disk across primary and fallback locations.
    pub fn clear_session() {
        let p1 = Self::get_session_path();
        if p1.is_file() {
            let _ = fs::remove_file(p1);
        }
        let p2 = Self::get_xdg_data_session_path();
        if p2.is_file() {
            let _ = fs::remove_file(p2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_normalization() {
        assert_eq!(
            ClientConfig::normalize_url("34.46.77.133:8080"),
            "http://34.46.77.133:8080"
        );
        assert_eq!(
            ClientConfig::normalize_url("http://34.46.77.133:8080/"),
            "http://34.46.77.133:8080"
        );
        assert_eq!(
            ClientConfig::normalize_url("https://secure.example.com/api/"),
            "https://secure.example.com/api"
        );
    }

    #[test]
    fn test_cli_precedence() {
        let args = vec![
            "gpui_telegram".to_string(),
            "--server-url".to_string(),
            "http://custom.cloud:8080".to_string(),
        ];
        let cfg = ClientConfig::resolve_with_args(&args);
        assert_eq!(cfg.server_url, "http://custom.cloud:8080");
        assert_eq!(cfg.config_source, ConfigSource::CliArgument);
    }

    #[test]
    fn test_cli_equals_syntax() {
        let args = vec![
            "gpui_telegram".to_string(),
            "--server-url=http://custom-eq.cloud:8080/".to_string(),
        ];
        let cfg = ClientConfig::resolve_with_args(&args);
        assert_eq!(cfg.server_url, "http://custom-eq.cloud:8080");
        assert_eq!(cfg.config_source, ConfigSource::CliArgument);
    }

    #[test]
    fn test_cli_short_flag() {
        let args = vec![
            "gpui_telegram".to_string(),
            "-s".to_string(),
            "http://short.cloud:8080".to_string(),
        ];
        let cfg = ClientConfig::resolve_with_args(&args);
        assert_eq!(cfg.server_url, "http://short.cloud:8080");
        assert_eq!(cfg.config_source, ConfigSource::CliArgument);
    }

    #[test]
    fn test_stored_session_serde() {
        let session = StoredSession {
            server_url: "http://34.46.77.133:8080".to_string(),
            user_id: 100077,
            username: "probe_user".to_string(),
            token: "sess_token_123.456".to_string(),
            session_id: "sess_123".to_string(),
            created_at_ms: 1762543098000,
        };
        let json = serde_json::to_string(&session).unwrap();
        let loaded: StoredSession = serde_json::from_str(&json).unwrap();
        assert_eq!(session, loaded);
    }

    #[test]
    fn test_cli_empty_url_falls_through() {
        let args = vec![
            "gpui_telegram".to_string(),
            "--server-url".to_string(),
            "".to_string(),
        ];
        let cfg = ClientConfig::resolve_with_args(&args);
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
        assert_eq!(cfg.config_source, ConfigSource::DefaultFallback);
    }

    #[test]
    fn test_cli_whitespace_url_falls_through() {
        let args = vec![
            "gpui_telegram".to_string(),
            "--server-url=   ".to_string(),
        ];
        let cfg = ClientConfig::resolve_with_args(&args);
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
        assert_eq!(cfg.config_source, ConfigSource::DefaultFallback);
    }

    #[test]
    fn test_app_dir_gpui_telegram_config_dir_override() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        struct EnvGuard {
            key: &'static str,
            prev: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                unsafe {
                    match &self.prev {
                        Some(val) => env::set_var(self.key, val),
                        None => env::remove_var(self.key),
                    }
                }
            }
        }

        // Pure helper precedence and trimming checks
        assert_eq!(
            ClientConfig::resolve_app_dir(Some("  /custom/gpui-cfg  "), Some("/Users/alice")),
            PathBuf::from("/custom/gpui-cfg")
        );
        assert_eq!(
            ClientConfig::resolve_app_dir(Some("   "), Some("/Users/alice")),
            PathBuf::from("/Users/alice/.config/gpui_telegram")
        );
        assert_eq!(
            ClientConfig::resolve_app_dir(None, None),
            PathBuf::from(".config/gpui_telegram")
        );

        // Live environment variable override check on ClientConfig::get_app_dir()
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _guard = EnvGuard {
            key: "GPUI_TELEGRAM_CONFIG_DIR",
            prev: env::var("GPUI_TELEGRAM_CONFIG_DIR").ok(),
        };

        let override_dir = env::temp_dir().join(format!(
            "gpui_telegram_config_override_{}",
            std::process::id()
        ));
        let padded = format!("  {}  ", override_dir.display());
        unsafe {
            env::set_var("GPUI_TELEGRAM_CONFIG_DIR", &padded);
        }

        assert_eq!(ClientConfig::get_app_dir(), override_dir);
        assert_eq!(ClientConfig::get_config_path(), override_dir.join("config.json"));
        assert_eq!(
            ClientConfig::get_session_path(),
            override_dir.join("session.json")
        );

        unsafe {
            env::set_var("GPUI_TELEGRAM_CONFIG_DIR", "   ");
        }
        let expected_fallback = ClientConfig::resolve_app_dir(None, env::var("HOME").ok().as_deref());
        assert_eq!(ClientConfig::get_app_dir(), expected_fallback);
    }
}
