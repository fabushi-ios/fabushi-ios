//! Target Symbol: StorageFolderArchive
//! Telegram Desktop 1:1 Submodule: `StorageFolderArchive`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub absolute: PathBuf,
    pub name: String,
    pub modified_timestamp: i64,
    pub size: i64,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveEntries {
    pub list: Vec<ArchiveEntry>,
    pub total_size: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveStatus {
    Done,
    TooLarge,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveWriteResult {
    pub status: ArchiveStatus,
    pub path: PathBuf,
    pub size: i64,
}

pub fn gather_archive_entries(folder_path: &Path) -> Result<ArchiveEntries, String> {
    let mut entries = Vec::new();
    let mut total_size = 0i64;

    fn walk_dir(
        root: &Path,
        current: &Path,
        entries: &mut Vec<ArchiveEntry>,
        total_size: &mut i64,
    ) -> Result<(), String> {
        let read_dir = fs::read_dir(current).map_err(|e| e.to_string())?;
        for entry in read_dir {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            let relative_name = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");

            let is_dir = metadata.is_dir();
            let size = if is_dir { 0 } else { metadata.len() as i64 };
            *total_size += size;

            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            entries.push(ArchiveEntry {
                absolute: path.clone(),
                name: relative_name,
                modified_timestamp: modified,
                size,
                is_directory: is_dir,
            });

            if is_dir {
                walk_dir(root, &path, entries, total_size)?;
            }
        }
        Ok(())
    }

    if folder_path.is_dir() {
        walk_dir(folder_path, folder_path, &mut entries, &mut total_size)?;
    } else if folder_path.is_file() {
        let metadata = fs::metadata(folder_path).map_err(|e| e.to_string())?;
        let size = metadata.len() as i64;
        total_size = size;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let name = folder_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        entries.push(ArchiveEntry {
            absolute: folder_path.to_path_buf(),
            name,
            modified_timestamp: modified,
            size,
            is_directory: false,
        });
    }

    Ok(ArchiveEntries {
        list: entries,
        total_size,
    })
}

pub fn archive_size_estimate(entries: &ArchiveEntries) -> i64 {
    // Estimating compressed size at ~80% of uncompressed plus zip headers overhead
    (entries.total_size as f64 * 0.8) as i64 + (entries.list.len() as i64 * 128)
}

pub fn write_archive(
    entries: &ArchiveEntries,
    target_zip_path: &Path,
    size_limit: i64,
    mut progress: impl FnMut(f64) -> bool,
) -> ArchiveWriteResult {
    if entries.total_size > size_limit {
        return ArchiveWriteResult {
            status: ArchiveStatus::TooLarge,
            path: target_zip_path.to_path_buf(),
            size: 0,
        };
    }

    let file = match File::create(target_zip_path) {
        Ok(f) => f,
        Err(_) => {
            return ArchiveWriteResult {
                status: ArchiveStatus::Failed,
                path: target_zip_path.to_path_buf(),
                size: 0,
            }
        }
    };

    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let total = entries.list.len().max(1);
    for (i, entry) in entries.list.iter().enumerate() {
        if !progress(i as f64 / total as f64) {
            let _ = fs::remove_file(target_zip_path);
            return ArchiveWriteResult {
                status: ArchiveStatus::Cancelled,
                path: target_zip_path.to_path_buf(),
                size: 0,
            };
        }

        if entry.is_directory {
            if zip.add_directory(&entry.name, options).is_err() {
                let _ = fs::remove_file(target_zip_path);
                return ArchiveWriteResult {
                    status: ArchiveStatus::Failed,
                    path: target_zip_path.to_path_buf(),
                    size: 0,
                };
            }
        } else {
            if zip.start_file(&entry.name, options).is_err() {
                let _ = fs::remove_file(target_zip_path);
                return ArchiveWriteResult {
                    status: ArchiveStatus::Failed,
                    path: target_zip_path.to_path_buf(),
                    size: 0,
                };
            }
            if let Ok(mut src) = File::open(&entry.absolute) {
                let mut buf = [0u8; 16384];
                while let Ok(n) = src.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    if zip.write_all(&buf[..n]).is_err() {
                        let _ = fs::remove_file(target_zip_path);
                        return ArchiveWriteResult {
                            status: ArchiveStatus::Failed,
                            path: target_zip_path.to_path_buf(),
                            size: 0,
                        };
                    }
                }
            }
        }
    }

    if zip.finish().is_err() {
        let _ = fs::remove_file(target_zip_path);
        return ArchiveWriteResult {
            status: ArchiveStatus::Failed,
            path: target_zip_path.to_path_buf(),
            size: 0,
        };
    }

    let final_size = fs::metadata(target_zip_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let _ = progress(1.0);

    ArchiveWriteResult {
        status: ArchiveStatus::Done,
        path: target_zip_path.to_path_buf(),
        size: final_size,
    }
}

pub fn clear_stale_archive_files(temp_dir: &Path, max_age_seconds: u64) {
    if let Ok(entries) = fs::read_dir(temp_dir) {
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(mod_time) = meta.modified() {
                    if let Ok(age) = now.duration_since(mod_time) {
                        if age.as_secs() > max_age_seconds {
                            let _ = fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct StorageFolderArchive {
    pub archives_created: usize,
}

impl StorageFolderArchive {
    pub fn new() -> Self {
        Self {
            archives_created: 0,
        }
    }

    pub fn prepare_and_write(
        &mut self,
        source: &Path,
        dest: &Path,
        size_limit: i64,
    ) -> ArchiveWriteResult {
        let entries = gather_archive_entries(source).unwrap_or_default();
        let result = write_archive(&entries, dest, size_limit, |_| true);
        if result.status == ArchiveStatus::Done {
            self.archives_created += 1;
        }
        result
    }
}
