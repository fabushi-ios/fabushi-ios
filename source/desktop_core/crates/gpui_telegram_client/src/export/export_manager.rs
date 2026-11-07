//! Target Symbol: ExportManager
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/export_manager.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::sync::Arc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_controller::ExportController;
use crate::export::export_settings::ExportSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportManagerView {
    #[default]
    None,
    Settings,
    Progress,
    Done,
}

/// Global coordinator for export sessions matching upstream `Export::Manager`
pub struct ExportManager {
    controller: Mutex<Option<Arc<ExportController>>>,
    current_view: Mutex<ExportManagerView>,
}

impl Default for ExportManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportManager {
    pub fn new() -> Self {
        Self {
            controller: Mutex::new(None),
            current_view: Mutex::new(ExportManagerView::None),
        }
    }

    pub fn start(&self, settings: ExportSettings) -> Arc<ExportController> {
        let controller = Arc::new(ExportController::new(settings));
        controller.start_export();
        *self.controller.lock() = Some(controller.clone());
        *self.current_view.lock() = ExportManagerView::Progress;
        controller
    }

    pub fn start_topic(&self, peer_id: i64, topic_id: i32, mut settings: ExportSettings) -> Arc<ExportController> {
        settings.single_peer_id = Some(peer_id);
        settings.single_topic_id = Some(topic_id);
        self.start(settings)
    }

    pub fn in_progress(&self) -> bool {
        if let Some(ref c) = *self.controller.lock() {
            c.is_active()
        } else {
            false
        }
    }

    pub fn stop(&self) {
        if let Some(ref c) = *self.controller.lock() {
            c.cancel();
        }
        *self.controller.lock() = None;
        *self.current_view.lock() = ExportManagerView::None;
    }

    pub fn stop_with_confirmation<F: FnOnce() -> bool>(&self, confirm: F) -> bool {
        if self.in_progress() {
            if confirm() {
                self.stop();
                true
            } else {
                false
            }
        } else {
            self.stop();
            true
        }
    }

    pub fn current_view(&self) -> ExportManagerView {
        *self.current_view.lock()
    }

    pub fn active_controller(&self) -> Option<Arc<ExportController>> {
        self.controller.lock().clone()
    }
}
