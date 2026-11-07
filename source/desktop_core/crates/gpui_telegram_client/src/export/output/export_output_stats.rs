//! Target Symbol: ExportOutputStatsOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_stats.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::sync::atomic::{AtomicI64, Ordering};
use serde::{Deserialize, Serialize};

/// Cumulative export volume statistics matching upstream `Export::Output::Stats`
#[derive(Debug, Default)]
pub struct ExportOutputStatsOutput {
    files_count: AtomicI64,
    bytes_count: AtomicI64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportStatsSnapshot {
    pub files_count: i64,
    pub bytes_count: i64,
}

impl ExportOutputStatsOutput {
    pub fn new() -> Self {
        Self {
            files_count: AtomicI64::new(0),
            bytes_count: AtomicI64::new(0),
        }
    }

    pub fn increment_files(&self) {
        self.files_count.fetch_add(1, Ordering::SeqCst);
    }

    pub fn increment_bytes(&self, bytes: i64) {
        if bytes > 0 {
            self.bytes_count.fetch_add(bytes, Ordering::SeqCst);
        }
    }

    pub fn files_count(&self) -> i64 {
        self.files_count.load(Ordering::SeqCst)
    }

    pub fn bytes_count(&self) -> i64 {
        self.bytes_count.load(Ordering::SeqCst)
    }

    pub fn snapshot(&self) -> ExportStatsSnapshot {
        ExportStatsSnapshot {
            files_count: self.files_count(),
            bytes_count: self.bytes_count(),
        }
    }

    pub fn formatted_bytes(&self) -> String {
        let b = self.bytes_count();
        if b < 1024 {
            format!("{b} B")
        } else if b < 1024 * 1024 {
            format!("{:.1} KB", b as f64 / 1024.0)
        } else if b < 1024 * 1024 * 1024 {
            format!("{:.2} MB", b as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}
