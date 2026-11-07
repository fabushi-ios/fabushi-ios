//! Target Symbol: DataAutoDownload
//! Telegram Desktop 1:1 Submodule: `data/data_auto_download`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    Cellular,
    WiFi,
    Roaming,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoDownloadPreset {
    pub photo: bool,
    pub voice_message: bool,
    pub video_message: bool,
    pub video_file_size_limit: i64,
    pub other_file_size_limit: i64,
}

impl Default for AutoDownloadPreset {
    fn default() -> Self {
        Self {
            photo: true,
            voice_message: true,
            video_message: true,
            video_file_size_limit: 15 * 1024 * 1024, // 15MB
            other_file_size_limit: 3 * 1024 * 1024,  // 3MB
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataAutoDownload {
    pub cellular: AutoDownloadPreset,
    pub wifi: AutoDownloadPreset,
    pub roaming: AutoDownloadPreset,
    pub is_enabled: bool,
}

impl Default for DataAutoDownload {
    fn default() -> Self {
        Self {
            cellular: AutoDownloadPreset {
                photo: true,
                voice_message: true,
                video_message: false,
                video_file_size_limit: 5 * 1024 * 1024,
                other_file_size_limit: 1 * 1024 * 1024,
            },
            wifi: AutoDownloadPreset {
                photo: true,
                voice_message: true,
                video_message: true,
                video_file_size_limit: 30 * 1024 * 1024,
                other_file_size_limit: 10 * 1024 * 1024,
            },
            roaming: AutoDownloadPreset {
                photo: false,
                voice_message: false,
                video_message: false,
                video_file_size_limit: 0,
                other_file_size_limit: 0,
            },
            is_enabled: true,
        }
    }
}

impl DataAutoDownload {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn should_auto_download_photo(&self, net: NetworkType) -> bool {
        if !self.is_enabled {
            return false;
        }
        match net {
            NetworkType::Cellular => self.cellular.photo,
            NetworkType::WiFi => self.wifi.photo,
            NetworkType::Roaming => self.roaming.photo,
        }
    }

    pub fn should_auto_download_video(&self, net: NetworkType, size_bytes: i64) -> bool {
        if !self.is_enabled {
            return false;
        }
        let limit = match net {
            NetworkType::Cellular => self.cellular.video_file_size_limit,
            NetworkType::WiFi => self.wifi.video_file_size_limit,
            NetworkType::Roaming => self.roaming.video_file_size_limit,
        };
        size_bytes <= limit
    }

    pub fn should_auto_download_file(&self, net: NetworkType, size_bytes: i64) -> bool {
        if !self.is_enabled {
            return false;
        }
        let limit = match net {
            NetworkType::Cellular => self.cellular.other_file_size_limit,
            NetworkType::WiFi => self.wifi.other_file_size_limit,
            NetworkType::Roaming => self.roaming.other_file_size_limit,
        };
        size_bytes <= limit
    }
}
