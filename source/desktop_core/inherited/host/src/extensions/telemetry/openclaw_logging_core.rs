//! OpenClaw Logging Core, Console Formatting, and Level Filtering
//!
//! Mirrors:
//! - `src/logging/console.ts`
//! - `src/logging/levels.ts`
//! - `src/logging/config.ts`
//! - `src/logging/env-log-level.ts`
//! - `src/logging/json-console-line.ts`
//! - `src/logging/timestamps.ts`
//! - `src/logging/subsystem.ts`
//! - `src/logger.ts`
//! - `src/logging.ts`

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LogLevel {
    Silent = 0,
    Fatal = 1,
    Error = 2,
    Warn = 3,
    Info = 4,
    Debug = 5,
    Trace = 6,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Silent => "silent",
            LogLevel::Fatal => "fatal",
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

pub fn parse_log_level(s: &str, default: LogLevel) -> LogLevel {
    match s.trim().to_ascii_lowercase().as_str() {
        "silent" => LogLevel::Silent,
        "fatal" => LogLevel::Fatal,
        "error" => LogLevel::Error,
        "warn" | "warning" => LogLevel::Warn,
        "info" => LogLevel::Info,
        "debug" => LogLevel::Debug,
        "trace" => LogLevel::Trace,
        _ => default,
    }
}

pub fn resolve_env_log_level(env_val: Option<&str>, verbose: bool) -> LogLevel {
    if verbose {
        return LogLevel::Debug;
    }
    match env_val {
        Some(v) if !v.trim().is_empty() => parse_log_level(v, LogLevel::Info),
        _ => LogLevel::Info,
    }
}

pub fn is_level_enabled(threshold: LogLevel, target: LogLevel) -> bool {
    if threshold == LogLevel::Silent {
        return false;
    }
    target <= threshold && target != LogLevel::Silent
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleStyle {
    Plain,
    Pretty,
    Json,
}

pub struct ConsoleLineFormatter;

impl ConsoleLineFormatter {
    pub fn format_json(
        timestamp_iso: &str,
        level: LogLevel,
        subsystem: Option<&str>,
        message: &str,
    ) -> String {
        let mut obj = json!({
            "timestamp": timestamp_iso,
            "level": level.as_str(),
            "message": message,
        });

        if let Some(sub) = subsystem {
            if let Some(map) = obj.as_object_mut() {
                map.insert("subsystem".to_string(), json!(sub));
            }
        }

        obj.to_string()
    }

    pub fn format_text(
        timestamp_iso: &str,
        level: LogLevel,
        subsystem: Option<&str>,
        message: &str,
    ) -> String {
        if let Some(sub) = subsystem {
            format!("[{}] {:5} [{}] {}", timestamp_iso, level.as_str().to_ascii_uppercase(), sub, message)
        } else {
            format!("[{}] {:5} {}", timestamp_iso, level.as_str().to_ascii_uppercase(), message)
        }
    }
}
