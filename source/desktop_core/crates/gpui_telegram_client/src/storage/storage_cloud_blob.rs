//! Target Symbol: StorageCloudBlob
//! Telegram Desktop 1:1 Submodule: `StorageCloudBlob`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blob {
    pub id: i32,
    pub post_id: i32,
    pub size: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlobState {
    Available { size: i64 },
    Loading { progress: f64 },
    Ready,
    Active,
    Failed { reason: String },
}

pub fn unpack_blob(
    archive_path: &Path,
    dest_folder: &Path,
    check_name: impl Fn(&str) -> bool,
) -> Result<usize, String> {
    let file = File::open(archive_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    if !dest_folder.exists() {
        fs::create_dir_all(dest_folder).map_err(|e| e.to_string())?;
    }

    let mut unpacked_count = 0;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().map_err(|e| e.to_string())?.to_string();
        if !check_name(&name) {
            continue;
        }

        let out_path = dest_folder.join(&name);
        if file.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
            }
            let mut out_file = File::create(&out_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out_file).map_err(|e| e.to_string())?;
            unpacked_count += 1;
        }
    }

    Ok(unpacked_count)
}

#[derive(Debug, Clone)]
pub struct StorageCloudBlob {
    pub blob: Blob,
    pub state: BlobState,
    pub storage_folder: PathBuf,
}

impl Default for StorageCloudBlob {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageCloudBlob {
    pub fn new() -> Self {
        Self {
            blob: Blob::default(),
            state: BlobState::Available { size: 0 },
            storage_folder: PathBuf::from("cloud_blobs"),
        }
    }

    pub fn set_state(&mut self, state: BlobState) {
        self.state = state;
    }

    pub fn unpack(&mut self, archive_path: &Path) -> Result<usize, String> {
        let count = unpack_blob(archive_path, &self.storage_folder, |_| true)?;
        self.state = BlobState::Ready;
        Ok(count)
    }
}
