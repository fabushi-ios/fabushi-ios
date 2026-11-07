//! Target Symbol: ExportOutputJsonOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_json.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use serde_json::json;
use crate::export::export_settings::ExportFormat;
use crate::export::output::export_output_abstract::{
    AbstractOutputWriter, ExportContactItem, ExportDialogItem, ExportMessageItem,
    ExportPersonalInfo, ExportSessionItem, ExportUserpicItem,
};
use crate::export::output::export_output_result::ExportOutputResultOutput;
use crate::export::output::export_output_stats::ExportOutputStatsOutput;

/// Complete JSON output generator matching upstream `Export::Output::JsonWriter`
pub struct ExportOutputJsonOutput {
    pub base_path: PathBuf,
    pub stats: ExportOutputStatsOutput,
}

impl Default for ExportOutputJsonOutput {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("Telegram_Export"),
            stats: ExportOutputStatsOutput::new(),
        }
    }
}

impl ExportOutputJsonOutput {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            stats: ExportOutputStatsOutput::new(),
        }
    }

    fn write_json_file(&self, relative_path: &str, value: &serde_json::Value) -> ExportOutputResultOutput {
        let full_path = self.base_path.join(relative_path);
        if let Some(parent) = full_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return ExportOutputResultOutput::error(format!("Failed to create directory: {e}"));
            }
        }
        match serde_json::to_string_pretty(value) {
            Ok(content) => match File::create(&full_path) {
                Ok(mut f) => match f.write_all(content.as_bytes()) {
                    Ok(_) => {
                        self.stats.increment_files();
                        self.stats.increment_bytes(content.len() as i64);
                        ExportOutputResultOutput::success().with_file_path(full_path.to_string_lossy().to_string())
                    }
                    Err(e) => ExportOutputResultOutput::error(format!("Failed to write JSON file: {e}")),
                },
                Err(e) => ExportOutputResultOutput::error(format!("Failed to create JSON file: {e}")),
            },
            Err(e) => ExportOutputResultOutput::error(format!("Failed to serialize JSON: {e}")),
        }
    }
}

impl AbstractOutputWriter for ExportOutputJsonOutput {
    fn format(&self) -> ExportFormat {
        ExportFormat::Json
    }

    fn start(&mut self) -> ExportOutputResultOutput {
        ExportOutputResultOutput::success()
    }

    fn finish(&mut self) -> ExportOutputResultOutput {
        ExportOutputResultOutput::success()
    }

    fn write_personal(&mut self, info: &ExportPersonalInfo) -> ExportOutputResultOutput {
        let val = json!({
            "about": "Telegram User Personal Information",
            "user_id": info.user_id,
            "first_name": info.first_name,
            "last_name": info.last_name,
            "phone_number": info.phone_number,
            "username": info.username,
            "bio": info.bio,
        });
        self.write_json_file("about.json", &val)
    }

    fn write_userpics_slice(&mut self, items: &[ExportUserpicItem]) -> ExportOutputResultOutput {
        let list: Vec<serde_json::Value> = items
            .iter()
            .map(|i| {
                json!({
                    "photo_id": i.photo_id,
                    "date": i.date,
                    "file_path": i.file_path,
                    "width": i.width,
                    "height": i.height,
                })
            })
            .collect();
        let val = json!({
            "about": "Telegram Profile Pictures",
            "photos": list,
        });
        self.write_json_file("userpics.json", &val)
    }

    fn write_contacts_list(&mut self, contacts: &[ExportContactItem]) -> ExportOutputResultOutput {
        let list: Vec<serde_json::Value> = contacts
            .iter()
            .map(|c| {
                json!({
                    "user_id": c.user_id,
                    "first_name": c.first_name,
                    "last_name": c.last_name,
                    "phone_number": c.phone_number,
                    "date": c.date,
                })
            })
            .collect();
        let val = json!({
            "about": "Telegram Contacts List",
            "contacts": list,
        });
        self.write_json_file("contacts.json", &val)
    }

    fn write_sessions_list(&mut self, sessions: &[ExportSessionItem]) -> ExportOutputResultOutput {
        let list: Vec<serde_json::Value> = sessions
            .iter()
            .map(|s| {
                json!({
                    "hash": s.hash,
                    "app_name": s.app_name,
                    "app_version": s.app_version,
                    "device_model": s.device_model,
                    "platform": s.platform,
                    "system_version": s.system_version,
                    "ip": s.ip,
                    "country": s.country,
                    "date_active": s.date_active,
                    "date_created": s.date_created,
                })
            })
            .collect();
        let val = json!({
            "about": "Telegram Active Sessions",
            "sessions": list,
        });
        self.write_json_file("sessions.json", &val)
    }

    fn write_dialogs_list(&mut self, dialogs: &[ExportDialogItem]) -> ExportOutputResultOutput {
        let list: Vec<serde_json::Value> = dialogs
            .iter()
            .map(|d| {
                json!({
                    "id": d.peer_id,
                    "name": d.name,
                    "type": d.dialog_type,
                    "messages_count": d.messages_count,
                })
            })
            .collect();
        let val = json!({
            "about": "Telegram Chat List Export",
            "chats": {
                "list": list
            }
        });
        self.write_json_file("result.json", &val)
    }

    fn write_messages_slice(&mut self, peer_id: i64, messages: &[ExportMessageItem]) -> ExportOutputResultOutput {
        let list: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "type": "message",
                    "date_unixtime": m.date,
                    "from_id": m.from_id,
                    "from": m.from_name,
                    "text": m.text,
                    "media_type": m.media_type,
                    "file": m.media_path,
                    "reply_to_message_id": m.reply_to_id,
                })
            })
            .collect();
        let val = json!({
            "id": peer_id,
            "messages": list,
        });
        let path = format!("chats/chat_{peer_id}/result.json");
        self.write_json_file(&path, &val)
    }

    fn write_other_data(&mut self, relative_path: &str, content: &[u8]) -> ExportOutputResultOutput {
        let full_path = self.base_path.join(relative_path);
        if let Some(parent) = full_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut f) = File::create(&full_path) {
            let _ = f.write_all(content);
            self.stats.increment_files();
            self.stats.increment_bytes(content.len() as i64);
            ExportOutputResultOutput::success()
        } else {
            ExportOutputResultOutput::error("Failed to write other data")
        }
    }
}
