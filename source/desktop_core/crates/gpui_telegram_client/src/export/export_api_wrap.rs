//! Target Symbol: ExportApiWrap
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/export_api_wrap.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_settings::ExportSettings;
use crate::export::output::export_output_result::ExportOutputResultOutput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportFileTask {
    pub file_id: u64,
    pub access_hash: u64,
    pub file_reference: Vec<u8>,
    pub dc_id: u32,
    pub total_size: i64,
    pub local_path: String,
    pub downloaded_size: i64,
}

#[derive(Debug, Default)]
pub struct LoadedFileCache {
    entries: HashMap<u64, String>,
}

impl LoadedFileCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, file_id: u64, path: String) {
        self.entries.insert(file_id, path);
    }

    pub fn get(&self, file_id: u64) -> Option<&String> {
        self.entries.get(&file_id)
    }
}

/// MTProto query bounds, chunk iterators, and download queue matching upstream `Export::ApiWrap`
pub struct ExportApiWrap {
    settings: ExportSettings,
    download_queue: Mutex<VecDeque<ExportFileTask>>,
    active_downloads: Mutex<usize>,
    file_cache: Mutex<LoadedFileCache>,
    is_cancelled: Mutex<bool>,
    pub max_concurrent_downloads: usize,
}

impl ExportApiWrap {
    pub const MESSAGES_PER_PAGE: usize = 100;
    pub const SPLIT_RANGES_LIMIT: usize = 100;
    pub const DEFAULT_CONCURRENT_DOWNLOADS: usize = 4;

    pub fn new(settings: ExportSettings) -> Self {
        Self {
            settings,
            download_queue: Mutex::new(VecDeque::new()),
            active_downloads: Mutex::new(0),
            file_cache: Mutex::new(LoadedFileCache::new()),
            is_cancelled: Mutex::new(false),
            max_concurrent_downloads: Self::DEFAULT_CONCURRENT_DOWNLOADS,
        }
    }

    pub fn enqueue_file(&self, task: ExportFileTask) {
        let mut queue = self.download_queue.lock();
        queue.push_back(task);
    }

    pub fn skip_current_file(&self) -> bool {
        let mut queue = self.download_queue.lock();
        queue.pop_front().is_some()
    }

    pub fn cancel_export_fast(&self) {
        *self.is_cancelled.lock() = true;
        let mut queue = self.download_queue.lock();
        queue.clear();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.is_cancelled.lock()
    }

    pub fn pending_files_count(&self) -> usize {
        self.download_queue.lock().len()
    }

    pub fn poll_next_download(&self) -> Option<ExportFileTask> {
        let mut active = self.active_downloads.lock();
        if *active >= self.max_concurrent_downloads {
            return None;
        }
        let mut queue = self.download_queue.lock();
        if let Some(task) = queue.pop_front() {
            *active += 1;
            Some(task)
        } else {
            None
        }
    }

    pub fn finish_download(&self, file_id: u64, local_path: String) {
        let mut active = self.active_downloads.lock();
        if *active > 0 {
            *active -= 1;
        }
        self.file_cache.lock().insert(file_id, local_path);
    }
}
