//! Target Symbol: ExportOutputHtmlOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_html.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use chrono::Utc;
use crate::export::export_settings::ExportFormat;
use crate::export::output::export_output_abstract::{
    AbstractOutputWriter, ExportContactItem, ExportDialogItem, ExportMessageItem,
    ExportPersonalInfo, ExportSessionItem, ExportUserpicItem,
};
use crate::export::output::export_output_result::ExportOutputResultOutput;
use crate::export::output::export_output_stats::ExportOutputStatsOutput;

/// Complete HTML output generator matching upstream `Export::Output::HtmlWriter`
pub struct ExportOutputHtmlOutput {
    pub base_path: PathBuf,
    pub stats: ExportOutputStatsOutput,
}

impl Default for ExportOutputHtmlOutput {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("Telegram_Export"),
            stats: ExportOutputStatsOutput::new(),
        }
    }
}

impl ExportOutputHtmlOutput {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            stats: ExportOutputStatsOutput::new(),
        }
    }

    fn write_html_file(&self, relative_path: &str, content: &str) -> ExportOutputResultOutput {
        let full_path = self.base_path.join(relative_path);
        if let Some(parent) = full_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return ExportOutputResultOutput::error(format!("Failed to create directory: {e}"));
            }
        }
        match File::create(&full_path) {
            Ok(mut f) => match f.write_all(content.as_bytes()) {
                Ok(_) => {
                    self.stats.increment_files();
                    self.stats.increment_bytes(content.len() as i64);
                    ExportOutputResultOutput::success().with_file_path(full_path.to_string_lossy().to_string())
                }
                Err(e) => ExportOutputResultOutput::error(format!("Failed to write HTML file: {e}")),
            },
            Err(e) => ExportOutputResultOutput::error(format!("Failed to create HTML file: {e}")),
        }
    }

    pub fn generate_css() -> &'static str {
        r#"
body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #eef2f5; color: #222; margin: 0; padding: 20px; }
.container { max-width: 800px; margin: 0 auto; background: #fff; border-radius: 8px; padding: 24px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }
h1, h2 { color: #1c7bb8; }
.message { border-bottom: 1px solid #f0f0f0; padding: 8px 0; }
.message .from { font-weight: bold; color: #3388bb; }
.message .date { float: right; font-size: 12px; color: #888; }
.message .text { margin-top: 4px; line-height: 1.4; word-wrap: break-word; }
.chat-row { padding: 10px 0; border-bottom: 1px solid #eee; display: flex; justify-content: space-between; }
.chat-row a { text-decoration: none; color: #1c7bb8; font-weight: 500; }
"#
    }
}

impl AbstractOutputWriter for ExportOutputHtmlOutput {
    fn format(&self) -> ExportFormat {
        ExportFormat::Html
    }

    fn start(&mut self) -> ExportOutputResultOutput {
        self.write_html_file("css/style.css", Self::generate_css())
    }

    fn finish(&mut self) -> ExportOutputResultOutput {
        ExportOutputResultOutput::success()
    }

    fn write_personal(&mut self, info: &ExportPersonalInfo) -> ExportOutputResultOutput {
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Telegram Profile</title><link rel=\"stylesheet\" href=\"css/style.css\"></head><body><div class=\"container\"><h1>Personal Info</h1><p><b>User ID:</b> {}</p><p><b>Name:</b> {} {}</p><p><b>Phone:</b> {}</p><p><b>Bio:</b> {}</p></div></body></html>",
            info.user_id,
            info.first_name,
            info.last_name,
            info.phone_number,
            info.bio.as_deref().unwrap_or(""),
        );
        self.write_html_file("about.html", &html)
    }

    fn write_userpics_slice(&mut self, items: &[ExportUserpicItem]) -> ExportOutputResultOutput {
        let mut list_html = String::new();
        for item in items {
            list_html.push_str(&format!(
                "<div class=\"userpic-item\"><img src=\"{}\" width=\"160\" height=\"160\"><p>Date: {}</p></div>",
                item.file_path, item.date
            ));
        }
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Userpics</title><link rel=\"stylesheet\" href=\"css/style.css\"></head><body><div class=\"container\"><h1>Profile Pictures</h1>{}</div></body></html>",
            list_html
        );
        self.write_html_file("userpics.html", &html)
    }

    fn write_contacts_list(&mut self, contacts: &[ExportContactItem]) -> ExportOutputResultOutput {
        let mut rows = String::new();
        for c in contacts {
            rows.push_str(&format!(
                "<div class=\"chat-row\"><span>{} {}</span><span>{}</span></div>",
                c.first_name, c.last_name, c.phone_number
            ));
        }
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Contacts</title><link rel=\"stylesheet\" href=\"css/style.css\"></head><body><div class=\"container\"><h1>Contacts List ({})</h1>{}</div></body></html>",
            contacts.len(), rows
        );
        self.write_html_file("contacts.html", &html)
    }

    fn write_sessions_list(&mut self, sessions: &[ExportSessionItem]) -> ExportOutputResultOutput {
        let mut rows = String::new();
        for s in sessions {
            rows.push_str(&format!(
                "<div class=\"chat-row\"><span><b>{} {}</b> ({})</span><span>{} - {}</span></div>",
                s.app_name, s.app_version, s.device_model, s.ip, s.country
            ));
        }
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Active Sessions</title><link rel=\"stylesheet\" href=\"css/style.css\"></head><body><div class=\"container\"><h1>Active Sessions ({})</h1>{}</div></body></html>",
            sessions.len(), rows
        );
        self.write_html_file("sessions.html", &html)
    }

    fn write_dialogs_list(&mut self, dialogs: &[ExportDialogItem]) -> ExportOutputResultOutput {
        let mut rows = String::new();
        for d in dialogs {
            rows.push_str(&format!(
                "<div class=\"chat-row\"><a href=\"chats/chat_{}/messages.html\">{}</a><span>{} messages ({})</span></div>",
                d.peer_id, d.name, d.messages_count, d.dialog_type
            ));
        }
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Telegram Data Export</title><link rel=\"stylesheet\" href=\"css/style.css\"></head><body><div class=\"container\"><h1>Telegram Chats ({})</h1><p><a href=\"about.html\">Personal Info</a> | <a href=\"contacts.html\">Contacts</a> | <a href=\"sessions.html\">Sessions</a></p>{}</div></body></html>",
            dialogs.len(), rows
        );
        self.write_html_file("index.html", &html)
    }

    fn write_messages_slice(&mut self, peer_id: i64, messages: &[ExportMessageItem]) -> ExportOutputResultOutput {
        let mut msgs_html = String::new();
        for m in messages {
            let date_str = chrono::DateTime::from_timestamp(m.date, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| m.date.to_string());
            msgs_html.push_str(&format!(
                "<div class=\"message\"><span class=\"from\">{}</span><span class=\"date\">{}</span><div class=\"text\">{}</div></div>",
                m.from_name, date_str, m.text
            ));
        }
        let html = format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Chat Messages</title><link rel=\"stylesheet\" href=\"../../css/style.css\"></head><body><div class=\"container\"><p><a href=\"../../index.html\">&larr; Back to chats</a></p><h1>Messages</h1>{}</div></body></html>",
            msgs_html
        );
        let path = format!("chats/chat_{peer_id}/messages.html");
        self.write_html_file(&path, &html)
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
