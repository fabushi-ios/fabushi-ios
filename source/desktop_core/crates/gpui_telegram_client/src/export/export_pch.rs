//! Target Symbol: ExportPch
//! Telegram Desktop 1:1 Submodule: `export_pch.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Export subsystem common environment, paths, and platform abstraction utilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPch {
    pub base_output_path: PathBuf,
    pub session_id: u64,
    pub created_at: i64,
    pub temporary_cache_dir: PathBuf,
    pub max_concurrent_io: usize,
}

impl Default for ExportPch {
    fn default() -> Self {
        Self {
            base_output_path: PathBuf::from("Telegram_Export"),
            session_id: 0,
            created_at: Utc::now().timestamp(),
            temporary_cache_dir: PathBuf::from("Telegram_Export/cache"),
            max_concurrent_io: 4,
        }
    }
}

impl ExportPch {
    pub fn new(output_path: impl Into<PathBuf>, session_id: u64) -> Self {
        let base_path = output_path.into();
        let cache_dir = base_path.join("cache");
        Self {
            base_output_path: base_path,
            session_id,
            created_at: Utc::now().timestamp(),
            temporary_cache_dir: cache_dir,
            max_concurrent_io: 4,
        }
    }

    pub fn resolve_relative_path(&self, relative_path: &str) -> Option<PathBuf> {
        let cleaned = relative_path.trim_start_matches(['/', '\\']);
        if cleaned.contains("..") {
            return None;
        }
        Some(self.base_output_path.join(cleaned))
    }

    pub fn formatted_timestamp(&self) -> String {
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
    }
}
