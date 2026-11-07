//! Target Symbol: ExportViewContentView
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/view/export_view_content.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use serde::{Deserialize, Serialize};
use crate::export::export_controller::{ExportProcessingState, ExportStep};
use crate::export::export_settings::ExportSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProgressRowState {
    #[default]
    Waiting,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressRow {
    pub step: ExportStep,
    pub title: String,
    pub description: String,
    pub state: ProgressRowState,
    pub progress_percent: u8,
}

/// View model capturing export progress rows matching upstream `Export::View::Content`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportViewContentView {
    pub rows: Vec<ProgressRow>,
    pub overall_percent: u8,
}

impl ExportViewContentView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn format_download_text(ready_bytes: i64, total_bytes: i64) -> String {
        let fmt = |b: i64| -> String {
            if b < 1024 {
                format!("{b} B")
            } else if b < 1024 * 1024 {
                format!("{:.1} KB", b as f64 / 1024.0)
            } else if b < 1024 * 1024 * 1024 {
                format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
            } else {
                format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0))
            }
        };

        if total_bytes > 0 {
            format!("{} / {}", fmt(ready_bytes), fmt(total_bytes))
        } else {
            fmt(ready_bytes)
        }
    }

    pub fn content_from_state(state: &ExportProcessingState, settings: &ExportSettings) -> Self {
        let step_title = match state.step {
            ExportStep::Initializing => "Initializing export...",
            ExportStep::DialogsList => "Loading chat list...",
            ExportStep::PersonalInfo => "Exporting account information...",
            ExportStep::Userpics => "Downloading profile photos...",
            ExportStep::Stories => "Exporting user stories...",
            ExportStep::ProfileMusic => "Exporting profile music...",
            ExportStep::Contacts => "Exporting contacts list...",
            ExportStep::Sessions => "Exporting active sessions...",
            ExportStep::OtherData => "Exporting other account data...",
            ExportStep::Chats => "Exporting chats...",
            ExportStep::Topics => "Exporting forum topics...",
            ExportStep::Messages => "Exporting chat messages...",
            ExportStep::Media => "Downloading chat media files...",
            ExportStep::Finishing => "Finalizing export files...",
        };

        let description = if state.bytes_total > 0 {
            Self::format_download_text(state.bytes_ready, state.bytes_total)
        } else if state.item_total > 0 {
            format!("{} / {}", state.item_index, state.item_total)
        } else {
            String::new()
        };

        let pct = if state.bytes_total > 0 {
            ((state.bytes_ready as f64 / state.bytes_total as f64) * 100.0).clamp(0.0, 100.0) as u8
        } else if state.item_total > 0 {
            ((state.item_index as f64 / state.item_total as f64) * 100.0).clamp(0.0, 100.0) as u8
        } else {
            0
        };

        let row = ProgressRow {
            step: state.step,
            title: step_title.to_string(),
            description,
            state: ProgressRowState::InProgress,
            progress_percent: pct,
        };

        Self {
            rows: vec![row],
            overall_percent: pct,
        }
    }
}
