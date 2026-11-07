use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub media_storage_path: String,
    pub session_timeout_secs: u64,
    pub ws_heartbeat_interval_secs: u64,
    pub ws_inactivity_timeout_secs: u64,
    #[serde(default = "default_agent_model")]
    pub default_agent_model: String,
}

fn default_agent_model() -> String {
    "grok-3".into()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            database_url: ":memory:".into(),
            media_storage_path: "data/media".into(),
            session_timeout_secs: 30 * 24 * 3600, // 30 days
            ws_heartbeat_interval_secs: 30,
            ws_inactivity_timeout_secs: 60,
            default_agent_model: "grok-3".into(),
        }
    }
}

impl ServerConfig {
    /// Read server configuration from environment variables with sensible defaults.
    /// When DATABASE_URL is unset, defaults to physical storage "sqlite://data/gpui_telegram.db"
    /// for production and standalone daemon execution.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(host) = std::env::var("HOST") {
            let host_trimmed = host.trim();
            if !host_trimmed.is_empty() {
                config.host = host_trimmed.to_string();
            }
        }
        if let Ok(port_str) = std::env::var("PORT") {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                config.port = port;
            }
        }
        let db_url = std::env::var("DATABASE_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "sqlite://data/gpui_telegram.db".into());
        config.database_url = db_url;

        if let Ok(media_path) = std::env::var("MEDIA_STORAGE_PATH") {
            let media_trimmed = media_path.trim();
            if !media_trimmed.is_empty() {
                config.media_storage_path = media_trimmed.to_string();
            }
        }
        if let Ok(model) = std::env::var("DEFAULT_AGENT_MODEL") {
            let model_trimmed = model.trim();
            if !model_trimmed.is_empty() {
                config.default_agent_model = model_trimmed.to_string();
            }
        }
        config
    }

    /// Resolves SQLite URI schemes (e.g. "sqlite://path" -> "path")
    pub fn resolve_db_path(&self) -> &str {
        if let Some(stripped) = self.database_url.strip_prefix("sqlite://") {
            stripped
        } else if let Some(stripped) = self.database_url.strip_prefix("sqlite:") {
            stripped
        } else {
            &self.database_url
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_server_config_default_is_in_memory() {
        let config = ServerConfig::default();
        assert_eq!(config.database_url, ":memory:");
        assert_eq!(config.resolve_db_path(), ":memory:");
    }

    #[test]
    fn test_server_config_from_env_defaults_to_disk() {
        let _lock = ENV_LOCK.lock().unwrap();
        let old_val = std::env::var("DATABASE_URL").ok();
        unsafe {
            std::env::remove_var("DATABASE_URL");
        }

        let config = ServerConfig::from_env();
        assert_eq!(config.database_url, "sqlite://data/gpui_telegram.db");
        assert_eq!(config.resolve_db_path(), "data/gpui_telegram.db");

        if let Some(v) = old_val {
            unsafe {
                std::env::set_var("DATABASE_URL", v);
            }
        }
    }

    #[test]
    fn test_server_config_from_env_custom_database_url() {
        let _lock = ENV_LOCK.lock().unwrap();
        let old_val = std::env::var("DATABASE_URL").ok();
        unsafe {
            std::env::set_var("DATABASE_URL", "sqlite:///opt/fabushi/data/gpui_telegram.db");
        }

        let config = ServerConfig::from_env();
        assert_eq!(config.database_url, "sqlite:///opt/fabushi/data/gpui_telegram.db");
        assert_eq!(config.resolve_db_path(), "/opt/fabushi/data/gpui_telegram.db");

        if let Some(v) = old_val {
            unsafe {
                std::env::set_var("DATABASE_URL", v);
            }
        } else {
            unsafe {
                std::env::remove_var("DATABASE_URL");
            }
        }
    }

    #[test]
    fn test_resolve_db_path_schemes() {
        let mut config = ServerConfig::default();

        config.database_url = ":memory:".into();
        assert_eq!(config.resolve_db_path(), ":memory:");

        config.database_url = "sqlite://:memory:".into();
        assert_eq!(config.resolve_db_path(), ":memory:");

        config.database_url = "sqlite::memory:".into();
        assert_eq!(config.resolve_db_path(), ":memory:");

        config.database_url = "sqlite://data/custom.db".into();
        assert_eq!(config.resolve_db_path(), "data/custom.db");

        config.database_url = "sqlite:data/custom.db".into();
        assert_eq!(config.resolve_db_path(), "data/custom.db");

        config.database_url = "data/custom.db".into();
        assert_eq!(config.resolve_db_path(), "data/custom.db");
    }
}

