//! Target Symbol: Localstorage
//! Telegram Desktop 1:1 Submodule: `Localstorage`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::PathBuf;
use super::details::storage_file_utilities::{read_file, FileWriteDescriptor};
use super::details::storage_settings_scheme::{
    ReadSettingsContext, SettingsSchemeWriter, StorageSettingsSchemeDetails,
    DBI_APPLICATION_SETTINGS, DBI_BACKGROUND_KEY, DBI_THEME_KEY,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalLanguage {
    pub id: String,
    pub name: String,
    pub native_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalWallpaper {
    pub id: u64,
    pub path: String,
    pub is_tiled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalTheme {
    pub day_key: u64,
    pub night_key: u64,
    pub is_night_mode: bool,
}

#[derive(Debug, Default)]
pub struct Localstorage {
    pub base_path: PathBuf,
    settings_context: ReadSettingsContext,
    autoupdate_prefix: String,
    update_manifest: Option<(Vec<u8>, Vec<u8>)>,
    wallpaper: Option<LocalWallpaper>,
    theme: LocalTheme,
    recent_languages: Vec<LocalLanguage>,
}

impl Localstorage {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            settings_context: ReadSettingsContext::new(),
            autoupdate_prefix: String::new(),
            update_manifest: None,
            wallpaper: None,
            theme: LocalTheme::default(),
            recent_languages: Vec::new(),
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.read_settings()?;
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), String> {
        self.write_settings()
    }

    pub fn finish(&mut self) -> Result<(), String> {
        self.sync()
    }

    pub fn write_settings(&self) -> Result<(), String> {
        let mut writer = SettingsSchemeWriter::new();
        if !self.settings_context.application_settings_raw.is_empty() {
            writer.write_block(
                DBI_APPLICATION_SETTINGS,
                &self.settings_context.application_settings_raw,
            );
        }
        if self.theme.day_key != 0 || self.theme.night_key != 0 {
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&self.theme.day_key.to_le_bytes());
            buf[8..16].copy_from_slice(&self.theme.night_key.to_le_bytes());
            writer.write_block(DBI_THEME_KEY, &buf);
        }
        if let Some(wp) = &self.wallpaper {
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&wp.id.to_le_bytes());
            writer.write_block(DBI_BACKGROUND_KEY, &buf);
        }

        let payload = writer.finish();
        let base_str = self.base_path.to_string_lossy().to_string();
        let mut descriptor = FileWriteDescriptor::new("settings", base_str, true);
        descriptor.write_byte_array(&payload);
        descriptor.finish()
    }

    pub fn read_settings(&mut self) -> Result<ReadSettingsContext, String> {
        let base_str = self.base_path.to_string_lossy().to_string();
        let Ok(mut descriptor) = read_file("settings", &base_str) else {
            return Ok(ReadSettingsContext::new());
        };

        if let Some(payload) = descriptor.read_byte_array() {
            let mut parser = StorageSettingsSchemeDetails::new();
            let ctx = parser.parse_settings(&payload, descriptor.version);
            self.settings_context = ctx.clone();
            Ok(ctx)
        } else {
            Ok(ReadSettingsContext::new())
        }
    }

    pub fn write_autoupdate_prefix(&mut self, prefix: String) {
        self.autoupdate_prefix = prefix;
    }

    pub fn read_autoupdate_prefix(&self) -> &str {
        &self.autoupdate_prefix
    }

    pub fn write_update_manifest(&mut self, manifest: Vec<u8>, signature: Vec<u8>) {
        self.update_manifest = Some((manifest, signature));
    }

    pub fn read_update_manifest(&self) -> Option<(&[u8], &[u8])> {
        self.update_manifest
            .as_ref()
            .map(|(m, s)| (m.as_slice(), s.as_slice()))
    }

    pub fn write_background(&mut self, wallpaper: LocalWallpaper) {
        self.wallpaper = Some(wallpaper);
    }

    pub fn read_background(&self) -> Option<&LocalWallpaper> {
        self.wallpaper.as_ref()
    }

    pub fn write_theme(&mut self, theme: LocalTheme) {
        self.theme = theme;
    }

    pub fn read_theme(&self) -> &LocalTheme {
        &self.theme
    }

    pub fn push_recent_language(&mut self, language: LocalLanguage) {
        if let Some(pos) = self.recent_languages.iter().position(|l| l.id == language.id) {
            self.recent_languages.remove(pos);
        }
        self.recent_languages.insert(0, language);
    }

    pub fn read_recent_languages(&self) -> &[LocalLanguage] {
        &self.recent_languages
    }

    pub fn remove_recent_language(&mut self, id: &str) {
        self.recent_languages.retain(|l| l.id != id);
    }

    pub fn set_application_settings_raw(&mut self, data: Vec<u8>) {
        self.settings_context.application_settings_raw = data;
    }

    pub fn application_settings_raw(&self) -> &[u8] {
        &self.settings_context.application_settings_raw
    }
}
