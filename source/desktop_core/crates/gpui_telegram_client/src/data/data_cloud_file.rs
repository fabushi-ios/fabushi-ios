//! Target Symbol: DataCloudFile
//! Telegram Desktop 1:1 Submodule: `data/data_cloud_file`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudFileLocation {
    pub dc_id: i32,
    pub id: u64,
    pub access_hash: u64,
    pub file_reference: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataCloudFile {
    pub location: CloudFileLocation,
    pub size_bytes: i64,
    pub is_downloaded: bool,
    pub local_cache_path: Option<String>,
}

impl Default for DataCloudFile {
    fn default() -> Self {
        Self {
            location: CloudFileLocation {
                dc_id: 1,
                id: 0,
                access_hash: 0,
                file_reference: Vec::new(),
            },
            size_bytes: 0,
            is_downloaded: false,
            local_cache_path: None,
        }
    }
}

impl DataCloudFile {
    pub fn new(dc_id: i32, id: u64, access_hash: u64, file_reference: Vec<u8>, size_bytes: i64) -> Self {
        Self {
            location: CloudFileLocation {
                dc_id,
                id,
                access_hash,
                file_reference,
            },
            size_bytes,
            is_downloaded: false,
            local_cache_path: None,
        }
    }

    pub fn set_cached(&mut self, path: impl Into<String>) {
        self.is_downloaded = true;
        self.local_cache_path = Some(path.into());
    }

    pub fn update_file_reference(&mut self, new_reference: Vec<u8>) {
        self.location.file_reference = new_reference;
    }
}
