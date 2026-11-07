//! Target Symbol: ExportOutputAbstractOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_abstract.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::export::export_settings::ExportFormat;
use crate::export::output::export_output_result::ExportOutputResultOutput;
use crate::export::output::export_output_stats::ExportOutputStatsOutput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportPersonalInfo {
    pub user_id: i64,
    pub first_name: String,
    pub last_name: String,
    pub phone_number: String,
    pub username: Option<String>,
    pub bio: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportUserpicItem {
    pub photo_id: i64,
    pub date: i64,
    pub file_path: String,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportContactItem {
    pub user_id: i64,
    pub first_name: String,
    pub last_name: String,
    pub phone_number: String,
    pub date: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportSessionItem {
    pub hash: i64,
    pub app_name: String,
    pub app_version: String,
    pub device_model: String,
    pub platform: String,
    pub system_version: String,
    pub ip: String,
    pub country: String,
    pub date_active: i64,
    pub date_created: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportDialogItem {
    pub peer_id: i64,
    pub name: String,
    pub dialog_type: String, // "personal_chat", "bot_chat", "private_group", "channel"
    pub messages_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportMessageEntity {
    pub entity_type: String, // "bold", "italic", "code", "link", etc.
    pub offset: usize,
    pub length: usize,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExportMessageItem {
    pub id: i64,
    pub peer_id: i64,
    pub from_id: i64,
    pub from_name: String,
    pub date: i64,
    pub text: String,
    pub entities: Vec<ExportMessageEntity>,
    pub media_path: Option<String>,
    pub media_type: Option<String>,
    pub reply_to_id: Option<i64>,
}

/// Abstract contract for serialization engines (HTML, JSON)
pub trait AbstractOutputWriter: Send + Sync {
    fn format(&self) -> ExportFormat;
    fn start(&mut self) -> ExportOutputResultOutput;
    fn finish(&mut self) -> ExportOutputResultOutput;
    fn write_personal(&mut self, info: &ExportPersonalInfo) -> ExportOutputResultOutput;
    fn write_userpics_slice(&mut self, items: &[ExportUserpicItem]) -> ExportOutputResultOutput;
    fn write_contacts_list(&mut self, contacts: &[ExportContactItem]) -> ExportOutputResultOutput;
    fn write_sessions_list(&mut self, sessions: &[ExportSessionItem]) -> ExportOutputResultOutput;
    fn write_dialogs_list(&mut self, dialogs: &[ExportDialogItem]) -> ExportOutputResultOutput;
    fn write_messages_slice(&mut self, peer_id: i64, messages: &[ExportMessageItem]) -> ExportOutputResultOutput;
    fn write_other_data(&mut self, relative_path: &str, content: &[u8]) -> ExportOutputResultOutput;
}

/// Base container and factory for export output writer matching upstream `Export::Output::AbstractWriter`
#[derive(Debug, Clone)]
pub struct ExportOutputAbstractOutput {
    pub base_path: PathBuf,
    pub format: ExportFormat,
}

impl Default for ExportOutputAbstractOutput {
    fn default() -> Self {
        Self {
            base_path: Self::default_export_folder(),
            format: ExportFormat::Html,
        }
    }
}

impl ExportOutputAbstractOutput {
    pub fn new(base_path: impl Into<PathBuf>, format: ExportFormat) -> Self {
        Self {
            base_path: Self::normalize_path(base_path.into()),
            format,
        }
    }

    pub fn default_export_folder() -> PathBuf {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        PathBuf::from(format!("Telegram Export ({date})"))
    }

    pub fn normalize_path(path: PathBuf) -> PathBuf {
        let s = path.to_string_lossy();
        let trimmed = s.trim_end_matches(['/', '\\']);
        PathBuf::from(trimmed)
    }
}
