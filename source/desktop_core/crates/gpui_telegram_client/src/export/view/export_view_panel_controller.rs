//! Target Symbol: ExportViewPanelControllerView
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/view/export_view_panel_controller.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::export::export_controller::ExportController;
use crate::export::export_settings::ExportSettings;
use crate::export::view::export_view_progress::ExportViewProgressView;
use crate::export::view::export_view_settings::ExportViewSettingsView;
use crate::export::view::export_view_top_bar::ExportViewTopBarView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PanelScreen {
    #[default]
    Settings,
    Progress,
    SuggestPrompt,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestBoxModal {
    pub title: String,
    pub text: String,
    pub is_active: bool,
}

/// Separate desktop window panel coordinator matching upstream `Export::View::PanelController`
pub struct ExportViewPanelControllerView {
    pub top_bar: Arc<ExportViewTopBarView>,
    pub settings_view: Arc<ExportViewSettingsView>,
    pub progress_view: Arc<ExportViewProgressView>,
    current_screen: Mutex<PanelScreen>,
    suggest_box: Mutex<Option<SuggestBoxModal>>,
    error_message: Mutex<Option<String>>,
    pub panel_width: f32,
    pub panel_height: f32,
}

impl Default for ExportViewPanelControllerView {
    fn default() -> Self {
        Self::new(ExportSettings::default())
    }
}

impl ExportViewPanelControllerView {
    pub fn new(settings: ExportSettings) -> Self {
        Self {
            top_bar: Arc::new(ExportViewTopBarView::new("Export Telegram Data")),
            settings_view: Arc::new(ExportViewSettingsView::new(settings)),
            progress_view: Arc::new(ExportViewProgressView::new()),
            current_screen: Mutex::new(PanelScreen::Settings),
            suggest_box: Mutex::new(None),
            error_message: Mutex::new(None),
            panel_width: 380.0,
            panel_height: 460.0,
        }
    }

    pub fn show_settings(&self) {
        *self.current_screen.lock() = PanelScreen::Settings;
        self.top_bar.set_title("Export Telegram Data");
    }

    pub fn show_progress(&self) {
        *self.current_screen.lock() = PanelScreen::Progress;
        self.top_bar.set_title("Exporting Data");
    }

    pub fn show_error(&self, message: impl Into<String>) {
        let msg = message.into();
        *self.error_message.lock() = Some(msg);
        *self.current_screen.lock() = PanelScreen::Error;
    }

    pub fn show_suggest_prompt(&self, title: impl Into<String>, text: impl Into<String>) {
        *self.suggest_box.lock() = Some(SuggestBoxModal {
            title: title.into(),
            text: text.into(),
            is_active: true,
        });
        *self.current_screen.lock() = PanelScreen::SuggestPrompt;
    }

    pub fn clear_suggest_prompt(&self) {
        *self.suggest_box.lock() = None;
        if *self.current_screen.lock() == PanelScreen::SuggestPrompt {
            *self.current_screen.lock() = PanelScreen::Settings;
        }
    }

    pub fn current_screen(&self) -> PanelScreen {
        *self.current_screen.lock()
    }

    pub fn is_default_path(path: &Path) -> bool {
        let s = path.to_string_lossy();
        s.starts_with("Telegram Export")
    }
}
