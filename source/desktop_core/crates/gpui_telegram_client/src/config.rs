use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);


pub const DEFAULT_SERVER_URL: &str = "http://34.170.16.35:8080";
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
        let mut i = 1;
        while i < args.len() {
            if args[i] == "--server-url" || args[i] == "-s" {
                if i + 1 < args.len() {
                    cli_url = Some(args[i + 1].clone());
                    i += 1;
                }
            } else if let Some(stripped) = args[i].strip_prefix("--server-url=") {
                cli_url = Some(stripped.to_string());
            }
            i += 1;
        }

        // Tier 1: CLI Argument
        if let Some(url) = cli_url {
            let trimmed = url.trim();
            if !trimmed.is_empty() {
                let normalized = Self::normalize_url(trimmed);
                return Self {
                    server_url: normalized,
                    http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS,
                    ws_ping_interval_secs: DEFAULT_WS_PING_INTERVAL_SECS,
                    max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
                    config_source: ConfigSource::CliArgument,
                };
            }
        }

        // Tier 2: Environment Variable
        if let Ok(env_url) = env::var("GPUI_TELEGRAM_SERVER_URL") {
            let trimmed = env_url.trim();
            if !trimmed.is_empty() {
                return Self {
                    server_url: Self::normalize_url(trimmed),
                    http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS,
                    ws_ping_interval_secs: DEFAULT_WS_PING_INTERVAL_SECS,
                    max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
                    config_source: ConfigSource::EnvironmentVariable,
                };
            }
        }

        // Tier 3: Config File (~/.config/gpui_telegram/config.json)
        let config_path = Self::get_config_path();
        if config_path.exists() {
            if let Ok(content) = fs::read_to_string(&config_path) {
                if let Ok(cfg_file) = serde_json::from_str::<UserConfigFile>(&content) {
                    if let Some(url) = cfg_file.server_url {
                        let trimmed = url.trim();
                        if !trimmed.is_empty() {
                            return Self {
                                server_url: Self::normalize_url(trimmed),
                                http_timeout_secs: cfg_file
                                    .http_timeout_secs
                                    .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS),
                                ws_ping_interval_secs: cfg_file
                                    .ws_ping_interval_secs
                                    .unwrap_or(DEFAULT_WS_PING_INTERVAL_SECS),
                                max_reconnect_attempts: cfg_file
                                    .max_reconnect_attempts
                                    .unwrap_or(DEFAULT_MAX_RECONNECT_ATTEMPTS),
                                config_source: ConfigSource::ConfigFile(config_path),
                            };
                        }
                    }
                }
            }
        }

        // Tier 4: Fallback Default
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS,
            ws_ping_interval_secs: DEFAULT_WS_PING_INTERVAL_SECS,
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            config_source: ConfigSource::DefaultFallback,
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

    /// Returns base application configuration directory (~/.config/gpui_telegram).
    pub fn get_app_dir() -> PathBuf {
        if let Ok(home) = env::var("HOME") {
            PathBuf::from(home).join(".config").join("gpui_telegram")
        } else {
            PathBuf::from(".config").join("gpui_telegram")
        }
    }

    /// Returns path to user configuration file (~/.config/gpui_telegram/config.json).
    pub fn get_config_path() -> PathBuf {
        Self::get_app_dir().join("config.json")
    }

    /// Returns path to session persistence file (~/.config/gpui_telegram/session.json).
    pub fn get_session_path() -> PathBuf {
        Self::get_app_dir().join("session.json")
    }

    /// Persists session data atomically with secure Unix file permissions (0600).
    pub fn save_session(session: &StoredSession) -> Result<(), std::io::Error> {
        let dir = Self::get_app_dir();
        fs::create_dir_all(&dir)?;
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

        if let Err(e) = (|| -> Result<(), std::io::Error> {
            fs::write(&tmp_path, json)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o600);
                let _ = fs::set_permissions(&tmp_path, perms);
            }

            fs::rename(&tmp_path, &target_path)?;
            Ok(())
        })() {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }

        Ok(())
    }

    /// Loads persisted session from ~/.config/gpui_telegram/session.json if valid.
    pub fn load_session() -> Option<StoredSession> {
        let path = Self::get_session_path();
        if path.exists() {
            let content = fs::read_to_string(path).ok()?;
            serde_json::from_str(&content).ok()
        } else {
            None
        }
    }

    /// Clears any cached session from disk.
    pub fn clear_session() {
        let path = Self::get_session_path();
        if path.exists() {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_normalization() {
        assert_eq!(
            ClientConfig::normalize_url("34.170.16.35:8080"),
            "http://34.170.16.35:8080"
        );
        assert_eq!(
            ClientConfig::normalize_url("http://34.170.16.35:8080/"),
            "http://34.170.16.35:8080"
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
            server_url: "http://34.170.16.35:8080".to_string(),
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
}

