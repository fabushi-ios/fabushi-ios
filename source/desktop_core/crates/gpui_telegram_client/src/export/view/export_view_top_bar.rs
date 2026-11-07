//! Target Symbol: ExportViewTopBarView
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/view/export_view_top_bar.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TopBarAction {
    #[default]
    None,
    Back,
    Close,
    Minimize,
}

/// Top navigation bar view model matching upstream `Export::View::TopBar`
#[derive(Debug)]
pub struct ExportViewTopBarView {
    pub title: Mutex<String>,
    pub shadow_visible: Mutex<bool>,
    pub last_action: Mutex<TopBarAction>,
    pub height: f32,
}

impl Default for ExportViewTopBarView {
    fn default() -> Self {
        Self {
            title: Mutex::new("Export Data".to_string()),
            shadow_visible: Mutex::new(true),
            last_action: Mutex::new(TopBarAction::None),
            height: 48.0,
        }
    }
}

impl ExportViewTopBarView {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Mutex::new(title.into()),
            shadow_visible: Mutex::new(true),
            last_action: Mutex::new(TopBarAction::None),
            height: 48.0,
        }
    }

    pub fn set_title(&self, title: impl Into<String>) {
        *self.title.lock() = title.into();
    }

    pub fn show_shadow(&self) {
        *self.shadow_visible.lock() = true;
    }

    pub fn hide_shadow(&self) {
        *self.shadow_visible.lock() = false;
    }

    pub fn on_close(&self) {
        *self.last_action.lock() = TopBarAction::Close;
    }

    pub fn on_back(&self) {
        *self.last_action.lock() = TopBarAction::Back;
    }

    pub fn on_minimize(&self) {
        *self.last_action.lock() = TopBarAction::Minimize;
    }

    pub fn current_title(&self) -> String {
        self.title.lock().clone()
    }

    pub fn is_shadow_visible(&self) -> bool {
        *self.shadow_visible.lock()
    }
}
