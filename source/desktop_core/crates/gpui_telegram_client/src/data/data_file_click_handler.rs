//! Target Symbol: DataFileClickHandler
//! Telegram Desktop 1:1 Submodule: `data/data_file_click_handler`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileClickAction {
    Open,
    SaveAs,
    ShowInFolder,
    CancelDownload,
    PlayAudio,
    StreamVideo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFileClickHandler {
    pub document_id: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub local_path: Option<String>,
}

impl Default for DataFileClickHandler {
    fn default() -> Self {
        Self {
            document_id: 0,
            is_downloaded: false,
            is_downloading: false,
            local_path: None,
        }
    }
}

impl DataFileClickHandler {
    pub fn new(document_id: u64) -> Self {
        Self {
            document_id,
            is_downloaded: false,
            is_downloading: false,
            local_path: None,
        }
    }

    pub fn determine_action(&self, is_media: bool) -> FileClickAction {
        if self.is_downloading {
            FileClickAction::CancelDownload
        } else if self.is_downloaded {
            if is_media {
                FileClickAction::PlayAudio
            } else {
                FileClickAction::Open
            }
        } else if is_media {
            FileClickAction::StreamVideo
        } else {
            FileClickAction::SaveAs
        }
    }

    pub fn set_downloaded(&mut self, path: impl Into<String>) {
        self.is_downloaded = true;
        self.is_downloading = false;
        self.local_path = Some(path.into());
    }

    pub fn set_downloading(&mut self) {
        self.is_downloading = true;
    }
}
