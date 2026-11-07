//! Target Symbol: FileDownload
//! Telegram Desktop 1:1 Submodule: `FileDownload`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAX_FILE_IN_MEMORY: usize = 10 * 1024 * 1024;
pub const MAX_STICKER_BYTES_SIZE: usize = 2 * 1024 * 1024;
pub const MAX_WALL_PAPER_IN_MEMORY: usize = MAX_FILE_IN_MEMORY;
pub const MAX_ANIMATION_IN_MEMORY: usize = MAX_FILE_IN_MEMORY;
pub const MAX_WALL_PAPER_DIMENSION: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailureReason {
    #[default]
    NoFailure,
    FileWriteFailure,
    OtherFailure,
}

#[derive(Debug)]
pub struct FileLoader {
    pub destination_path: PathBuf,
    pub load_size: i64,
    pub full_size: i64,
    pub auto_loading: bool,
    pub cache_tag: u8,
    pub finished: bool,
    pub cancelled: bool,
    pub failure_reason: FailureReason,
    pub memory_buffer: Vec<u8>,
    file_handle: Option<File>,
}

impl FileLoader {
    pub fn new(
        destination_path: impl Into<PathBuf>,
        full_size: i64,
        auto_loading: bool,
        cache_tag: u8,
    ) -> Self {
        let dest = destination_path.into();
        Self {
            destination_path: dest,
            load_size: 0,
            full_size,
            auto_loading,
            cache_tag,
            finished: false,
            cancelled: false,
            failure_reason: FailureReason::NoFailure,
            memory_buffer: Vec::new(),
            file_handle: None,
        }
    }

    pub fn current_progress(&self) -> f64 {
        if self.full_size <= 0 {
            0.0
        } else {
            (self.load_size as f64 / self.full_size as f64).clamp(0.0, 1.0)
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.full_size > MAX_FILE_IN_MEMORY as i64 {
            if let Some(parent) = self.destination_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&self.destination_path)
                .map_err(|e| {
                    self.failure_reason = FailureReason::FileWriteFailure;
                    e.to_string()
                })?;
            self.file_handle = Some(file);
        }
        Ok(())
    }

    pub fn write_part(&mut self, offset: i64, data: &[u8]) -> Result<(), String> {
        if self.cancelled {
            return Err("Download cancelled".to_string());
        }

        if let Some(file) = &mut self.file_handle {
            file.seek(SeekFrom::Start(offset as u64))
                .map_err(|e| {
                    self.failure_reason = FailureReason::FileWriteFailure;
                    e.to_string()
                })?;
            file.write_all(data).map_err(|e| {
                self.failure_reason = FailureReason::FileWriteFailure;
                e.to_string()
            })?;
        } else {
            let end_offset = offset as usize + data.len();
            if self.memory_buffer.len() < end_offset {
                self.memory_buffer.resize(end_offset, 0);
            }
            self.memory_buffer[offset as usize..end_offset].copy_from_slice(data);
        }

        self.load_size = (self.load_size).max(offset + data.len() as i64);
        if self.load_size >= self.full_size && self.full_size > 0 {
            self.finalize()?;
        }
        Ok(())
    }

    pub fn finish_with_bytes(&mut self, data: &[u8]) {
        self.memory_buffer = data.to_vec();
        self.load_size = data.len() as i64;
        self.full_size = data.len() as i64;
        self.finished = true;
    }

    pub fn finalize(&mut self) -> Result<(), String> {
        if let Some(file) = &mut self.file_handle {
            file.sync_all().map_err(|e| {
                self.failure_reason = FailureReason::FileWriteFailure;
                e.to_string()
            })?;
        } else if !self.destination_path.as_os_str().is_empty() {
            if let Some(parent) = self.destination_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&self.destination_path, &self.memory_buffer).map_err(|e| {
                self.failure_reason = FailureReason::FileWriteFailure;
                e.to_string()
            })?;
        }
        self.finished = true;
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.file_handle = None;
        if self.destination_path.exists() {
            let _ = std::fs::remove_file(&self.destination_path);
        }
    }
}

#[derive(Debug, Default)]
pub struct FileDownload {
    pub active_loaders: usize,
}

impl FileDownload {
    pub fn new() -> Self {
        Self { active_loaders: 0 }
    }

    pub fn create_loader(&mut self, path: &Path, size: i64) -> FileLoader {
        self.active_loaders += 1;
        FileLoader::new(path, size, false, 0)
    }
}
