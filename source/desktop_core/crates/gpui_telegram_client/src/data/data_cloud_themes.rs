//! Target Symbol: DataCloudThemes
//! Telegram Desktop 1:1 Submodule: `DataCloudThemes`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudThemeType {
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CloudThemeSettings {
    pub accent_color: u32,
    pub outgoing_accent_color: Option<u32>,
    pub outgoing_message_colors: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CloudTheme {
    pub id: u64,
    pub access_hash: u64,
    pub slug: String,
    pub title: String,
    pub emoticon: String,
    pub light: Option<CloudThemeSettings>,
    pub dark: Option<CloudThemeSettings>,
}

/// Target Symbol for Module 03 Ledger: DataCloudThemes coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataCloudThemes {
    pub themes: Vec<CloudTheme>,
}

impl DataCloudThemes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, theme: CloudTheme) {
        if let Some(pos) = self.themes.iter().position(|t| t.id == theme.id) {
            self.themes[pos] = theme;
        } else {
            self.themes.push(theme);
        }
    }

    pub fn find_by_slug(&self, slug: &str) -> Option<&CloudTheme> {
        self.themes.iter().find(|t| t.slug == slug)
    }

    pub fn count(&self) -> usize {
        self.themes.len()
    }
}
