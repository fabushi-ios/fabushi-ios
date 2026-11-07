//! Target Symbol: ExportViewProgressView
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/view/export_view_progress.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::sync::Arc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_controller::{ExportProcessingState, ExportState};
use crate::export::export_settings::ExportSettings;
use crate::export::output::export_output_stats::ExportStatsSnapshot;
use crate::export::view::export_view_content::ExportViewContentView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProgressAction {
    #[default]
    None,
    SkipFile,
    Cancel,
    Done,
}

/// Reactive desktop view component for export progress matching upstream `Export::View::ProgressWidget`
#[derive(Debug, Default)]
pub struct ExportViewProgressView {
    pub content: Mutex<ExportViewContentView>,
    pub is_finished: Mutex<bool>,
    pub stats_summary: Mutex<Option<ExportStatsSnapshot>>,
    pub last_action: Mutex<ProgressAction>,
}

impl ExportViewProgressView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_from_state(&self, state: &ExportState, settings: &ExportSettings) {
        match state {
            ExportState::Processing(proc) => {
                let view_content = ExportViewContentView::content_from_state(proc, settings);
                *self.content.lock() = view_content;
                *self.is_finished.lock() = false;
            }
            ExportState::Finished(snapshot) => {
                *self.is_finished.lock() = true;
                *self.stats_summary.lock() = Some(*snapshot);
            }
            _ => {}
        }
    }

    pub fn on_skip_file(&self) {
        *self.last_action.lock() = ProgressAction::SkipFile;
    }

    pub fn on_cancel(&self) {
        *self.last_action.lock() = ProgressAction::Cancel;
    }

    pub fn on_done(&self) {
        *self.last_action.lock() = ProgressAction::Done;
    }

    pub fn current_percent(&self) -> u8 {
        self.content.lock().overall_percent
    }

    pub fn is_done(&self) -> bool {
        *self.is_finished.lock()
    }
}
