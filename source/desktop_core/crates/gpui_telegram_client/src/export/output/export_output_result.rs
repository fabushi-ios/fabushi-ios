//! Target Symbol: ExportOutputResultOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_result.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use serde::{Deserialize, Serialize};

/// Triple-state execution result for export operations matching upstream `Export::Output::Result`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExportResultType {
    #[default]
    Success,
    Error,
    FatalError,
}

/// Output operation result container matching `Export::Output::Result`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportOutputResultOutput {
    pub status: ExportResultType,
    pub message: Option<String>,
    pub file_path: Option<String>,
}

impl Default for ExportOutputResultOutput {
    fn default() -> Self {
        Self::success()
    }
}

impl ExportOutputResultOutput {
    pub fn success() -> Self {
        Self {
            status: ExportResultType::Success,
            message: None,
            file_path: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            status: ExportResultType::Error,
            message: Some(msg.into()),
            file_path: None,
        }
    }

    pub fn fatal_error(msg: impl Into<String>) -> Self {
        Self {
            status: ExportResultType::FatalError,
            message: Some(msg.into()),
            file_path: None,
        }
    }

    pub fn with_file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    pub fn is_success(&self) -> bool {
        matches!(self.status, ExportResultType::Success)
    }

    pub fn is_error(&self) -> bool {
        matches!(self.status, ExportResultType::Error)
    }

    pub fn is_fatal_error(&self) -> bool {
        matches!(self.status, ExportResultType::FatalError)
    }

    pub fn is_ok(&self) -> bool {
        self.is_success()
    }
}
