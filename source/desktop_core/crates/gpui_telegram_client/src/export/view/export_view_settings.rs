//! Target Symbol: ExportViewSettingsView
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/view/export_view_settings.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::PathBuf;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_settings::{
    ChatTypeFlags, ExportFormat, ExportMediaSettings, ExportSettings, ExportTypeFlags, MediaTypeFlags,
};

/// Preset size limits in bytes matching upstream `kSizeLimits` in `export_view_settings.cpp`
pub const SIZE_LIMITS: &[i64] = &[
    1 * 1024 * 1024,       // 1 MB
    3 * 1024 * 1024,       // 3 MB
    5 * 1024 * 1024,       // 5 MB
    10 * 1024 * 1024,      // 10 MB
    50 * 1024 * 1024,      // 50 MB
    100 * 1024 * 1024,     // 100 MB
    500 * 1024 * 1024,     // 500 MB
    1000 * 1024 * 1024,    // 1000 MB
    1500 * 1024 * 1024,    // 1500 MB
    2000 * 1024 * 1024,    // 2000 MB
    4000 * 1024 * 1024,    // 4000 MB (Premium)
];

/// Interactive settings view model matching upstream `Export::View::SettingsWidget`
#[derive(Debug)]
pub struct ExportViewSettingsView {
    settings: Mutex<ExportSettings>,
    selected_size_index: Mutex<usize>,
}

impl Default for ExportViewSettingsView {
    fn default() -> Self {
        Self::new(ExportSettings::default())
    }
}

impl ExportViewSettingsView {
    pub fn new(settings: ExportSettings) -> Self {
        let size = settings.media.size_limit;
        let idx = SIZE_LIMITS
            .iter()
            .position(|&s| s >= size)
            .unwrap_or(SIZE_LIMITS.len() - 1);

        Self {
            settings: Mutex::new(settings),
            selected_size_index: Mutex::new(idx),
        }
    }

    pub fn size_limit_by_index(index: usize) -> i64 {
        if index < SIZE_LIMITS.len() {
            SIZE_LIMITS[index]
        } else {
            *SIZE_LIMITS.last().unwrap()
        }
    }

    pub fn set_size_limit_index(&self, index: usize) {
        let clamped = index.min(SIZE_LIMITS.len() - 1);
        *self.selected_size_index.lock() = clamped;
        let limit = Self::size_limit_by_index(clamped);
        self.settings.lock().media.size_limit = limit;
    }

    pub fn set_format(&self, format: ExportFormat) {
        self.settings.lock().format = format;
    }

    pub fn toggle_type_flag(&self, flag: u32, enabled: bool) {
        self.settings.lock().types.set(flag, enabled);
    }

    pub fn toggle_media_flag(&self, flag: u32, enabled: bool) {
        let mut st = self.settings.lock();
        if enabled {
            st.media.types.0 |= flag;
        } else {
            st.media.types.0 &= !flag;
        }
    }

    pub fn set_output_path(&self, path: impl Into<PathBuf>) {
        self.settings.lock().path = path.into();
    }

    pub fn snapshot(&self) -> ExportSettings {
        self.settings.lock().clone()
    }

    pub fn is_valid(&self) -> bool {
        self.settings.lock().validate()
    }
}
