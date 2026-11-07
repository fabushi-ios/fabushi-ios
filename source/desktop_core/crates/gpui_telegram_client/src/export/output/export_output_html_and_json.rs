//! Target Symbol: ExportOutputHtmlAndJsonOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_html_and_json.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::{Path, PathBuf};
use crate::export::export_settings::ExportFormat;
use crate::export::output::export_output_abstract::{
    AbstractOutputWriter, ExportContactItem, ExportDialogItem, ExportMessageItem,
    ExportPersonalInfo, ExportSessionItem, ExportUserpicItem,
};
use crate::export::output::export_output_html::ExportOutputHtmlOutput;
use crate::export::output::export_output_json::ExportOutputJsonOutput;
use crate::export::output::export_output_result::ExportOutputResultOutput;

/// Dual-format output writer coordinating synchronous HTML and JSON generation
pub struct ExportOutputHtmlAndJsonOutput {
    pub html_writer: ExportOutputHtmlOutput,
    pub json_writer: ExportOutputJsonOutput,
}

impl Default for ExportOutputHtmlAndJsonOutput {
    fn default() -> Self {
        Self::new(PathBuf::from("Telegram_Export"))
    }
}

impl ExportOutputHtmlAndJsonOutput {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        let path = base_path.into();
        Self {
            html_writer: ExportOutputHtmlOutput::new(path.clone()),
            json_writer: ExportOutputJsonOutput::new(path),
        }
    }
}

impl AbstractOutputWriter for ExportOutputHtmlAndJsonOutput {
    fn format(&self) -> ExportFormat {
        ExportFormat::HtmlAndJson
    }

    fn start(&mut self) -> ExportOutputResultOutput {
        let r1 = self.html_writer.start();
        let r2 = self.json_writer.start();
        if !r1.is_success() { r1 } else { r2 }
    }

    fn finish(&mut self) -> ExportOutputResultOutput {
        let r1 = self.html_writer.finish();
        let r2 = self.json_writer.finish();
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_personal(&mut self, info: &ExportPersonalInfo) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_personal(info);
        let r2 = self.json_writer.write_personal(info);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_userpics_slice(&mut self, items: &[ExportUserpicItem]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_userpics_slice(items);
        let r2 = self.json_writer.write_userpics_slice(items);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_contacts_list(&mut self, contacts: &[ExportContactItem]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_contacts_list(contacts);
        let r2 = self.json_writer.write_contacts_list(contacts);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_sessions_list(&mut self, sessions: &[ExportSessionItem]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_sessions_list(sessions);
        let r2 = self.json_writer.write_sessions_list(sessions);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_dialogs_list(&mut self, dialogs: &[ExportDialogItem]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_dialogs_list(dialogs);
        let r2 = self.json_writer.write_dialogs_list(dialogs);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_messages_slice(&mut self, peer_id: i64, messages: &[ExportMessageItem]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_messages_slice(peer_id, messages);
        let r2 = self.json_writer.write_messages_slice(peer_id, messages);
        if !r1.is_success() { r1 } else { r2 }
    }

    fn write_other_data(&mut self, relative_path: &str, content: &[u8]) -> ExportOutputResultOutput {
        let r1 = self.html_writer.write_other_data(relative_path, content);
        let r2 = self.json_writer.write_other_data(relative_path, content);
        if !r1.is_success() { r1 } else { r2 }
    }
}
