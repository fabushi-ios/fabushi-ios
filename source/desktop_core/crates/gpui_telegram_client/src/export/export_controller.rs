//! Target Symbol: ExportController
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/export_controller.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::sync::Arc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_settings::ExportSettings;
use crate::export::output::export_output_stats::ExportStatsSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExportStep {
    #[default]
    Initializing,
    DialogsList,
    PersonalInfo,
    Userpics,
    Stories,
    ProfileMusic,
    Contacts,
    Sessions,
    OtherData,
    Chats,
    Topics,
    Messages,
    Media,
    Finishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExportEntityType {
    #[default]
    Chat,
    SavedMessages,
    RepliesMessages,
    VerifyCodes,
    Topic,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportProcessingState {
    pub step: ExportStep,
    pub entity_type: ExportEntityType,
    pub item_index: usize,
    pub item_total: usize,
    pub bytes_ready: i64,
    pub bytes_total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPasswordState {
    pub has_password: bool,
    pub hint: Option<String>,
    pub has_recovery: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportState {
    Idle,
    PasswordRequired(ExportPasswordState),
    Processing(ExportProcessingState),
    Cancelled,
    Failed(String),
    Finished(ExportStatsSnapshot),
}

/// Export session orchestrator managing state transitions matching upstream `Export::Controller`
pub struct ExportController {
    settings: ExportSettings,
    state: Mutex<ExportState>,
    steps_pipeline: Vec<ExportStep>,
    current_step_index: Mutex<usize>,
}

impl Default for ExportController {
    fn default() -> Self {
        Self::new(ExportSettings::default())
    }
}

impl ExportController {
    pub fn new(settings: ExportSettings) -> Self {
        let mut steps = vec![ExportStep::Initializing];
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::PERSONAL_INFO) {
            steps.push(ExportStep::PersonalInfo);
        }
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::USERPICS) {
            steps.push(ExportStep::Userpics);
        }
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::CONTACTS) {
            steps.push(ExportStep::Contacts);
        }
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::SESSIONS) {
            steps.push(ExportStep::Sessions);
        }
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::OTHER_DATA) {
            steps.push(ExportStep::OtherData);
        }
        if settings.types.contains(crate::export::export_settings::ExportTypeFlags::DIALOGS) {
            steps.push(ExportStep::DialogsList);
            steps.push(ExportStep::Chats);
            steps.push(ExportStep::Messages);
            steps.push(ExportStep::Media);
        }
        steps.push(ExportStep::Finishing);

        Self {
            settings,
            state: Mutex::new(ExportState::Idle),
            steps_pipeline: steps,
            current_step_index: Mutex::new(0),
        }
    }

    pub fn start_export(&self) {
        *self.current_step_index.lock() = 0;
        let initial_step = self.steps_pipeline.first().copied().unwrap_or(ExportStep::Initializing);
        *self.state.lock() = ExportState::Processing(ExportProcessingState {
            step: initial_step,
            entity_type: ExportEntityType::Other,
            item_index: 0,
            item_total: 1,
            bytes_ready: 0,
            bytes_total: 0,
        });
    }

    pub fn advance_step(&self) -> bool {
        let mut idx = self.current_step_index.lock();
        if *idx + 1 < self.steps_pipeline.len() {
            *idx += 1;
            let next_step = self.steps_pipeline[*idx];
            *self.state.lock() = ExportState::Processing(ExportProcessingState {
                step: next_step,
                entity_type: ExportEntityType::Chat,
                item_index: 0,
                item_total: 1,
                bytes_ready: 0,
                bytes_total: 0,
            });
            true
        } else {
            false
        }
    }

    pub fn update_progress(&self, item_index: usize, item_total: usize, bytes_ready: i64, bytes_total: i64) {
        let mut st = self.state.lock();
        if let ExportState::Processing(ref mut p) = *st {
            p.item_index = item_index;
            p.item_total = item_total;
            p.bytes_ready = bytes_ready;
            p.bytes_total = bytes_total;
        }
    }

    pub fn submit_password(&self, password: &str) -> bool {
        if password.is_empty() {
            return false;
        }
        self.start_export();
        true
    }

    pub fn cancel(&self) {
        *self.state.lock() = ExportState::Cancelled;
    }

    pub fn finish(&self, snapshot: ExportStatsSnapshot) {
        *self.state.lock() = ExportState::Finished(snapshot);
    }

    pub fn fail(&self, message: impl Into<String>) {
        *self.state.lock() = ExportState::Failed(message.into());
    }

    pub fn current_state(&self) -> ExportState {
        self.state.lock().clone()
    }

    pub fn is_active(&self) -> bool {
        matches!(*self.state.lock(), ExportState::Processing(_))
    }
}
