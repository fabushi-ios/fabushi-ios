//! Target Symbol: DataDownloadManager
//! Telegram Desktop 1:1 Submodule: `data/data_download_manager`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadItem {
    pub id: u64,
    pub filename: String,
    pub target_path: String,
    pub total_bytes: i64,
    pub downloaded_bytes: i64,
    pub is_paused: bool,
    pub is_completed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataDownloadManager {
    pub default_download_directory: String,
    pub items: HashMap<u64, DownloadItem>,
    pub max_concurrent_downloads: usize,
}

impl Default for DataDownloadManager {
    fn default() -> Self {
        Self {
            default_download_directory: "~/Downloads/Telegram Desktop".to_string(),
            items: HashMap::new(),
            max_concurrent_downloads: 3,
        }
    }
}

impl DataDownloadManager {
    pub fn new(default_dir: impl Into<String>) -> Self {
        Self {
            default_download_directory: default_dir.into(),
            items: HashMap::new(),
            max_concurrent_downloads: 3,
        }
    }

    pub fn start_download(&mut self, id: u64, filename: impl Into<String>, total_bytes: i64) {
        let name = filename.into();
        let target = format!("{}/{}", self.default_download_directory, name);
        self.items.insert(
            id,
            DownloadItem {
                id,
                filename: name,
                target_path: target,
                total_bytes,
                downloaded_bytes: 0,
                is_paused: false,
                is_completed: false,
                error: None,
            },
        );
    }

    pub fn update_progress(&mut self, id: u64, bytes: i64) {
        if let Some(item) = self.items.get_mut(&id) {
            item.downloaded_bytes = bytes.min(item.total_bytes);
            if item.downloaded_bytes >= item.total_bytes && item.total_bytes > 0 {
                item.is_completed = true;
            }
        }
    }

    pub fn pause(&mut self, id: u64) {
        if let Some(item) = self.items.get_mut(&id) {
            item.is_paused = true;
        }
    }

    pub fn resume(&mut self, id: u64) {
        if let Some(item) = self.items.get_mut(&id) {
            item.is_paused = false;
        }
    }

    pub fn cancel(&mut self, id: u64) {
        self.items.remove(&id);
    }

    pub fn get_item(&self, id: u64) -> Option<&DownloadItem> {
        self.items.get(&id)
    }

    pub fn active_count(&self) -> usize {
        self.items
            .values()
            .filter(|i| !i.is_paused && !i.is_completed && i.error.is_none())
            .count()
    }
}
