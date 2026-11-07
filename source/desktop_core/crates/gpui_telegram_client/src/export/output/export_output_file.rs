//! Target Symbol: ExportOutputFileOutput
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/output/export_output_file.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use crate::export::output::export_output_result::ExportOutputResultOutput;

/// Sandboxed file writer managing output files matching upstream `Export::Output::File`
#[derive(Debug)]
pub struct ExportOutputFileOutput {
    base_dir: PathBuf,
    relative_path: PathBuf,
    active_file: Option<File>,
    bytes_written: i64,
}

impl ExportOutputFileOutput {
    pub fn new(base_dir: impl Into<PathBuf>, relative_path: impl Into<PathBuf>) -> io::Result<Self> {
        let base = base_dir.into();
        let rel = relative_path.into();
        let safe_rel = Self::prepare_relative_path(&rel)?;
        let full_path = base.join(&safe_rel);

        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&full_path)?;

        Ok(Self {
            base_dir: base,
            relative_path: safe_rel,
            active_file: Some(file),
            bytes_written: 0,
        })
    }

    /// Verifies relative path contains no directory traversal sequences (e.g. `..`)
    pub fn prepare_relative_path(path: &Path) -> io::Result<PathBuf> {
        let mut clean = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::Normal(c) => clean.push(c),
                std::path::Component::CurDir => {}
                _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Illegal path traversal")),
            }
        }
        Ok(clean)
    }

    pub fn write_block(&mut self, data: &[u8]) -> ExportOutputResultOutput {
        match self.write_block_attempt(data) {
            Ok(bytes) => {
                self.bytes_written += bytes as i64;
                ExportOutputResultOutput::success()
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::PermissionDenied || e.kind() == io::ErrorKind::ReadOnlyFilesystem {
                    ExportOutputResultOutput::fatal_error(format!("Fatal I/O error writing block: {e}"))
                } else {
                    ExportOutputResultOutput::error(format!("Recoverable I/O error writing block: {e}"))
                }
            }
        }
    }

    fn write_block_attempt(&mut self, data: &[u8]) -> io::Result<usize> {
        if let Some(ref mut f) = self.active_file {
            f.write_all(data)?;
            f.flush()?;
            Ok(data.len())
        } else {
            Err(io::Error::new(io::ErrorKind::NotConnected, "File is not open"))
        }
    }

    pub fn reopen(&mut self) -> io::Result<()> {
        let full_path = self.base_dir.join(&self.relative_path);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&full_path)?;
        self.active_file = Some(file);
        Ok(())
    }

    pub fn size(&self) -> i64 {
        self.bytes_written
    }

    pub fn is_empty(&self) -> bool {
        self.bytes_written == 0
    }

    pub fn copy_file(source: &Path, destination: &Path) -> io::Result<u64> {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)
    }
}
